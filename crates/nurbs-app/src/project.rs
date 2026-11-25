//! Project curve (Romain 24 nov., 19 h 03: "j'ai une curve et je voudrais
//! la projeter pour couper un mesh… un peu comme Maya, projette curve en
//! mesh… soit ça coupe et ça supprime la partie, soit ça projette juste la
//! curve et comme ça je peux manipuler après ma surface"). A history step
//! (`Feature.project`) in the Object under the curve: the kernel sweeps the
//! curve along its own normal and either splits the faces it meets first
//! along it (both pieces keep their tag, as the Knife's, so one can be
//! extruded or moved), or cuts the part through (`build::project`). The
//! curve stays: moved or edited, it projects again.

use crate::state::{AppState, SelMode};
use glam::DVec3;
use nurbs_kernel::{Feature, FeatureId, Project};

impl AppState {
    /// The Object the curve projects onto: the one whose faces the curve's
    /// points meet first, seen from its side (along its normal), most of
    /// them; else the one last worked on.
    pub fn project_target(&self, c: &Feature) -> Option<FeatureId> {
        let n = c.axis().normalize();
        let pts: Vec<DVec3> = self.curve_lines(c).into_iter().flatten().collect();
        let mut votes: Vec<(FeatureId, usize)> = vec![];
        if let Some(m) = &self.model {
            let far = m.bounds().map(|(lo, hi)| (glam::Vec3::from(hi) - glam::Vec3::from(lo)).length() as f64).unwrap_or(1000.0) * 2.0 + 10.0;
            let step = (pts.len() / 24).max(1);
            for p in pts.iter().step_by(step) {
                let from = *p + n * far;
                let Some(h) = self.picker.pick(m, from.as_vec3(), (-n).as_vec3()) else { continue };
                let Some(t) = m.faces.get(h.face).and_then(|f| f.tag) else { continue };
                let shape = self.element_of(t.feature);
                let Some(o) = self.doc.object_of(shape) else { continue };
                match votes.iter_mut().find(|v| v.0 == o) {
                    Some(v) => v.1 += 1,
                    None => votes.push((o, 1)),
                }
            }
        }
        votes.sort_by(|a, b| b.1.cmp(&a.1));
        votes.first().map(|v| v.0).or(self.last_object.filter(|o| self.doc.object(*o).is_some())).or_else(|| (self.doc.objects.len() == 1).then(|| self.doc.objects[0].id))
    }

    /// A Project curve step for the curve `curve`: drawn on the faces it
    /// meets (`cut` false) or cutting through; selected, its window open.
    pub fn add_project(&mut self, curve: FeatureId, cut: bool) -> Option<FeatureId> {
        let Some(c) = self.doc.feature(curve).cloned().filter(|c| c.is_path()) else {
            self.toast("Project: pick a curve", true);
            return None;
        };
        // already projected: that projection takes the mode asked, no
        // second one on the same faces (Romain 25 nov.: clicking Draw on
        // faces again made Project 2 and 3, which drew nothing)
        let made = self.projections_of(curve).into_iter().rev().find(|p| self.doc.feature(*p).is_some_and(|f| f.visible && !f.suspended));
        if let Some((pid, p)) = made.and_then(|pid| Some((pid, self.doc.feature(pid)?.project.clone()?))) {
            if p.cut != cut {
                self.set_project(pid, Project { cut, ..p });
            }
            self.set_sel_mode(SelMode::Object);
            self.selection = vec![pid];
            let name = self.doc.feature(pid).map(|f| f.name.clone()).unwrap_or_default();
            self.toast(&format!("{} is projected already: {name} {}", c.name, if cut { "cuts through" } else { "draws on the faces" }), false);
            return Some(pid);
        }
        let Some(o) = self.project_target(&c) else {
            self.toast("Project: the curve is over no Object (it projects along its normal)", true);
            return None;
        };
        let shapes = self.doc.object(o).map(|ob| ob.members.clone()).unwrap_or_default();
        self.abort_gestures();
        let mut id = None;
        let label = if cut { "Project cut" } else { "Project" };
        self.edit(label, |d| id = d.add_project(curve, cut, shapes));
        let id = id?;
        self.set_sel_mode(SelMode::Object);
        self.selection = vec![id];
        let on = self.doc.object(o).map(|ob| ob.name.clone()).unwrap_or_default();
        let what = if cut { format!("{} cuts {on} through", c.name) } else { format!("{} drawn on {on}: Face mode to pick a piece", c.name) };
        self.toast(&what, false);
        Some(id)
    }

    /// A Project curve step's settings changed (one undo step); its name
    /// follows when it is still the one it was given.
    pub fn set_project(&mut self, id: FeatureId, p: Project) {
        let Some(f) = self.doc.feature(id) else { return };
        let old = f.project.as_ref().map(|q| q.label()).unwrap_or("Project");
        let auto = f.name == old || f.name.strip_prefix(old).is_some_and(|r| r.trim().parse::<u32>().is_ok());
        let name = if auto && old != p.label() { Some(self.doc.fresh_name(p.label())) } else { None };
        self.edit(p.label(), |d| {
            if let Some(f) = d.feature_mut(id) {
                f.project = Some(p);
                if let Some(n) = name {
                    f.name = n;
                }
            }
        });
    }

    /// The Project steps made from a curve.
    pub fn projections_of(&self, curve: FeatureId) -> Vec<FeatureId> {
        self.doc.features.iter().filter(|f| f.project.as_ref().is_some_and(|p| p.curve == curve)).map(|f| f.id).collect()
    }

    /// The Object a Project step acts in (its element: that Object's first
    /// shape).
    pub fn project_owner(&self, p: &Project) -> Option<FeatureId> {
        let o = p.shapes.iter().find_map(|m| self.doc.object_of(*m))?;
        self.doc.object(o).and_then(|ob| ob.members.first().copied())
    }
}
