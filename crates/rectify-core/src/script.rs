//! Stage 3's edits: what the AI pass (Claude Desktop, through the MCP
//! server) and the hand tools change on top of the algorithm's regions.
//! A list of commands, saved next to the mesh as `<name>.rectify.json` and
//! replayed in order on the regions found by the algorithm:
//! - on the regions, before Regularize: merge, retype, free, paint, place;
//! - on the shapes, after Regularize: same radius, coaxial, coplanar;
//! - for the round pieces: revolve (add one), no revolve (drop one);
//! - for the clean-up: keep (a zone left as the AI mesh).
//!
//! A region is named by a point on it (`at`), its number (`id`) only when no
//! point is given: numbers change when the regions are found again, points
//! do not.

use crate::edit;
use crate::mesh::{Mesh, Topo};
use crate::prim::{Kind, Prim};
use crate::revolve::Hint;
use crate::segment::{SegParams, Segmentation};
use crate::NONE;
use glam::DVec3;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Ref {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 3]>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Cmd {
    /// The regions become one (refitted; `kind` forces its shape).
    Merge {
        regions: Vec<Ref>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
    },
    /// Another shape for a region: plane, cylinder, sphere, or free.
    Retype { region: Ref, kind: String },
    /// The regions are left free (the snap does not touch them).
    Free { regions: Vec<Ref> },
    /// Cylinders (or spheres) given one radius (their mean when none).
    SameRadius {
        regions: Vec<Ref>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f64>,
    },
    /// Cylinders put on one axis.
    Coaxial { regions: Vec<Ref> },
    /// Planes made one plane.
    Coplanar { regions: Vec<Ref> },
    /// A round piece: axis through `at`, up to `radius`, `depth` each side.
    Revolve {
        at: [f64; 3],
        axis: [f64; 3],
        radius: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<f64>,
    },
    /// No round piece whose axis passes near `at`.
    NoRevolve { at: [f64; 3] },
    /// A zone left as the AI mesh (the clean-up takes the moves back).
    Keep { at: [f64; 3], radius: f64 },
    /// A brush stroke (by hand): merge, split or free on these faces.
    Paint {
        mode: String,
        faces: Vec<u32>,
        #[serde(default)]
        regions: Vec<u32>,
    },
    /// A shape painted by hand (Shape tool).
    Place {
        kind: String,
        faces: Vec<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<[f64; 3]>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    /// "ai" or "hand".
    #[serde(default)]
    pub by: String,
    /// Why (the AI pass says what it saw).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(flatten)]
    pub cmd: Cmd,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Script {
    #[serde(default)]
    pub mesh: String,
    #[serde(default)]
    pub edits: Vec<Entry>,
}

impl Script {
    pub fn load(path: &std::path::Path) -> Result<Script, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }
    pub fn count(&self, by: &str) -> usize {
        self.edits.iter().filter(|e| e.by == by).count()
    }
    /// The script's file for a mesh: `<name>.rectify.json` next to it.
    pub fn path_for(mesh: &std::path::Path) -> std::path::PathBuf {
        let stem = mesh.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        mesh.with_file_name(format!("{stem}.rectify.json"))
    }
}

pub fn parse_kind(s: &str) -> Result<Option<Kind>, String> {
    match s.to_ascii_lowercase().as_str() {
        "plane" => Ok(Some(Kind::Plane)),
        "cylinder" => Ok(Some(Kind::Cylinder)),
        "sphere" => Ok(Some(Kind::Sphere)),
        "free" | "none" => Ok(None),
        k => Err(format!("unknown kind \"{k}\" (plane, cylinder, sphere, free)")),
    }
}

/// A point on the region (the centre of its face nearest to its middle):
/// how a region is named in a script.
pub fn point_of(m: &Mesh, seg: &Segmentation, r: u32) -> Option<[f64; 3]> {
    let reg = seg.regions.get(r as usize)?;
    if reg.faces.is_empty() {
        return None;
    }
    let mut c = DVec3::ZERO;
    let mut w = 0.0;
    for &f in &reg.faces {
        let a = m.tri_area(f as usize);
        c += m.tri_centre(f as usize) * a;
        w += a;
    }
    let c = c / w.max(1e-12);
    let f = reg.faces.iter().min_by(|a, b| (m.tri_centre(**a as usize) - c).length_squared().partial_cmp(&(m.tri_centre(**b as usize) - c).length_squared()).unwrap())?;
    Some(m.tri_centre(*f as usize).to_array())
}

/// The region a reference names now (by its point first).
pub fn resolve(m: &Mesh, seg: &Segmentation, r: &Ref) -> Option<u32> {
    if let Some(at) = r.at {
        let at = DVec3::from_array(at);
        let best = (0..m.tris.len()).into_par_iter().filter(|&t| seg.label[t] != NONE).map(|t| ((m.tri_centre(t) - at).length_squared(), t)).min_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if let Some((d2, t)) = best {
            if d2.sqrt() < 1.0 {
                return Some(seg.label[t]);
            }
        }
    }
    let id = r.id?;
    seg.regions.get(id as usize).filter(|x| !x.faces.is_empty()).map(|_| id)
}

fn resolve_all(m: &Mesh, seg: &Segmentation, refs: &[Ref]) -> Result<Vec<u32>, String> {
    let mut out = vec![];
    for r in refs {
        match resolve(m, seg, r) {
            Some(x) => {
                if !out.contains(&x) {
                    out.push(x)
                }
            }
            None => return Err(format!("no region at {:?} (id {:?})", r.at, r.id)),
        }
    }
    Ok(out)
}

/// Gives region `r` another shape (fitted on its faces), or frees it.
pub fn retype(m: &Mesh, seg: &mut Segmentation, r: u32, kind: Option<Kind>, p: &SegParams) -> Result<String, String> {
    let Some(reg) = seg.regions.get_mut(r as usize) else { return Err("no such region".into()) };
    match kind {
        None => {
            for &f in &reg.faces {
                seg.label[f as usize] = NONE;
            }
            let n = reg.faces.len();
            reg.faces.clear();
            reg.area = 0.0;
            Ok(format!("region {r} freed ({n} faces)"))
        }
        Some(k) => {
            let s = edit::samples(m, &reg.faces);
            let q = edit::fit_best(&s, Some(k), None, p).or_else(|| match k {
                Kind::Plane => crate::prim::fit_plane(&s),
                Kind::Cylinder => crate::prim::fit_cylinder(&s, None, None),
                Kind::Sphere => crate::prim::fit_sphere(&s, None),
            });
            match q {
                Some(q) => {
                    reg.rms = q.rms(&s);
                    reg.prim = q;
                    Ok(format!("region {r} is now a {}", q.describe()))
                }
                None => Err(format!("region {r} is not a {}", k.label().to_lowercase())),
            }
        }
    }
}

/// Applies one region edit (merge, retype, free, paint, place). Other
/// commands are left for later stages (Ok(None)).
pub fn apply_region_edit(m: &Mesh, topo: &Topo, seg: &mut Segmentation, cmd: &Cmd, p: &SegParams) -> Result<Option<String>, String> {
    match cmd {
        Cmd::Merge { regions, kind } => {
            let rs = resolve_all(m, seg, regions)?;
            if rs.len() < 2 {
                return Err("merge needs two regions or more".into());
            }
            let r = edit::merge(m, seg, &rs, &HashSet::new(), p).ok_or("the regions could not be merged")?;
            if let Some(k) = kind {
                if let Some(k) = parse_kind(k)? {
                    retype(m, seg, r, Some(k), p)?;
                }
            }
            Ok(Some(format!("{} regions merged into region {r}: {}", rs.len(), seg.regions[r as usize].prim.describe())))
        }
        Cmd::Retype { region, kind } => {
            let r = resolve(m, seg, region).ok_or("no region there")?;
            retype(m, seg, r, parse_kind(kind)?, p).map(Some)
        }
        Cmd::Free { regions } => {
            let rs = resolve_all(m, seg, regions)?;
            for &r in &rs {
                retype(m, seg, r, None, p)?;
            }
            Ok(Some(format!("{} regions freed", rs.len())))
        }
        Cmd::Paint { mode, faces, regions } => {
            let set: HashSet<u32> = faces.iter().copied().filter(|&f| (f as usize) < m.tris.len()).collect();
            match mode.as_str() {
                "merge" => edit::merge(m, seg, regions, &set, p).map(|r| Some(format!("merged into region {r}"))).ok_or_else(|| "nothing to merge".to_string()),
                "split" => match edit::split(m, topo, seg, &set) {
                    0 => Err("nothing cut".into()),
                    n => Ok(Some(format!("cut in {} pieces", n + 1))),
                },
                "free" => Ok(Some(format!("{} faces freed", edit::erase(m, seg, &set)))),
                k => Err(format!("unknown brush \"{k}\"")),
            }
        }
        Cmd::Place { kind, faces, hint } => {
            let k = parse_kind(kind)?;
            edit::place(m, topo, seg, faces, k, hint.map(DVec3::from_array), p).map(|r| Some(format!("{} placed as region {r}", seg.regions[r as usize].prim.describe())))
        }
        _ => Ok(None),
    }
}

/// Applies one shape constraint (after Regularize).
pub fn apply_constraint(m: &Mesh, seg: &mut Segmentation, cmd: &Cmd) -> Result<Option<String>, String> {
    let weight = |seg: &Segmentation, r: u32| seg.regions[r as usize].area.max(1e-6);
    match cmd {
        Cmd::SameRadius { regions, radius } => {
            let rs = resolve_all(m, seg, regions)?;
            let mut sum = 0.0;
            let mut w = 0.0;
            for &r in &rs {
                match seg.regions[r as usize].prim {
                    Prim::Cylinder { r: x, .. } | Prim::Sphere { r: x, .. } => {
                        sum += x * weight(seg, r);
                        w += weight(seg, r);
                    }
                    _ => return Err(format!("region {r} is not round")),
                }
            }
            let target = radius.unwrap_or(sum / w.max(1e-12));
            for &r in &rs {
                let reg = &mut seg.regions[r as usize];
                match &mut reg.prim {
                    Prim::Cylinder { r: x, .. } | Prim::Sphere { r: x, .. } => *x = target,
                    _ => {}
                }
                reg.locked = true;
            }
            Ok(Some(format!("{} regions at radius {:.3} mm", rs.len(), target)))
        }
        Cmd::Coaxial { regions } => {
            let rs = resolve_all(m, seg, regions)?;
            let mut axis = DVec3::ZERO;
            let mut first: Option<DVec3> = None;
            let mut pts = vec![];
            for &r in &rs {
                let Prim::Cylinder { p, a, .. } = seg.regions[r as usize].prim else { return Err(format!("region {r} is not a cylinder")) };
                let f = *first.get_or_insert(a);
                let a = if a.dot(f) < 0.0 { -a } else { a };
                axis += a * weight(seg, r);
                pts.push((p, weight(seg, r)));
            }
            let axis = axis.normalize_or_zero();
            if axis == DVec3::ZERO {
                return Err("no axis".into());
            }
            // the common line: the mean of the axis points, square to it
            let w: f64 = pts.iter().map(|x| x.1).sum();
            let c = pts.iter().map(|(p, wi)| (*p - axis * axis.dot(*p)) * *wi).sum::<DVec3>() / w;
            for &r in &rs {
                if let Prim::Cylinder { p, a, .. } = &mut seg.regions[r as usize].prim {
                    *p = c;
                    *a = axis;
                }
                seg.regions[r as usize].locked = true;
            }
            Ok(Some(format!("{} cylinders on one axis", rs.len())))
        }
        Cmd::Coplanar { regions } => {
            let rs = resolve_all(m, seg, regions)?;
            let mut nsum = DVec3::ZERO;
            let mut first: Option<DVec3> = None;
            for &r in &rs {
                let Prim::Plane { n, .. } = seg.regions[r as usize].prim else { return Err(format!("region {r} is not a plane")) };
                let f = *first.get_or_insert(n);
                nsum += if n.dot(f) < 0.0 { -n } else { n } * weight(seg, r);
            }
            let n = nsum.normalize_or_zero();
            let (mut dsum, mut w) = (0.0, 0.0);
            for &r in &rs {
                for &f in &seg.regions[r as usize].faces {
                    let a = m.tri_area(f as usize);
                    dsum += n.dot(m.tri_centre(f as usize)) * a;
                    w += a;
                }
            }
            let d = dsum / w.max(1e-12);
            for &r in &rs {
                if let Prim::Plane { n: pn, d: pd } = &mut seg.regions[r as usize].prim {
                    let s = if pn.dot(n) < 0.0 { -1.0 } else { 1.0 };
                    *pn = n * s;
                    *pd = d * s;
                }
                seg.regions[r as usize].locked = true;
            }
            Ok(Some(format!("{} planes made one", rs.len())))
        }
        _ => Ok(None),
    }
}

/// The round pieces asked for, and the points where none is wanted.
pub fn revolve_hints(edits: &[Entry]) -> (Vec<Hint>, Vec<DVec3>) {
    let mut hints = vec![];
    let mut off = vec![];
    for e in edits {
        match &e.cmd {
            Cmd::Revolve { at, axis, radius, depth } => hints.push(Hint { at: DVec3::from_array(*at), axis: DVec3::from_array(*axis), radius: *radius, depth: depth.unwrap_or(*radius) }),
            Cmd::NoRevolve { at } => off.push(DVec3::from_array(*at)),
            _ => {}
        }
    }
    (hints, off)
}

pub fn keep_zones(edits: &[Entry]) -> Vec<(DVec3, f64)> {
    edits
        .iter()
        .filter_map(|e| match &e.cmd {
            Cmd::Keep { at, radius } => Some((DVec3::from_array(*at), *radius)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_round_trip() {
        let s = Script {
            mesh: "gun.obj".into(),
            edits: vec![
                Entry { by: "ai".into(), note: "the hose is one tube".into(), cmd: Cmd::Merge { regions: vec![Ref { id: Some(3), at: Some([1.0, 2.0, 3.0]) }, Ref { id: Some(4), at: None }], kind: Some("cylinder".into()) } },
                Entry { by: "hand".into(), note: String::new(), cmd: Cmd::Keep { at: [0.0, 0.0, 0.0], radius: 2.0 } },
            ],
        };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"op\":\"merge\""), "{text}");
        let back: Script = serde_json::from_str(&text).unwrap();
        assert_eq!(back, s);
        let one: Entry = serde_json::from_str(r#"{"op":"same_radius","regions":[{"id":7}],"note":"buttons"}"#).unwrap();
        assert!(matches!(one.cmd, Cmd::SameRadius { .. }));
    }
}
