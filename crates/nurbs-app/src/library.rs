//! The Library (Romain 24 nov., 8 h 21: "Comme dans la dernière version de
//! Plasticity permet de sauvegarder de petits samples et ensuite les placer
//! en interactif sur les surfaces comme des boulons"): shapes with their
//! operations saved as a sample (Documents/Forme NURBS/library/*.json, in
//! a frame of their own: their base's middle at the origin, up +Y), shown
//! in the Library island with a picture made from their faces, then placed
//! with a click on any face: its normal is the sample's up, a point of
//! interest near the pointer (a hole's centre, a face's middle) catches it,
//! [ and ] turn it by 15°. The copy goes in the Object of the face it is
//! put on, so a Sub in the sample cuts it; an operation that named the face
//! the sample sat on (a fillet at a boss's foot) names that face.

use crate::snap::SnapKind;
use crate::state::*;
use glam::{DMat4, DQuat, DVec3, Vec3};
use nurbs_kernel::{FaceTag, Feature, FeatureId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The feature an operation of a sample names when it named a face out of
/// the sample (the face it sat on): replaced by the face it is put on.
pub const HOST: FeatureId = 0xFFFF_FFF0;

/// A sample: its features in its own frame, and its faces' triangles and
/// edges for its picture and the preview while it is placed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    pub name: String,
    pub features: Vec<Feature>,
    #[serde(default)]
    pub tris: Vec<[[f32; 3]; 3]>,
    #[serde(default)]
    pub edges: Vec<Vec<[f32; 3]>>,
    /// Where it was read from (None: one that comes with the app).
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

impl Sample {
    pub fn builtin(&self) -> bool {
        self.path.is_none()
    }
    /// Its triangles' box (its frame).
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for t in &self.tris {
            for p in t {
                lo = lo.min(Vec3::from_array(*p));
                hi = hi.max(Vec3::from_array(*p));
            }
        }
        if lo.x > hi.x {
            (Vec3::splat(-5.0), Vec3::splat(5.0))
        } else {
            (lo, hi)
        }
    }
    /// How many shapes and operations.
    pub fn counts(&self) -> (usize, usize) {
        let s = self.features.iter().filter(|f| f.is_solid()).count();
        (s, self.features.len() - s)
    }
}

/// The samples that come with the app (made by the capture harness's
/// `library` scenario from real parts, then kept in the repository).
const BUILTIN: &[&str] = &[
    include_str!("../assets/library/Socket bolt.json"),
    include_str!("../assets/library/Rivet.json"),
    include_str!("../assets/library/Vent.json"),
    include_str!("../assets/library/Boss.json"),
];

#[derive(Default)]
pub struct Library {
    pub samples: Vec<Sample>,
    pub loaded: bool,
    /// The island shown (the rail's LIBRARY card).
    pub open: bool,
}

pub fn library_dir() -> PathBuf {
    let d = crate::files::documents_dir().join("library");
    let _ = std::fs::create_dir_all(&d);
    d
}

impl Library {
    /// The samples: those that come with the app, then the saved ones by
    /// name.
    pub fn load(&mut self) {
        self.samples.clear();
        for s in BUILTIN {
            if let Ok(x) = serde_json::from_str::<Sample>(s) {
                self.samples.push(x);
            }
        }
        let mut own: Vec<Sample> = vec![];
        if let Ok(rd) = std::fs::read_dir(library_dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map(|x| x == "json").unwrap_or(false) {
                    if let Some(mut x) = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Sample>(&t).ok()) {
                        x.path = Some(p);
                        own.push(x);
                    }
                }
            }
        }
        own.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        self.samples.extend(own);
        self.loaded = true;
    }
    pub fn ensure(&mut self) {
        if !self.loaded {
            self.load();
        }
    }
}

/// Where a sample would go: the point, the face's normal (its up), its x
/// axis (turned by the angle), the face's tag (None on the ground), and
/// the point of interest that caught it.
#[derive(Clone, Copy, Debug)]
pub struct Spot {
    pub p: DVec3,
    pub n: DVec3,
    pub x: DVec3,
    pub host: Option<i64>,
    pub snapped: Option<SnapKind>,
}

impl Spot {
    /// The sample's frame there.
    pub fn matrix(&self) -> DMat4 {
        let y = self.n.normalize_or(DVec3::Y);
        let x = (self.x - y * self.x.dot(y)).normalize_or(y.any_orthonormal_vector());
        let z = x.cross(y);
        DMat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), self.p.extend(1.0))
    }
}

/// The Library's place tool under way: which sample, its turn.
#[derive(Clone, Debug)]
pub struct Placing {
    pub sample: usize,
    pub angle: f64,
    pub spot: Option<Spot>,
    /// Placed since it was armed.
    pub placed: usize,
    /// Its size (1: as saved); Shift held and the pointer moved scales it.
    pub scale: f64,
    /// Ctrl or Shift held (Romain 24 nov., 9 h 36: "CTRL maintenu : free
    /// rotate, SHIFT maintenu : free scale"): the spot stays where it was,
    /// the pointer turns it round its normal or scales it.
    pub hold: Option<PlaceHold>,
    /// The copies just placed, drawn until the kernel's model has them
    /// (the click showed nothing for a moment): the document version, the
    /// sample, its frame.
    pub ghosts: Vec<(u64, usize, DMat4)>,
}

/// Where Ctrl / Shift began to be held while placing.
#[derive(Clone, Copy, Debug)]
pub struct PlaceHold {
    pub spot: Spot,
    pub start: [f32; 2],
    pub angle0: f64,
    pub scale0: f64,
}

impl AppState {
    /// The Library island on or off (its samples read the first time).
    pub fn toggle_library(&mut self) {
        self.library.ensure();
        self.library.open = !self.library.open;
    }

    /// Saves the selected shapes (or Objects) with their operations as a
    /// sample. Returns its index.
    pub fn save_sample(&mut self, name: Option<&str>) -> Option<usize> {
        self.library.ensure();
        let mut els: Vec<FeatureId> = vec![];
        for id in &self.selection {
            let el = self.element_of(*id);
            if self.doc.feature(el).map(|f| f.is_solid()).unwrap_or(false) && !els.contains(&el) {
                els.push(el);
            }
        }
        let els = self.doc.with_operands(&els);
        if els.is_empty() {
            self.toast("Library: select the shapes to save first (Object mode)", true);
            return None;
        }
        let mut ids: Vec<FeatureId> = els.clone();
        for el in &els {
            for o in self.element_ops(*el) {
                let ok = self.doc.feature(o).map(|f| f.mirror.is_none() && f.array.is_none() && f.shell.is_none()).unwrap_or(false);
                if ok && !ids.contains(&o) {
                    ids.push(o);
                }
            }
        }
        let feats: Vec<Feature> = self.doc.features.iter().filter(|f| ids.contains(&f.id)).cloned().collect();
        // its base: the middle of its box, at the bottom of what it adds
        // (else at the top of what it cuts: a vent, a hole)
        let shapes: Vec<&Feature> = feats.iter().filter(|f| f.is_solid()).collect();
        let bx = |fs: &[&Feature]| fs.iter().map(|f| f.bounds()).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
        let (lo, hi) = bx(&shapes)?;
        let adds: Vec<&Feature> = shapes.iter().copied().filter(|f| f.op == nurbs_kernel::Op::Add).collect();
        let y0 = match bx(&adds) {
            Some((l, _)) => l.y,
            None => hi.y,
        };
        let base = DVec3::new((lo.x + hi.x) * 0.5, y0, (lo.z + hi.z) * 0.5);
        let to = DMat4::from_translation(-base);
        let host = |t: i64| match FaceTag::unpack(t) {
            Some(ft) if !ids.contains(&ft.feature) => FaceTag { feature: HOST, sub: 0 }.pack(),
            _ => t,
        };
        let features: Vec<Feature> = feats
            .iter()
            .map(|f| {
                if f.is_solid() {
                    f.moved_by(to)
                } else {
                    let mut g = f.carried_by(&|q| q - base);
                    g.map_tags(&host);
                    g.suspended = false;
                    g
                }
            })
            .collect();
        // its picture: its faces' triangles and edges as built now
        let (mut tris, mut edges) = (vec![], vec![]);
        if let Some(m) = &self.model {
            let mine = |face: i32| face >= 0 && m.faces.get(face as usize).and_then(|f| f.tag).map(|t| ids.contains(&t.feature)).unwrap_or(false);
            let off = base.as_vec3();
            // a thousandth of a mm is enough for a picture (a file 3 times smaller)
            let r3 = |v: Vec3| v.to_array().map(|x| (x * 1000.0).round() / 1000.0);
            for (k, t) in m.triangles.iter().enumerate() {
                if mine(m.tri_face[k] as i32) {
                    tris.push(t.map(|i| r3(Vec3::from_array(m.positions[i as usize]) - off)));
                }
            }
            for e in &m.edges {
                if !e.seam && (mine(e.faces[0]) || mine(e.faces[1])) {
                    edges.push(e.pts.iter().map(|p| r3(Vec3::from_array(*p) - off)).collect());
                }
            }
        }
        let first = shapes.first().map(|f| f.name.clone()).unwrap_or_else(|| "Sample".into());
        let base_name = name.map(|s| s.to_string()).or_else(|| {
            // a whole Object: its name
            let o = self.doc.object_of(els[0])?;
            let ob = self.doc.object(o)?;
            ob.members.iter().all(|m| els.contains(m)).then(|| ob.name.clone())
        });
        let base_name = base_name.unwrap_or(first);
        let taken = |n: &str| self.library.samples.iter().any(|s| s.name == n && !s.builtin());
        let name = if taken(&base_name) { (2..).map(|i| format!("{base_name} {i}")).find(|n| !taken(n)).unwrap() } else { base_name };
        let path = library_dir().join(format!("{}.json", crate::files::sanitize(&name)));
        let mut s = Sample { name: name.clone(), features, tris, edges, path: Some(path.clone()) };
        match serde_json::to_string(&s) {
            Ok(txt) => {
                if let Err(e) = std::fs::write(&path, txt) {
                    self.toast(&format!("Library: cannot write {}: {e}", path.display()), true);
                    return None;
                }
            }
            Err(e) => {
                self.toast(&format!("Library: {e}"), true);
                return None;
            }
        }
        s.path = Some(path);
        self.library.samples.push(s);
        self.library.open = true;
        let (ns, no) = (els.len(), ids.len() - els.len());
        self.toast(&format!("“{name}” saved to the Library ({ns} shape{}, {no} operation{})", if ns == 1 { "" } else { "s" }, if no == 1 { "" } else { "s" }), false);
        Some(self.library.samples.len() - 1)
    }

    /// A saved sample deleted (its file too); those that come with the app
    /// stay.
    pub fn delete_sample(&mut self, i: usize) {
        let Some(s) = self.library.samples.get(i) else { return };
        let Some(p) = s.path.clone() else {
            self.toast("This sample comes with the app", false);
            return;
        };
        let name = s.name.clone();
        let _ = std::fs::remove_file(&p);
        self.library.samples.remove(i);
        if self.place.as_ref().map(|pl| pl.sample == i).unwrap_or(false) {
            self.select_tool();
        }
        self.toast(&format!("“{name}” taken out of the Library"), false);
    }

    /// Arms the place tool with sample `i`.
    pub fn arm_place(&mut self, i: usize) {
        self.library.ensure();
        if i >= self.library.samples.len() {
            return;
        }
        self.abort_gestures();
        self.tool = Tool::Place;
        self.place = Some(Placing { sample: i, angle: 0.0, spot: None, placed: 0, scale: 1.0, hold: None, ghosts: vec![] });
    }

    /// Where the sample would go for the pointer at `p`.
    pub fn place_spot(&self, p: [f32; 2]) -> Option<Spot> {
        let angle = self.place.as_ref()?.angle;
        let (ro, _) = self.ray(p);
        let turn = |n: DVec3| -> DVec3 {
            // its x along the world's X laid on the face (Z when the face
            // looks along X), turned round the normal
            let r = if n.x.abs() > 0.9 { DVec3::Z } else { DVec3::X };
            let x = (r - n * r.dot(n)).normalize_or(n.any_orthonormal_vector());
            DQuat::from_axis_angle(n, angle.to_radians()) * x
        };
        let face_n = |face: usize, at: DVec3| -> DVec3 {
            let n = self.normal_near(face, at).unwrap_or(DVec3::Y).normalize_or(DVec3::Y);
            if n.dot(ro - at) < 0.0 {
                -n
            } else {
                n
            }
        };
        let tag_of = |face: usize| self.model.as_ref().and_then(|m| m.faces.get(face)).and_then(|f| f.tag).map(|t| t.pack());
        let under = self.pick_at(p);
        if let Some(s) = self.pick_snap(p, &[]).filter(|s| matches!(s.kind, SnapKind::Center | SnapKind::FaceCenter | SnapKind::Mid)) {
            let face = self.snap_face(s, under.map(|h| h.face));
            if let Some(f) = face {
                let n = face_n(f, s.pos);
                return Some(Spot { p: s.pos, n, x: turn(n), host: tag_of(f), snapped: Some(s.kind) });
            }
        }
        if let Some(h) = under {
            let at = h.point.as_dvec3();
            let n = face_n(h.face, at);
            return Some(Spot { p: at, n, x: turn(n), host: tag_of(h.face), snapped: None });
        }
        let g = BasePlane::ground();
        let (ro, rd) = self.ray(p);
        g.hit(ro, rd).map(|q| Spot { p: q, n: DVec3::Y, x: turn(DVec3::Y), host: None, snapped: None })
    }

    /// The pointer moved with the place tool; Ctrl held turns the sample
    /// round its normal where it is, Shift held scales it there.
    pub fn place_hover(&mut self, p: Option<[f32; 2]>, ctrl: bool, shift: bool) {
        let version = self.model.as_ref().map(|m| m.serial / 4).unwrap_or(0);
        let fine = self.model.as_ref().is_some_and(|m| m.fine);
        if let Some(pl) = self.place.as_mut() {
            // a ghost goes once the kernel's fine model holds its copy
            pl.ghosts.retain(|g| !(fine && version >= g.0));
        }
        let Some(pl) = self.place.clone() else { return };
        if (ctrl || shift) && p.is_some() {
            let p = p.unwrap();
            let hold = match pl.hold {
                Some(h) => h,
                None => {
                    let Some(spot) = pl.spot.or_else(|| self.place_spot(p)) else { return };
                    PlaceHold { spot, start: p, angle0: pl.angle, scale0: pl.scale }
                }
            };
            let v = self.view();
            let Some(c) = v.project(hold.spot.p.as_vec3()) else { return };
            let (a, b) = ([hold.start[0] - c[0], hold.start[1] - c[1]], [p[0] - c[0], p[1] - c[1]]);
            let (mut angle, mut scale) = (pl.angle, pl.scale);
            if ctrl {
                // the angle the pointer went round the spot (on screen: the
                // normal facing the eye turns as the pointer does)
                let d = (b[1].atan2(b[0]) - a[1].atan2(a[0])) as f64;
                let facing = if hold.spot.n.dot(v.eye.as_dvec3() - hold.spot.p) >= 0.0 { -1.0 } else { 1.0 };
                angle = (hold.angle0 + d.to_degrees() * facing).rem_euclid(360.0);
            }
            if shift {
                // twice as big per 250 points away from the spot, half as
                // big per 250 towards it (Romain 24 nov., 10 h 21: the ratio
                // of distances went ×10 at once near the spot)
                let (la, lb) = ((a[0] * a[0] + a[1] * a[1]).sqrt(), (b[0] * b[0] + b[1] * b[1]).sqrt());
                scale = (hold.scale0 * 2f64.powf(((lb - la) / 250.0) as f64)).clamp(0.05, 20.0);
                if self.settings.snap {
                    scale = (scale * 20.0).round() / 20.0;
                }
            }
            let mut spot = hold.spot;
            let turn = DQuat::from_axis_angle(spot.n, (angle - hold.angle0).to_radians());
            spot.x = turn * hold.spot.x;
            if let Some(pl) = self.place.as_mut() {
                pl.hold = Some(hold);
                pl.angle = angle;
                pl.scale = scale;
                pl.spot = Some(spot);
            }
            let deg = (angle + 180.0).rem_euclid(360.0) - 180.0;
            self.gizmo_readout = Some(format!("{deg:.0}° · ×{scale:.2}"));
            return;
        }
        let spot = p.and_then(|p| self.place_spot(p));
        if let Some(pl) = self.place.as_mut() {
            if pl.hold.take().is_some() {
                self.gizmo_readout = None;
            }
            pl.spot = spot;
        }
    }

    /// Turns the sample being placed by `deg` (15° a press of [ or ]).
    pub fn turn_place(&mut self, deg: f64) {
        if let Some(pl) = self.place.as_mut() {
            pl.angle = (pl.angle + deg).rem_euclid(360.0);
        }
    }

    /// A click with the place tool: a copy of the sample there (one undo
    /// step), in the Object of the face under it; the tool stays for the
    /// next one.
    pub fn place_click(&mut self, p: [f32; 2]) -> Vec<FeatureId> {
        // Ctrl / Shift held: where the sample is held, as it shows
        let held = self.place.as_ref().filter(|pl| pl.hold.is_some()).and_then(|pl| pl.spot);
        let Some(spot) = held.or_else(|| self.place_spot(p)) else { return vec![] };
        self.place_at(spot)
    }

    /// The sample put at `spot`: its features copied with fresh ids, moved
    /// into the spot's frame, the face it sat on now the spot's face.
    pub fn place_at(&mut self, spot: Spot) -> Vec<FeatureId> {
        let Some(pl) = self.place.clone() else { return vec![] };
        let Some(s) = self.library.samples.get(pl.sample).cloned() else { return vec![] };
        let k = pl.scale;
        // the same sample again exactly where the last one went: refused
        // (one inside the other loses its fillets)
        let m0 = spot.matrix() * DMat4::from_scale(DVec3::splat(k));
        if pl.ghosts.last().is_some_and(|(_, i, m)| *i == pl.sample && m.abs_diff_eq(m0, 1e-6)) || self.last_place.is_some_and(|(i, m)| i == pl.sample && m.abs_diff_eq(m0, 1e-6)) {
            self.toast(&format!("A {} is there already", s.name), false);
            return vec![];
        }
        self.last_place = Some((pl.sample, m0));
        let m = spot.matrix() * DMat4::from_scale(DVec3::splat(k));
        let host_obj = spot.host.and_then(FaceTag::unpack).map(|t| self.element_of(t.feature)).and_then(|el| self.doc.object_of(el));
        let mut new_shapes: Vec<FeatureId> = vec![];
        let host = spot.host;
        let mut dropped = 0;
        self.edit(&format!("Place {}", s.name), |d| {
            let mut map: std::collections::HashMap<FeatureId, FeatureId> = Default::default();
            for f in &s.features {
                map.insert(f.id, d.next_id);
                d.next_id += 1;
            }
            for f in &s.features {
                // its size first (in its own frame), then into the spot's
                let mut g = if f.is_solid() {
                    let f = if (k - 1.0).abs() > 1e-9 {
                        let mut g = crate::state::scaled(f, DVec3::ZERO, DQuat::IDENTITY, DVec3::splat(k));
                        g.origin = f.origin * k;
                        g
                    } else {
                        f.clone()
                    };
                    f.moved_by(spot.matrix())
                } else {
                    let mut g = f.carried_by(&|q| m.transform_point3(q));
                    if let Some(r) = g.round.as_mut() {
                        r.size *= k;
                    }
                    if let Some(p) = g.push.as_mut() {
                        p.inset *= k;
                        p.depth *= k;
                    }
                    g
                };
                // its own features first (the face it is put on may carry an
                // id the sample used too)
                crate::state::retag(&mut g, &map);
                // the face it sat on: the face under the pointer (on the
                // ground there is none: that operation is left out)
                let names_host = std::cell::Cell::new(false);
                g.map_tags(&|t| match FaceTag::unpack(t) {
                    Some(ft) if ft.feature == HOST => {
                        names_host.set(true);
                        host.unwrap_or(t)
                    }
                    _ => t,
                });
                if names_host.get() && host.is_none() {
                    dropped += 1;
                    continue;
                }
                g.id = map[&f.id];
                if let Some(b) = g.shape_bool.as_mut() {
                    b.operands.iter_mut().for_each(|o| *o = map.get(o).copied().unwrap_or(*o));
                }
                if let Some(l) = g.loft.as_mut() {
                    l.sections.iter_mut().for_each(|o| *o = map.get(o).copied().unwrap_or(*o));
                }
                let base = f.name.trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ').to_string();
                g.name = d.fresh_name(if base.is_empty() { f.kind_label() } else { &base });
                if g.is_solid() {
                    new_shapes.push(g.id);
                }
                d.features.push(g);
            }
            match host_obj.and_then(|o| d.object_mut(o)) {
                Some(o) => o.members.extend(new_shapes.iter().copied()),
                None => {
                    if !new_shapes.is_empty() {
                        let id = d.next_id;
                        d.next_id += 1;
                        let name = d.fresh_object_name(&s.name);
                        d.objects.push(nurbs_kernel::Object::new(id, name, new_shapes.clone()));
                    }
                }
            }
        });
        let version = self.version;
        if let Some(pl) = self.place.as_mut() {
            pl.placed += 1;
            pl.ghosts.push((version, pl.sample, m));
        }
        if dropped > 0 {
            self.toast(&format!("{dropped} operation(s) of “{}” named the face it sat on: left out on the ground", s.name), false);
        }
        new_shapes
    }

    /// The sample's triangles and edges where it would go (the preview).
    pub fn place_preview(&self) -> Option<(Vec<[Vec3; 3]>, Vec<Vec<Vec3>>, Spot)> {
        let pl = self.place.as_ref()?;
        let spot = pl.spot?;
        let s = self.library.samples.get(pl.sample)?;
        let m = (spot.matrix() * DMat4::from_scale(DVec3::splat(pl.scale))).as_mat4();
        let t = |p: &[f32; 3]| m.transform_point3(Vec3::from_array(*p));
        let tris = s.tris.iter().map(|q| [t(&q[0]), t(&q[1]), t(&q[2])]).collect();
        let edges = s.edges.iter().map(|e| e.iter().map(t).collect()).collect();
        Some((tris, edges, spot))
    }

    /// The copies just placed that the kernel has not shown yet.
    pub fn place_ghosts(&self) -> Vec<(Vec<[Vec3; 3]>, Vec<Vec<Vec3>>)> {
        let Some(pl) = self.place.as_ref() else { return vec![] };
        pl.ghosts
            .iter()
            .filter_map(|(_, i, m)| {
                let s = self.library.samples.get(*i)?;
                let m = m.as_mat4();
                let t = |p: &[f32; 3]| m.transform_point3(Vec3::from_array(*p));
                Some((s.tris.iter().map(|q| [t(&q[0]), t(&q[1]), t(&q[2])]).collect(), s.edges.iter().map(|e| e.iter().map(t).collect()).collect()))
            })
            .collect()
    }
}
