//! Mirror (the tool proposed to Romain on 20 nov., 15 h 42, when he asked
//! for one that was missing): a step that makes what is built before it
//! symmetric across a plane square to a world axis; every step after it is
//! made on both sides by the kernel (`build::mirrored_doc`, `twin_tool`).
//! A click on either side names the same shape or face: both carry the
//! same tags.

use crate::state::AppState;
use glam::DVec3;
use nurbs_kernel::{Feature, FeatureId, Mirror};

impl AppState {
    /// The selected Mirror step.
    pub fn selected_mirror(&self) -> Option<&Feature> {
        self.single().filter(|f| f.mirror.is_some())
    }

    /// Adds a Mirror at the end (Alt + M, the icon by Align, the marking
    /// menu on empty space), across the world's `axis` through the origin,
    /// keeping the side most of the part is on; it is selected.
    pub fn add_mirror(&mut self, axis: u8) -> Option<FeatureId> {
        if self.doc.features.iter().all(|f| !f.visible || f.is_path()) {
            self.toast("Nothing to mirror yet", true);
            return None;
        }
        self.abort_gestures();
        let a = axis.min(2) as usize;
        let flip = self.model.as_ref().and_then(|m| m.bounds()).map(|(lo, hi)| (lo[a] + hi[a]) < -1e-3).unwrap_or(false);
        let m = Mirror { axis: axis.min(2), offset: 0.0, flip };
        let mut id = None;
        self.edit("Mirror", |d| id = Some(d.add_mirror(m)));
        if let Some(id) = id {
            self.selection = vec![id];
            self.toast(&format!("Mirror across {}: the steps after it are made on both sides", m.axis_name()), false);
        }
        id
    }

    /// Sets the selected Mirror (live while a slider is dragged: `start`
    /// begins a new undo step).
    pub fn set_mirror(&mut self, m: Mirror, start: bool) {
        let Some(id) = self.selected_mirror().map(|f| f.id) else { return };
        if start {
            if self.is_live() {
                self.end_live();
            }
            self.begin_live("Mirror");
        } else if !self.is_live() {
            self.begin_live("Mirror");
        }
        self.live(|d| {
            if let Some(f) = d.feature_mut(id) {
                f.mirror = Some(m);
                f.origin = m.normal() * m.offset;
            }
        });
    }

    /// The plane of the selected Mirror to draw: its corners round the part
    /// (a little past it) and the Mirror.
    pub fn mirror_plane(&self) -> Option<(Mirror, [DVec3; 4])> {
        let m = self.selected_mirror()?.mirror?;
        let (lo, hi) = self.model.as_ref().and_then(|md| md.bounds()).map(|(lo, hi)| (DVec3::from_array(lo.map(|x| x as f64)), DVec3::from_array(hi.map(|x| x as f64)))).unwrap_or((DVec3::splat(-20.0), DVec3::splat(20.0)));
        let pad = (hi - lo).max_element().max(10.0) * 0.12;
        let (lo, hi) = (lo - DVec3::splat(pad), hi + DVec3::splat(pad));
        let a = m.axis.min(2) as usize;
        let (u, v) = ((a + 1) % 3, (a + 2) % 3);
        let corner = |su: bool, sv: bool| {
            let mut p = DVec3::ZERO;
            p[a] = m.offset;
            p[u] = if su { hi[u] } else { lo[u] };
            p[v] = if sv { hi[v] } else { lo[v] };
            p
        };
        Some((m, [corner(false, false), corner(true, false), corner(true, true), corner(false, true)]))
    }
}
