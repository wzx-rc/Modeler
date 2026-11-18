//! The five steps in order, each one keeping its result so that a step can
//! be run again with new settings without redoing the ones before it.

use crate::clean;
use crate::mesh::{Mesh, Topo};
use crate::regularize::{regularize, RegParams, RegReport};
use crate::segment::{segment, SegParams, Segmentation};
use crate::smooth::{smooth, SmoothParams};
use crate::snap::{snap, SnapParams, SnapReport};
use crate::symmetry::{self, SymReport};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    Symmetry,
    Smooth,
    Segment,
    Regularize,
    Snap,
}

impl Step {
    pub const ALL: [Step; 5] = [Step::Symmetry, Step::Smooth, Step::Segment, Step::Regularize, Step::Snap];
    pub fn label(self) -> &'static str {
        match self {
            Step::Symmetry => "Symmetry",
            Step::Smooth => "Smooth",
            Step::Segment => "Regions",
            Step::Regularize => "Regularize",
            Step::Snap => "Snap",
        }
    }
    pub fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub symmetry: bool,
    /// Largest distance between a point and its mirror partner (mm).
    pub sym_tol: f64,
    pub smooth_on: bool,
    pub smooth: SmoothParams,
    pub seg: SegParams,
    pub reg_on: bool,
    pub reg: RegParams,
    pub snap: SnapParams,
}

impl Default for Params {
    fn default() -> Self {
        Params { symmetry: true, sym_tol: 0.25, smooth_on: true, smooth: Default::default(), seg: Default::default(), reg_on: true, reg: Default::default(), snap: Default::default() }
    }
}

pub struct Pipeline {
    pub original: Mesh,
    pub topo: Topo,
    pub sym: Option<SymReport>,
    pub sym_mesh: Option<Mesh>,
    pub smooth_mesh: Option<Mesh>,
    pub seg_raw: Option<Segmentation>,
    pub seg: Option<Segmentation>,
    pub reg: Option<RegReport>,
    pub seg_final: Option<Segmentation>,
    pub result: Option<Mesh>,
    pub snap: Option<SnapReport>,
    /// Seconds each step took (by `Step::index`), None when not run.
    pub times: [Option<f64>; 5],
}

fn clone_seg(s: &Segmentation) -> Segmentation {
    Segmentation { label: s.label.clone(), regions: s.regions.clone() }
}

impl Pipeline {
    pub fn new(original: Mesh) -> Pipeline {
        let topo = Topo::new(&original);
        Pipeline { original, topo, sym: None, sym_mesh: None, smooth_mesh: None, seg_raw: None, seg: None, reg: None, seg_final: None, result: None, snap: None, times: [None; 5] }
    }

    /// The step's results and every later one's are dropped.
    pub fn clear_from(&mut self, step: Step) {
        let i = step.index();
        if i <= 0 {
            self.sym = None;
            self.sym_mesh = None;
        }
        if i <= 1 {
            self.smooth_mesh = None;
        }
        if i <= 2 {
            self.seg_raw = None;
        }
        if i <= 3 {
            self.seg = None;
            self.reg = None;
        }
        if i <= 4 {
            self.seg_final = None;
            self.result = None;
            self.snap = None;
        }
        for k in i..5 {
            self.times[k] = None;
        }
    }

    pub fn done(&self, step: Step) -> bool {
        self.times[step.index()].is_some()
    }

    /// The first step not run yet (None: all done).
    pub fn next(&self) -> Option<Step> {
        Step::ALL.into_iter().find(|s| !self.done(*s))
    }

    fn sym_out(&self) -> &Mesh {
        self.sym_mesh.as_ref().unwrap_or(&self.original)
    }

    pub fn smooth_out(&self) -> &Mesh {
        self.smooth_mesh.as_ref().unwrap_or_else(|| self.sym_out())
    }

    /// Runs one step (the ones before it must be done).
    pub fn run(&mut self, step: Step, p: &Params, progress: &(dyn Fn(f32) + Sync)) {
        let t0 = Instant::now();
        self.clear_from(step);
        match step {
            Step::Symmetry => {
                if p.symmetry {
                    let rep = symmetry::find(&self.original, p.sym_tol);
                    progress(0.4);
                    let mut m = self.original.clone();
                    // a poor mirror (less than half of the points matched) is not used
                    if rep.matched > 0.5 {
                        let plane_tol = (m.edge_median() * 0.5).min(0.05);
                        symmetry::symmetrize(&mut m, &self.topo, &rep.mirror, p.sym_tol, plane_tol);
                        self.sym_mesh = Some(m);
                    }
                    self.sym = Some(rep);
                }
            }
            Step::Smooth => {
                if p.smooth_on {
                    let mut m = self.sym_out().clone();
                    smooth(&mut m, &self.topo, &p.smooth);
                    self.smooth_mesh = Some(m);
                }
            }
            Step::Segment => {
                let m = self.smooth_out();
                let mut s = segment(m, &self.topo, &p.seg, &|f| progress(f * 0.7));
                clean::merge(m, &self.topo, &mut s, &p.seg);
                progress(0.85);
                clean::borders(m, &self.topo, &mut s, p.seg.angle);
                clean::islands(m, &self.topo, &mut s, p.seg.min_area);
                clean::small(&mut s, p.seg.min_area);
                clean::compact(&mut s);
                clean::refit(m, &self.topo, &mut s.regions);
                self.seg_raw = Some(s);
            }
            Step::Regularize => {
                let Some(raw) = &self.seg_raw else { return };
                let mut s = clone_seg(raw);
                if p.reg_on {
                    let mut rp = p.reg;
                    rp.max_rms = rp.max_rms.max(p.seg.tol * 0.6);
                    let mir = self.sym.as_ref().filter(|r| r.matched > 0.5).map(|r| r.mirror);
                    let rep = regularize(self.smooth_out(), &self.topo, &mut s, mir.as_ref(), &rp);
                    self.reg = Some(rep);
                }
                self.seg = Some(s);
            }
            Step::Snap => {
                let Some(reg) = &self.seg else { return };
                let mut s = clone_seg(reg);
                // the borders on the smoothed mesh (where the regions were
                // found), the move on the unsmoothed one: free areas keep
                // their fine detail
                let sm = self.smooth_out();
                clean::borders(sm, &self.topo, &mut s, p.seg.angle);
                clean::islands(sm, &self.topo, &mut s, p.seg.min_area);
                let mut m = self.sym_out().clone();
                progress(0.3);
                let rep = snap(&mut m, &self.topo, &s, &p.snap);
                self.snap = Some(rep);
                self.result = Some(m);
                self.seg_final = Some(s);
            }
        }
        self.times[step.index()] = Some(t0.elapsed().as_secs_f64());
        progress(1.0);
    }

    /// Runs `step` and every one after it.
    pub fn run_from(&mut self, step: Step, p: &Params, progress: &(dyn Fn(Step, f32) + Sync)) {
        for s in Step::ALL.into_iter().skip(step.index()) {
            self.run(s, p, &|f| progress(s, f));
        }
    }
}
