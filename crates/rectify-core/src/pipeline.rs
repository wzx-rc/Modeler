//! The five steps in order, each one keeping its result so that a step can
//! be run again with new settings without redoing the ones before it.

use crate::clean;
use crate::mesh::{Mesh, Topo};
use crate::regularize::{regularize, RegParams, RegReport};
use crate::revolve::{revolve, RevolveParams, RevolveReport};
use crate::script::{self, Script};
use crate::segment::{segment, SegParams, Segmentation};
use crate::smooth::{smooth, SmoothParams};
use crate::snap::{snap, SnapParams, SnapReport};
use crate::symmetry::{self, SymReport};
use crate::tidy::{tidy, Diff, TidyParams, TidyReport};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    Symmetry,
    Smooth,
    Segment,
    Regularize,
    Snap,
    Revolve,
    Clean,
}

impl Step {
    pub const ALL: [Step; 7] = [Step::Symmetry, Step::Smooth, Step::Segment, Step::Regularize, Step::Snap, Step::Revolve, Step::Clean];
    /// The steps run once per lane (the algorithm's, the edited one).
    pub const LANE: [Step; 4] = [Step::Regularize, Step::Snap, Step::Revolve, Step::Clean];
    pub fn label(self) -> &'static str {
        match self {
            Step::Symmetry => "Symmetry",
            Step::Smooth => "Smooth",
            Step::Segment => "Regions",
            Step::Regularize => "Regularize",
            Step::Snap => "Snap",
            Step::Revolve => "Round",
            Step::Clean => "Clean",
        }
    }
    /// The lane step's place in `Lane::times`.
    pub fn lane_index(self) -> Option<usize> {
        Step::LANE.iter().position(|s| *s == self)
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
    pub revolve: RevolveParams,
    pub tidy: TidyParams,
}

impl Default for Params {
    fn default() -> Self {
        Params { symmetry: true, sym_tol: 0.25, smooth_on: true, smooth: Default::default(), seg: Default::default(), reg_on: true, reg: Default::default(), snap: Default::default(), revolve: Default::default(), tidy: Default::default() }
    }
}

impl Params {
    /// Settings from text, "fade=2,tol=0.3" (the capture harness and the
    /// probe try settings this way). Unknown keys are returned.
    pub fn apply(&mut self, text: &str) -> Vec<String> {
        let mut unknown = vec![];
        for kv in text.split(',') {
            let mut it = kv.split('=');
            let (Some(k), Some(v)) = (it.next(), it.next().and_then(|v| v.trim().parse::<f64>().ok())) else { continue };
            match k.trim() {
                "fade" => self.snap.fade = v,
                "flatten" => self.snap.flatten = v,
                "gap" => self.snap.gap = v,
                "edge_tol" => self.snap.edge_tol = v,
                "edge_angle" => self.snap.edge_angle = v,
                "max_move" => self.snap.max_move = v,
                "round" => self.snap.round = v,
                "min_radius" => self.snap.min_radius = v,
                "relax" => self.snap.relax = v as usize,
                "end_fade" => self.snap.end_fade = v,
                "tol" => self.seg.tol = v,
                "angle" => self.seg.angle = v,
                "min_area" => self.seg.min_area = v,
                "max_radius" => self.seg.max_radius = v,
                "smooth_radius" => self.smooth.radius = v,
                "smooth_angle" => self.smooth.angle = v,
                "smooth_iters" => self.smooth.normal_iters = v as usize,
                "vertex_iters" => self.smooth.vertex_iters = v as usize,
                "sym_tol" => self.sym_tol = v,
                "symmetry" => self.symmetry = v != 0.0,
                "reg" => self.reg_on = v != 0.0,
                "revolve" => self.revolve.on = v != 0.0,
                "rev_refine" => self.revolve.refine = v != 0.0,
                "rev_turn_tol" => self.revolve.turn_tol = v,
                "rev_radius" => self.revolve.radius = v,
                "rev_thick" => self.revolve.max_thick = v,
                "rev_outlier" => self.revolve.outlier = v,
                "rev_margin" => self.revolve.margin = v,
                "rev_cover" => self.revolve.min_cover = v,
                "tidy" => self.tidy.on = v != 0.0,
                "tidy_margin" => self.tidy.margin = v,
                "tidy_edge" => self.tidy.edge_margin = v,
                "tidy_noise" => self.tidy.noise = v,
                "tidy_edge_noise" => self.tidy.edge_noise = v,
                "tidy_iters" => self.tidy.iters = v as usize,
                "tidy_rings" => self.tidy.rings = v as usize,
                "tidy_revert" => self.tidy.revert = v != 0.0,
                "tidy_fade" => self.tidy.fade = v,
                "tidy_step" => self.tidy.revert_step = v,
                "tidy_calm" => self.tidy.calm = v != 0.0,
                k => unknown.push(k.to_string()),
            }
        }
        unknown
    }
}

/// The stages shown in the top bar: what the mesh looks like after each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// The mesh as it came (the scan, the AI's output).
    Scan,
    /// The algorithm alone: regions, shapes, snap, round pieces.
    Algorithm,
    /// Then cleaned: never rougher than the AI mesh.
    Cleaned,
    /// The algorithm driven by the edits (the AI pass, the hand tools).
    Guided,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Scan, Stage::Algorithm, Stage::Cleaned, Stage::Guided];
    pub fn label(self) -> &'static str {
        match self {
            Stage::Scan => "Scan",
            Stage::Algorithm => "Algorithm",
            Stage::Cleaned => "Cleaned",
            Stage::Guided => "AI",
        }
    }
    pub fn number(self) -> usize {
        Stage::ALL.iter().position(|s| *s == self).unwrap()
    }
    /// The lane it is computed in (none for the scan).
    pub fn lane(self) -> Option<usize> {
        match self {
            Stage::Scan => None,
            Stage::Algorithm | Stage::Cleaned => Some(0),
            Stage::Guided => Some(1),
        }
    }
    /// The last step it needs.
    pub fn last_step(self) -> Option<Step> {
        match self {
            Stage::Scan => None,
            Stage::Algorithm => Some(Step::Revolve),
            Stage::Cleaned | Stage::Guided => Some(Step::Clean),
        }
    }
}

/// The steps from Regularize on, run on one set of regions: the
/// algorithm's own (lane 0) or the edited ones (lane 1).
#[derive(Default)]
pub struct Lane {
    pub seg: Option<Segmentation>,
    pub reg: Option<RegReport>,
    pub seg_final: Option<Segmentation>,
    pub snapped: Option<Mesh>,
    pub snap: Option<SnapReport>,
    /// After the round pieces (stage 1's mesh in lane 0).
    pub result: Option<Mesh>,
    pub revolve: Option<RevolveReport>,
    /// After the clean-up (stage 2's mesh in lane 0, stage 3's in lane 1).
    pub cleaned: Option<Mesh>,
    pub tidy: Option<TidyReport>,
    /// The diff maps of `result` and `cleaned` against the AI mesh.
    pub diff_result: Option<Diff>,
    pub diff_cleaned: Option<Diff>,
    /// Seconds per lane step (`Step::lane_index`).
    pub times: [Option<f64>; 4],
}

impl Lane {
    fn clear_from(&mut self, k: usize) {
        if k <= 0 {
            self.seg = None;
            self.reg = None;
        }
        if k <= 1 {
            self.seg_final = None;
            self.snapped = None;
            self.snap = None;
        }
        if k <= 2 {
            self.result = None;
            self.revolve = None;
        }
        self.cleaned = None;
        self.tidy = None;
        self.diff_result = None;
        self.diff_cleaned = None;
        for i in k..4 {
            self.times[i] = None;
        }
    }
}

pub struct Pipeline {
    pub original: Mesh,
    pub topo: Topo,
    pub sym: Option<SymReport>,
    pub sym_mesh: Option<Mesh>,
    pub smooth_mesh: Option<Mesh>,
    pub seg_raw: Option<Segmentation>,
    /// Seconds for Symmetry, Smooth, Segment.
    pub times: [Option<f64>; 3],
    /// 0: the algorithm's regions; 1: the edited ones.
    pub lanes: [Lane; 2],
    /// The edits (stage 3), and the regions they give.
    pub script: Script,
    pub seg_edit: Option<Segmentation>,
    /// What each edit did (or why it failed), in order.
    pub edit_log: Vec<Result<String, String>>,
}

impl Pipeline {
    pub fn new(original: Mesh) -> Pipeline {
        let topo = Topo::new(&original);
        Pipeline { original, topo, sym: None, sym_mesh: None, smooth_mesh: None, seg_raw: None, times: [None; 3], lanes: [Lane::default(), Lane::default()], script: Script::default(), seg_edit: None, edit_log: vec![] }
    }

    /// The step's results and every later one's are dropped, in both lanes
    /// (a lane step: in that lane only).
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
            self.seg_edit = None;
        }
        for k in i.min(3)..3 {
            self.times[k] = None;
        }
        let k = step.lane_index().unwrap_or(0);
        for l in &mut self.lanes {
            l.clear_from(k);
        }
    }

    /// Drops a lane's results from `step` on.
    pub fn clear_lane(&mut self, lane: usize, step: Step) {
        self.lanes[lane].clear_from(step.lane_index().unwrap_or(0));
    }

    /// The edits changed: the edited regions and lane 1 are out of date.
    pub fn edits_changed(&mut self) {
        self.seg_edit = None;
        self.lanes[1].clear_from(0);
    }

    pub fn done_in(&self, step: Step, lane: usize) -> bool {
        match step.lane_index() {
            Some(k) => self.lanes[lane].times[k].is_some(),
            None => self.times[step.index()].is_some(),
        }
    }

    pub fn done(&self, step: Step) -> bool {
        self.done_in(step, 0)
    }

    pub fn time_of(&self, step: Step, lane: usize) -> Option<f64> {
        match step.lane_index() {
            Some(k) => self.lanes[lane].times[k],
            None => self.times[step.index()],
        }
    }

    /// The first step `stage` still needs (None: it is ready).
    pub fn next_for(&self, stage: Stage) -> Option<Step> {
        let (Some(lane), Some(last)) = (stage.lane(), stage.last_step()) else { return None };
        Step::ALL.into_iter().take(last.index() + 1).find(|s| !self.done_in(*s, lane))
    }

    /// The first step not run yet for stages 1 and 2 (None: all done).
    pub fn next(&self) -> Option<Step> {
        self.next_for(Stage::Cleaned)
    }

    pub fn ready(&self, stage: Stage) -> bool {
        stage == Stage::Scan || self.next_for(stage).is_none()
    }

    /// The mesh of a stage, if it is computed.
    pub fn stage_mesh(&self, stage: Stage) -> Option<&Mesh> {
        match stage {
            Stage::Scan => Some(&self.original),
            Stage::Algorithm => self.lanes[0].result.as_ref(),
            Stage::Cleaned => self.lanes[0].cleaned.as_ref(),
            Stage::Guided => self.lanes[1].cleaned.as_ref(),
        }
    }

    /// The diff map of a stage against the AI mesh.
    pub fn stage_diff(&self, stage: Stage) -> Option<&Diff> {
        match stage {
            Stage::Scan => None,
            Stage::Algorithm => self.lanes[0].diff_result.as_ref(),
            Stage::Cleaned => self.lanes[0].diff_cleaned.as_ref(),
            Stage::Guided => self.lanes[1].diff_cleaned.as_ref(),
        }
    }

    /// The regions a stage's mesh was made with.
    pub fn stage_seg(&self, stage: Stage) -> Option<&Segmentation> {
        match stage.lane() {
            None => None,
            Some(l) => {
                let lane = &self.lanes[l];
                let input = if l == 1 { self.seg_edit.as_ref().or(self.seg_raw.as_ref()) } else { self.seg_raw.as_ref() };
                lane.seg_final.as_ref().or(lane.seg.as_ref()).or(input)
            }
        }
    }

    fn sym_out(&self) -> &Mesh {
        self.sym_mesh.as_ref().unwrap_or(&self.original)
    }

    /// The mesh the snap moves (symmetrized, not smoothed): the AI mesh the
    /// clean-up compares with.
    pub fn base(&self) -> &Mesh {
        self.sym_out()
    }

    pub fn smooth_out(&self) -> &Mesh {
        self.smooth_mesh.as_ref().unwrap_or_else(|| self.sym_out())
    }

    /// The edited regions: the algorithm's, with the script's region edits
    /// replayed on them.
    pub fn build_edits(&mut self, p: &Params) {
        let Some(raw) = &self.seg_raw else { return };
        let mut s = raw.clone();
        let m = self.smooth_mesh.as_ref().or(self.sym_mesh.as_ref()).unwrap_or(&self.original);
        let mut log = vec![];
        for e in &self.script.edits {
            match script::apply_region_edit(m, &self.topo, &mut s, &e.cmd, &p.seg) {
                Ok(Some(t)) => log.push(Ok(t)),
                Ok(None) => log.push(Ok(String::new())),
                Err(t) => log.push(Err(t)),
            }
        }
        self.seg_edit = Some(s);
        self.edit_log = log;
    }

    /// Runs one step (the ones before it must be done); a lane step in
    /// `lane`.
    pub fn run(&mut self, step: Step, lane: usize, p: &Params, progress: &(dyn Fn(f32) + Sync)) {
        let t0 = Instant::now();
        match step.lane_index() {
            Some(k) => self.lanes[lane].clear_from(k),
            None => self.clear_from(step),
        }
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
                if lane == 1 && self.seg_edit.is_none() {
                    self.build_edits(p);
                }
                let input = if lane == 1 { self.seg_edit.as_ref() } else { self.seg_raw.as_ref() };
                let Some(raw) = input else { return };
                let mut s = raw.clone();
                if p.reg_on {
                    let mut rp = p.reg;
                    rp.max_rms = rp.max_rms.max(p.seg.tol * 0.6);
                    let mir = self.sym.as_ref().filter(|r| r.matched > 0.5).map(|r| r.mirror);
                    let rep = regularize(self.smooth_out(), &self.topo, &mut s, mir.as_ref(), &rp);
                    self.lanes[lane].reg = Some(rep);
                }
                // the shape constraints of the edits, on top
                if lane == 1 {
                    let mut log = std::mem::take(&mut self.edit_log);
                    let m = self.smooth_out();
                    for (i, e) in self.script.edits.iter().enumerate() {
                        match script::apply_constraint(m, &mut s, &e.cmd) {
                            Ok(Some(t)) => {
                                if let Some(x) = log.get_mut(i) {
                                    *x = Ok(t)
                                }
                            }
                            Ok(None) => {}
                            Err(t) => {
                                if let Some(x) = log.get_mut(i) {
                                    *x = Err(t)
                                }
                            }
                        }
                    }
                    for (i, e) in self.script.edits.iter().enumerate() {
                        let note = match &e.cmd {
                            script::Cmd::Revolve { .. } => "round piece asked for (see Round)",
                            script::Cmd::NoRevolve { .. } => "round piece dropped (see Round)",
                            script::Cmd::Keep { .. } => "zone kept as the AI mesh (see Clean)",
                            _ => continue,
                        };
                        if let Some(x) = log.get_mut(i) {
                            *x = Ok(note.into());
                        }
                    }
                    self.edit_log = log;
                }
                self.lanes[lane].seg = Some(s);
            }
            Step::Snap => {
                let Some(reg) = &self.lanes[lane].seg else { return };
                let mut s = reg.clone();
                // the borders on the smoothed mesh (where the regions were
                // found), the move on the unsmoothed one: free areas keep
                // their fine detail
                let sm = self.smooth_out();
                clean::borders(sm, &self.topo, &mut s, p.seg.angle);
                clean::islands(sm, &self.topo, &mut s, p.seg.min_area);
                let mut m = self.sym_out().clone();
                progress(0.3);
                let rep = snap(&mut m, &self.topo, &s, &p.snap);
                let l = &mut self.lanes[lane];
                l.snap = Some(rep);
                l.snapped = Some(m);
                l.seg_final = Some(s);
            }
            Step::Revolve => {
                let (Some(m0), Some(s)) = (&self.lanes[lane].snapped, &self.lanes[lane].seg_final) else { return };
                let mut m = m0.clone();
                let (hints, off) = if lane == 1 { script::revolve_hints(&self.script.edits) } else { (vec![], vec![]) };
                let rep = revolve(&mut m, self.sym_out(), &self.topo, s, &hints, &off, &p.revolve);
                let l = &mut self.lanes[lane];
                l.revolve = Some(rep);
                l.result = Some(m);
            }
            Step::Clean => {
                let Some(m0) = &self.lanes[lane].result else { return };
                let mut m = m0.clone();
                let kinds = self.lanes[lane].snap.as_ref().map(|r| r.kinds.clone()).unwrap_or_default();
                let kinds = if kinds.len() == m.pos.len() { kinds } else { vec![0; m.pos.len()] };
                let keep = if lane == 1 { script::keep_zones(&self.script.edits) } else { vec![] };
                let (rep, before, after) = tidy(self.sym_out(), &mut m, &self.topo, &kinds, &keep, &p.tidy);
                let l = &mut self.lanes[lane];
                l.tidy = Some(rep);
                l.diff_result = Some(before);
                l.diff_cleaned = Some(after);
                l.cleaned = Some(m);
            }
        }
        let secs = t0.elapsed().as_secs_f64();
        match step.lane_index() {
            Some(k) => self.lanes[lane].times[k] = Some(secs),
            None => self.times[step.index()] = Some(secs),
        }
        progress(1.0);
    }

    /// Runs what `stage` still needs.
    pub fn run_stage(&mut self, stage: Stage, p: &Params, progress: &(dyn Fn(Step, f32) + Sync)) {
        let Some(lane) = stage.lane() else { return };
        while let Some(s) = self.next_for(stage) {
            self.run(s, lane, p, &|f| progress(s, f));
        }
    }

    /// Runs `step` and every one after it (lane 0, and lane 1 when it has
    /// edits).
    pub fn run_from(&mut self, step: Step, p: &Params, progress: &(dyn Fn(Step, f32) + Sync)) {
        self.clear_from(step);
        self.run_stage(Stage::Cleaned, p, progress);
        if !self.script.edits.is_empty() {
            self.run_stage(Stage::Guided, p, progress);
        }
    }
}
