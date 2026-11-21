//! Levels on the part's faces (the "scan → shapes" button, the detail:
//! Romain, 19 nov. 2026, 20 h 14: "comment pousser le détail au maximum
//! pour avoir une réplique exacte du scan ?"): where a flat face of the part
//! lies on the scan, the scan's own flat levels on it (a recessed panel, a
//! raised plate, a pocket, a boss) read as steps to add or carve.
//!
//! Seen along an axis, the scan and the part are drawn into depth maps (the
//! nearest surface from that side). On each flat face of the part square to
//! the axis, the scan's flat plateaus seed a watershed of the scan's slope
//! kept inside that face; each region takes its plateau's level (the
//! median: the scan's waves are not followed). A region within `STEP_MIN`
//! of the face's level is the face; one too small, or not parted from its
//! neighbour by a wall, goes into that neighbour; touching regions of one
//! level are one. What is left are steps, each from the level of the
//! neighbour it shares most border with (the face's, or the step it lies
//! in: a boss in a pocket) to its own, its outline where the scan's wall is
//! halfway between the two, made a hard-surface shape: a circle, a rounded
//! rectangle (a slot when its corners take half its width), else straight
//! sides with their corners.

use crate::mesh::Mesh;
use crate::primary::{self, Grid};
use glam::{DVec2, DVec3};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// A step under this (mm) is the scan's noise.
pub const STEP_MIN: f64 = 0.3;
/// The scan further than this from the part's face (mm) is another surface.
const MAX_GAP: f64 = 8.0;
/// A face of the part within 3° of square to the axis is read.
const FACE_COS: f64 = 0.9986;
/// The maps' size: about this many steps along the part's longer side.
const STEPS: f64 = 640.0;
/// The mean slope (depth per mm) on a step's border with its base under
/// this: no wall there, a wave of the scan (14°).
const WALL_MIN: f64 = 0.25;
/// Plateaus: neighbours within this (mm), all within `SPAN` of the first.
const PLATEAU_STEP: f64 = 0.06;
const SPAN: f64 = 0.25;
const NONE: u32 = u32::MAX;
/// A step is a level when this share of its cells is read flat, or this
/// share lies within `NEAR` (mm) of its level (the scan's facets make a
/// level bumpy at the cells' scale); a cable over a face, a slope are not.
const MIN_FLAT: f64 = 0.3;
const MIN_NEAR: f64 = 0.4;
const NEAR: f64 = 0.25;

/// A step's outline in the view's plane (`Level::u`, `v`), mm.
#[derive(Clone, Debug)]
pub enum Outline {
    Round { c: DVec2, r: f64 },
    /// A rectangle along `x` (unit), its half sizes, its corners rounded
    /// at `r` (half its width: a slot).
    Rect { c: DVec2, x: DVec2, half: DVec2, r: f64 },
    /// Straight sides, counter-clockwise, each corner's cut (a sketch's).
    Poly { pts: Vec<DVec2>, cuts: Vec<f64> },
}

impl Outline {
    /// Its corners and their cuts (none for a round).
    pub fn polygon(&self) -> Option<(Vec<DVec2>, Vec<f64>)> {
        match self {
            Outline::Round { .. } => None,
            Outline::Rect { c, x, half, r } => {
                let y = x.perp();
                let pts = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].iter().map(|&(a, b)| *c + *x * (a * half.x) + y * (b * half.y)).collect();
                Some((pts, vec![*r; 4]))
            }
            Outline::Poly { pts, cuts } => Some((pts.clone(), cuts.clone())),
        }
    }
    pub fn centre(&self) -> DVec2 {
        match self {
            Outline::Round { c, .. } | Outline::Rect { c, .. } => *c,
            Outline::Poly { pts, .. } => {
                let (lo, hi) = pts.iter().fold((DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)), |(a, b), q| (a.min(*q), b.max(*q)));
                (lo + hi) * 0.5
            }
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Outline::Round { .. } => "round",
            Outline::Rect { half, r, .. } if *r >= half.x.min(half.y) * 0.98 => "slot",
            Outline::Rect { r, .. } if *r > 0.0 => "rounded rectangle",
            Outline::Rect { .. } => "rectangle",
            Outline::Poly { .. } => "outline",
        }
    }
}

/// A step read on a face of the part.
#[derive(Clone, Debug)]
pub struct Level {
    /// The face's outward normal (a world axis, either way) and the plane's
    /// axes the outline is in.
    pub axis: DVec3,
    pub u: DVec3,
    pub v: DVec3,
    pub outline: Outline,
    /// Along `axis` (mm): the level it starts from (the face's, or the step
    /// it lies in) and the scan's level it goes to (above: a plate or a
    /// boss; below: a pocket).
    pub base: f64,
    pub level: f64,
    /// The part's face it lies on (index in the part's faces, as given) and
    /// that face's level.
    pub face: u32,
    pub face_level: f64,
    /// Its area seen along the axis (mm²), the share of it read flat, the
    /// spread of its flat levels (mm, the middle half), the share of the
    /// face it covers (near 1: the face itself is off, move it instead).
    pub area: f64,
    pub flat: f64,
    /// The share of it within 0.25 mm of its level.
    pub near: f64,
    /// How deep it lies in other steps (0: on the face; 1: in a step on
    /// it…): the order to make them in.
    pub depth: usize,
    pub spread: f64,
    pub cover: f64,
    /// A point inside it, on the part's face.
    pub at: DVec3,
}

impl Level {
    pub fn raised(&self) -> bool {
        self.level > self.base
    }
    pub fn height(&self) -> f64 {
        (self.level - self.base).abs()
    }
    /// A point of the plane at `d` along the axis.
    pub fn point(&self, p: DVec2, d: f64) -> DVec3 {
        self.u * p.x + self.v * p.y + self.axis * d
    }
}

#[derive(Clone, Debug)]
pub struct Opts {
    /// The smallest step kept (mm²) and the smallest height (mm).
    pub min_area: f64,
    pub min_step: f64,
    /// The maps' step (mm); None: from the part's size.
    pub px: Option<f64>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { min_area: 4.0, min_step: STEP_MIN, px: None }
    }
}

/// The steps on the part's faces square to `axes` (world axes, either
/// way: +Z reads the faces facing +Z). `part_face`: each part triangle's
/// face; `grain`: the scan's median edge (mm). Biggest first per axis.
pub fn read(scan: &Mesh, part: &Mesh, part_face: &[u32], axes: &[DVec3], grain: f64, opts: &Opts) -> Vec<Level> {
    // the part's triangles' normals
    let normals: Vec<DVec3> = part
        .tris
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| part.pos[i as usize]);
            (b - a).cross(c - a).normalize_or_zero()
        })
        .collect();
    let mut by_axis: Vec<(DVec3, Vec<f64>)> = vec![];
    for a in axes {
        let a = a.normalize_or_zero();
        if a == DVec3::ZERO {
            continue;
        }
        let b = a.abs();
        let k = if b.x >= b.y && b.x >= b.z { 0 } else if b.y >= b.z { 1 } else { 2 };
        let w = [DVec3::X, DVec3::Y, DVec3::Z][k];
        let s = a.dot(w).signum();
        match by_axis.iter_mut().find(|(x, _)| *x == w) {
            Some((_, sides)) => {
                if !sides.contains(&s) {
                    sides.push(s)
                }
            }
            None => by_axis.push((w, vec![s])),
        }
    }
    let out: Vec<Vec<Level>> = by_axis.par_iter().map(|(w, sides)| read_axis(scan, part, part_face, &normals, *w, sides, grain, opts)).collect();
    out.into_iter().flatten().collect()
}

fn read_axis(scan: &Mesh, part: &Mesh, part_face: &[u32], normals: &[DVec3], w: DVec3, sides: &[f64], grain: f64, opts: &Opts) -> Vec<Level> {
    let (u, v) = primary::plane_axes(w);
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for q in &part.pos {
        let p = DVec2::new(q.dot(u), q.dot(v));
        lo = lo.min(p);
        hi = hi.max(p);
    }
    if !(hi.x > lo.x && hi.y > lo.y) {
        return vec![];
    }
    let ext = (hi - lo).max_element();
    let px = opts.px.unwrap_or((ext / STEPS).max(grain * 0.8)).max(0.02);
    let margin = (3.0 / px).ceil() as usize + 2;
    let g = Grid {
        w: ((hi.x - lo.x) / px).ceil() as usize + 2 * margin,
        h: ((hi.y - lo.y) / px).ceil() as usize + 2 * margin,
        px,
        u0: lo.x - margin as f64 * px,
        v0: lo.y - margin as f64 * px,
    };
    let (sf, sb) = primary::depth_maps(scan, &g, u, v, w);
    let n = g.w * g.h;
    let (mut pf, mut pb) = (vec![f64::NEG_INFINITY; n], vec![f64::INFINITY; n]);
    let (mut tf, mut tb) = (vec![NONE; n], vec![NONE; n]);
    primary::raster(part, &g, u, v, w, |i, z, t| {
        if z > pf[i] {
            pf[i] = z;
            tf[i] = t as u32;
        }
        if z < pb[i] {
            pb[i] = z;
            tb[i] = t as u32;
        }
    });
    let mut out = vec![];
    for &s in sides {
        let axis = w * s;
        let (scan_up, part_up, tri): (Vec<f64>, Vec<f64>, &[u32]) = if s > 0.0 {
            (primary::smooth(&sf, &g), pf.clone(), &tf)
        } else {
            (primary::smooth(&sb, &g).iter().map(|d| -d).collect(), pb.iter().map(|d| -d).collect(), &tb)
        };
        // the part's faces square to the axis, seen from that side
        let dom: Vec<u32> = (0..n)
            .map(|i| {
                let t = tri[i];
                if t == NONE || normals[t as usize].dot(axis) < FACE_COS || !scan_up[i].is_finite() || (scan_up[i] - part_up[i]).abs() > MAX_GAP {
                    NONE
                } else {
                    part_face.get(t as usize).copied().unwrap_or(0)
                }
            })
            .collect();
        let tag = format!("{}{}", if s > 0.0 { "p" } else { "m" }, ["x", "y", "z"][if w.x != 0.0 { 0 } else if w.y != 0.0 { 1 } else { 2 }]);
        let mut steps = read_side(&scan_up, &part_up, &dom, &g, opts, &tag);
        for st in steps.iter_mut() {
            st.axis = axis;
            st.u = u;
            st.v = v;
            st.at = st.point(st.outline.centre(), st.face_level);
        }
        out.extend(steps);
    }
    out
}

/// The scan's plateaus kept inside one face each (`dom`): flat cells,
/// neighbours on the same face within `PLATEAU_STEP`, all within `SPAN`.
fn plateaus_in(d: &[f64], flat: &[bool], dom: &[u32], g: &Grid) -> Vec<Vec<usize>> {
    let mut lab = vec![false; d.len()];
    let mut list = vec![];
    let mut stack = vec![];
    for s in 0..d.len() {
        if !flat[s] || lab[s] || dom[s] == NONE {
            continue;
        }
        let base = d[s];
        let mut cells = vec![s];
        lab[s] = true;
        stack.push(s);
        while let Some(a) = stack.pop() {
            for b in g.around(a) {
                if flat[b] && !lab[b] && dom[b] == dom[a] && (d[b] - d[a]).abs() < PLATEAU_STEP && (d[b] - base).abs() < SPAN {
                    lab[b] = true;
                    cells.push(b);
                    stack.push(b);
                }
            }
        }
        list.push(cells);
    }
    list
}

/// A watershed of `slope` from the seeds, each region kept on its face.
fn flood_in(slope: &[f64], seeds: &[u32], dom: &[u32], g: &Grid) -> Vec<u32> {
    let mut lab = seeds.to_vec();
    let mut heap = BinaryHeap::new();
    let mut n = 0u64;
    let key = |s: f64| (s.max(0.0).min(1e6) * 1e6) as u64;
    for i in 0..lab.len() {
        if lab[i] != 0 {
            heap.push(Reverse((key(slope[i]), n, i)));
            n += 1;
        }
    }
    while let Some(Reverse((_, _, a))) = heap.pop() {
        for b in g.around(a) {
            if lab[b] == 0 && dom[b] == dom[a] && slope[b].is_finite() {
                lab[b] = lab[a];
                heap.push(Reverse((key(slope[b]), n, b)));
                n += 1;
            }
        }
    }
    lab
}

#[derive(Clone, Debug)]
struct Reg {
    dom: u32,
    level: f64,
    cells: usize,
    /// merged into the face (it matches the part)
    face: bool,
}

/// One side: `scan` and `part` the depth maps turned to rise along the
/// side's axis, `dom` the part's face under each cell (or `NONE`).
fn read_side(scan: &[f64], part: &[f64], dom: &[u32], g: &Grid, opts: &Opts, tag: &str) -> Vec<Level> {
    let px = g.px;
    let cell = px * px;
    let (slope, flat) = primary::slopes(scan, g);
    let pl = plateaus_in(scan, &flat, dom, g);
    let seed_min = ((0.5 / cell).ceil() as usize).max(4);
    let min_cells = ((opts.min_area / cell).ceil() as usize).max(6);
    // each face's level and size
    let mut face_cells: HashMap<u32, Vec<f64>> = HashMap::new();
    for i in 0..dom.len() {
        if dom[i] != NONE {
            face_cells.entry(dom[i]).or_default().push(part[i]);
        }
    }
    let face_size: HashMap<u32, usize> = face_cells.iter().map(|(k, v)| (*k, v.len())).collect();
    let face_level: HashMap<u32, f64> = face_cells.into_iter().map(|(k, v)| (k, primary::median(v))).collect();

    let mut seeds = vec![0u32; scan.len()];
    let mut regs = vec![Reg { dom: NONE, level: 0.0, cells: 0, face: true }];
    let mut seed_cells: Vec<Vec<usize>> = vec![vec![]];
    for cells in pl {
        if cells.len() < seed_min {
            continue;
        }
        let id = regs.len() as u32;
        for &c in &cells {
            seeds[c] = id;
        }
        regs.push(Reg { dom: dom[cells[0]], level: primary::median(cells.iter().map(|&c| scan[c]).collect()), cells: 0, face: false });
        seed_cells.push(cells);
    }
    if regs.len() == 1 {
        return vec![];
    }
    let lab = flood_in(&slope, &seeds, dom, g);
    for &l in &lab {
        if l != 0 {
            regs[l as usize].cells += 1;
        }
    }
    let mut parent: Vec<u32> = (0..regs.len() as u32).collect();
    fn root(p: &mut [u32], mut a: u32) -> u32 {
        while p[a as usize] != a {
            p[a as usize] = p[p[a as usize] as usize];
            a = p[a as usize];
        }
        a
    }
    // regions merge until each left is a step parted from its base by a wall
    for _round in 0..200 {
        let roots: Vec<u32> = (0..regs.len() as u32).map(|r| root(&mut parent, r)).collect();
        // borders: (region, neighbour or 0 for the face / outside) → (cells, wall)
        let mut border: HashMap<(u32, u32), (usize, f64)> = HashMap::new();
        for y in 0..g.h {
            for x in 0..g.w {
                let i = y * g.w + x;
                for j in [(x + 1 < g.w).then(|| i + 1), (y + 1 < g.h).then(|| i + g.w)].into_iter().flatten() {
                    if dom[i] == NONE && dom[j] == NONE {
                        continue;
                    }
                    let side = |a: usize, b: usize| -> u32 {
                        let l = lab[b];
                        if dom[b] != dom[a] || l == 0 {
                            return 0;
                        }
                        let r = roots[l as usize];
                        if regs[r as usize].face {
                            0
                        } else {
                            r
                        }
                    };
                    let me = |a: usize| -> u32 {
                        let l = lab[a];
                        if dom[a] == NONE || l == 0 {
                            return 0;
                        }
                        let r = roots[l as usize];
                        if regs[r as usize].face {
                            0
                        } else {
                            r
                        }
                    };
                    let (a, b) = (me(i), side(i, j));
                    let (a2, b2) = (me(j), side(j, i));
                    let wall = slope[i].max(slope[j]).min(3.0);
                    if a != 0 && a != b {
                        let e = border.entry((a, b)).or_default();
                        e.0 += 1;
                        e.1 += wall;
                    }
                    if a2 != 0 && a2 != b2 {
                        let e = border.entry((a2, b2)).or_default();
                        e.0 += 1;
                        e.1 += wall;
                    }
                }
            }
        }
        let mut nb: HashMap<u32, Vec<(u32, usize, f64)>> = HashMap::new();
        for (&(a, b), &(c, wsum)) in &border {
            nb.entry(a).or_default().push((b, c, wsum / c as f64));
        }
        let alive: Vec<u32> = (1..regs.len() as u32).filter(|&r| roots[r as usize] == r && !regs[r as usize].face).collect();
        let mut order = alive.clone();
        order.sort_by_key(|&r| regs[r as usize].cells);
        let mut taken: Vec<bool> = vec![false; regs.len()];
        let mut changed = false;
        for r in order {
            if taken[r as usize] {
                continue;
            }
            let reg = regs[r as usize].clone();
            let fl = face_level.get(&reg.dom).copied().unwrap_or(reg.level);
            let list = nb.get(&r).cloned().unwrap_or_default();
            let Some(&(n, _, wall)) = list.iter().max_by_key(|x| x.1) else {
                // no border: an island of the map alone
                continue;
            };
            let nl = if n == 0 { fl } else { regs[n as usize].level };
            let touches_face = list.iter().any(|x| x.0 == 0);
            let into = if reg.cells < min_cells || (reg.level - nl).abs() < opts.min_step || wall < WALL_MIN {
                Some(n)
            } else if (reg.level - fl).abs() < opts.min_step && touches_face {
                Some(0)
            } else {
                None
            };
            let Some(n) = into else { continue };
            if n != 0 && taken[n as usize] {
                continue;
            }
            taken[r as usize] = true;
            changed = true;
            if n == 0 {
                regs[r as usize].face = true;
            } else {
                taken[n as usize] = true;
                parent[r as usize] = n;
                let c = regs[r as usize].cells;
                regs[n as usize].cells += c;
            }
        }
        if !changed {
            break;
        }
    }
    // steps of one level close together on one face are one (a panel's
    // floor cut through by slots, between them)
    {
        let roots: Vec<u32> = (0..regs.len() as u32).map(|r| root(&mut parent, r)).collect();
        let mut cells: HashMap<u32, Vec<usize>> = HashMap::new();
        for i in 0..lab.len() {
            if lab[i] != 0 && dom[i] != NONE {
                let r = roots[lab[i] as usize];
                if !regs[r as usize].face {
                    cells.entry(r).or_default().push(i);
                }
            }
        }
        let reach = (2.0 / px).ceil() as usize;
        let bbox = |c: &[usize]| {
            c.iter().fold((usize::MAX, usize::MAX, 0usize, 0usize), |(x0, y0, x1, y1), &i| (x0.min(i % g.w), y0.min(i / g.w), x1.max(i % g.w), y1.max(i / g.w)))
        };
        let keys: Vec<u32> = cells.keys().copied().collect();
        let boxes: HashMap<u32, (usize, usize, usize, usize)> = keys.iter().map(|k| (*k, bbox(&cells[k]))).collect();
        let mut pairs = vec![];
        for (ia, &a) in keys.iter().enumerate() {
            for &b in &keys[ia + 1..] {
                let (ra, rb) = (&regs[a as usize], &regs[b as usize]);
                if ra.dom != rb.dom || (ra.level - rb.level).abs() > 0.15 {
                    continue;
                }
                let (p, q) = (boxes[&a], boxes[&b]);
                let gap = |a0: usize, a1: usize, b0: usize, b1: usize| if b0 > a1 { b0 - a1 } else if a0 > b1 { a0 - b1 } else { 0 };
                if gap(p.0, p.2, q.0, q.2) > 2 * reach || gap(p.1, p.3, q.1, q.3) > 2 * reach {
                    continue;
                }
                // within 2 × reach cells of each other, over the face
                let target: std::collections::HashSet<usize> = cells[&b].iter().copied().collect();
                let mut dist: HashMap<usize, usize> = cells[&a].iter().map(|&i| (i, 0)).collect();
                let mut queue: std::collections::VecDeque<usize> = cells[&a].iter().copied().collect();
                let mut found = false;
                while let Some(c) = queue.pop_front() {
                    let d = dist[&c];
                    if d >= 2 * reach {
                        continue;
                    }
                    for nb in g.around(c) {
                        if dom[nb] != ra.dom || dist.contains_key(&nb) {
                            continue;
                        }
                        if target.contains(&nb) {
                            found = true;
                            break;
                        }
                        dist.insert(nb, d + 1);
                        queue.push_back(nb);
                    }
                    if found {
                        break;
                    }
                }
                if found {
                    pairs.push((a, b));
                }
            }
        }
        for (a, b) in pairs {
            let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
            if ra == rb {
                continue;
            }
            let (big, small) = if regs[ra as usize].cells >= regs[rb as usize].cells { (ra, rb) } else { (rb, ra) };
            parent[small as usize] = big;
            let c = regs[small as usize].cells;
            regs[big as usize].cells += c;
        }
    }
    let roots: Vec<u32> = (0..regs.len() as u32).map(|r| root(&mut parent, r)).collect();
    let is_step = |r: u32| r != 0 && !regs[roots[r as usize] as usize].face;

    // each step: its cells, its base (the neighbour with the longest border)
    let mut cells_of: HashMap<u32, Vec<usize>> = HashMap::new();
    for i in 0..lab.len() {
        if lab[i] != 0 && dom[i] != NONE && is_step(lab[i]) {
            cells_of.entry(roots[lab[i] as usize]).or_default().push(i);
        }
    }
    struct Info {
        base: f64,
        level: f64,
        fl: f64,
        dom: u32,
        keep: Vec<usize>,
        parent: Option<u32>,
    }
    // each step's neighbours (0: the face, or off it) and how long a border
    let mut nbs: HashMap<u32, HashMap<u32, usize>> = HashMap::new();
    for (&r, cells) in &cells_of {
        let d = regs[r as usize].dom;
        let nb = nbs.entry(r).or_default();
        for &i in cells {
            for j in g.around(i) {
                let o = if dom[j] != d || lab[j] == 0 || !is_step(lab[j]) { 0 } else { roots[lab[j] as usize] };
                if o != r {
                    *nb.entry(o).or_default() += 1;
                }
            }
        }
    }
    // in a bigger step (most of its border with it): its child, the
    // parent's outline taking it in (a pocket's ribs, a plate's holes)
    let longest = |nb: &HashMap<u32, usize>, skip: &dyn Fn(u32) -> bool| nb.iter().filter(|x| !skip(*x.0)).max_by_key(|x| (*x.1, Reverse(*x.0))).map(|x| (*x.0, *x.1)).unwrap_or((0, 0));
    let mut parent_of: HashMap<u32, u32> = HashMap::new();
    for (&r, nb) in &nbs {
        let total: usize = nb.values().sum();
        let (n, nn) = longest(nb, &|_| false);
        if n != 0 && nn * 2 >= total && cells_of[&r].len() < cells_of.get(&n).map_or(0, |c| c.len()) {
            parent_of.insert(r, n);
        }
    }
    let mut info: HashMap<u32, Info> = HashMap::new();
    for (&r, cells) in &cells_of {
        let reg = &regs[r as usize];
        let fl = face_level.get(&reg.dom).copied().unwrap_or(reg.level);
        // its base: the neighbour it shares most border with, but its own
        // children
        let (n, nn) = longest(&nbs[&r], &|o| parent_of.get(&o) == Some(&r));
        let total: usize = nbs[&r].values().sum();
        let base = if n == 0 { fl } else { regs[n as usize].level };
        let level = reg.level;
        if (level - base).abs() < opts.min_step {
            continue;
        }
        let parent = parent_of.get(&r).copied();
        // its cells where the scan is nearer its level than its base's,
        // within the band between them (a cable over it is not it)
        let (bl, bh) = (level.min(base) - 0.3, level.max(base) + 0.3);
        let keep: Vec<usize> = cells.iter().copied().filter(|&i| scan[i] >= bl && scan[i] <= bh && (scan[i] - level).abs() <= (scan[i] - base).abs()).collect();
        if std::env::var_os("FORME_LEVELS_DEBUG").is_some() && cells.len() as f64 * cell > 5.0 {
            let c = cells.iter().fold(DVec2::ZERO, |a, &i| a + g.corner((i % g.w) as f64 + 0.5, (i / g.w) as f64 + 0.5)) / cells.len() as f64;
            eprintln!("{tag} step {r} at ({:.1}, {:.1}) {:.1} mm², {:.2} → {:.2}, neighbour {n} ({nn} of {total}), parent {parent:?}", c.x, c.y, cells.len() as f64 * cell, base, level);
        }
        info.insert(r, Info { base, level, fl, dom: reg.dom, keep, parent });
    }
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&r, x) in &info {
        if let Some(p) = x.parent.filter(|p| info.contains_key(p)) {
            children.entry(p).or_default().push(r);
        }
    }
    let depth_of = |r: u32| {
        let mut d = 0;
        let mut a = r;
        while let Some(p) = info.get(&a).and_then(|x| x.parent).filter(|p| info.contains_key(p)) {
            d += 1;
            a = p;
            if d > 16 {
                break;
            }
        }
        d
    };
    let mut out = vec![];
    let mut shown: Vec<(Vec<usize>, bool)> = vec![];
    for (&r, x) in &info {
        // its cells and its children's, theirs too
        let mut own: std::collections::HashSet<usize> = x.keep.iter().copied().collect();
        let mut all: Vec<usize> = x.keep.clone();
        let mut stack: Vec<u32> = children.get(&r).cloned().unwrap_or_default();
        let mut seen = 0;
        while let Some(c) = stack.pop() {
            seen += 1;
            if seen > 256 {
                break;
            }
            if let Some(y) = info.get(&c) {
                all.extend(y.keep.iter().copied());
            }
            stack.extend(children.get(&c).cloned().unwrap_or_default());
        }
        all.sort_unstable();
        all.dedup();
        for comp in components(&all, g) {
            let mine: Vec<usize> = comp.iter().copied().filter(|i| own.contains(i)).collect();
            // a level is flat: its top or floor more than its walls (a cable
            // over a face, a slope are not)
            let nflat = mine.iter().filter(|&&i| flat[i]).count();
            let near = mine.iter().filter(|&&i| (scan[i] - x.level).abs() < NEAR).count();
            if std::env::var_os("FORME_LEVELS_DEBUG").is_some() && mine.len() as f64 * cell > 15.0 {
                let (mut cx, mut cy) = (0.0, 0.0);
                for &i in &mine {
                    cx += (i % g.w) as f64;
                    cy += (i / g.w) as f64;
                }
                let c = g.corner(cx / mine.len() as f64 + 0.5, cy / mine.len() as f64 + 0.5);
                eprintln!("{tag} comp at ({:.1}, {:.1}) {:.1} mm² {:.2} → {:.2}: flat {:.2}, near its level {:.2}", c.x, c.y, mine.len() as f64 * cell, x.base, x.level, nflat as f64 / mine.len().max(1) as f64, near as f64 / mine.len().max(1) as f64);
            }
            if mine.len() < min_cells || ((nflat as f64) < MIN_FLAT * mine.len() as f64 && (near as f64) < MIN_NEAR * mine.len() as f64) {
                continue;
            }
            let raised = x.level > x.base;
            for mut st in shape_step(&comp, raised, x.dom, dom, scan, &flat, g, x.base, x.level, x.fl, face_size.get(&x.dom).copied().unwrap_or(1)) {
                st.flat = nflat as f64 / mine.len() as f64;
                st.near = near as f64 / mine.len() as f64;
                st.depth = depth_of(r);
                out.push(st);
            }
            shown.push((mine.clone(), raised));
        }
        own.clear();
    }
    // a face whose most part the scan shows at one other level is off as a
    // whole: its steps there say so (`cover`: their share together)
    let n = out.len();
    for i in 0..n {
        let (f, b, l, up) = (out[i].face, out[i].base, out[i].level, out[i].raised());
        if (b - out[i].face_level).abs() > 0.05 {
            continue;
        }
        let group: Vec<usize> = (0..n).filter(|&j| out[j].face == f && (out[j].base - b).abs() < 0.05 && out[j].raised() == up && (out[j].level - l).abs() < 0.2).collect();
        let share = group.iter().map(|&j| out[j].area).sum::<f64>() / (face_size.get(&f).copied().unwrap_or(1) as f64 * cell);
        out[i].cover = out[i].cover.max(share);
    }
    out.sort_by(|a, b| b.area.total_cmp(&a.area));
    if let Some(dir) = std::env::var_os("FORME_LEVELS_DUMP") {
        dump(std::path::Path::new(&dir).join(format!("levels-{tag}.ppm")), scan, part, dom, &lab, &shown, &out, g);
    }
    out
}

/// A picture of one side (`FORME_LEVELS_DUMP`=dir): the scan against the
/// part on the faces read (green within 0.3 mm, orange above, blue below),
/// grey elsewhere, the regions' borders dark, the steps' cells tinted (red
/// raised, blue recessed) and their outlines white.
#[allow(clippy::too_many_arguments)]
fn dump(path: std::path::PathBuf, scan: &[f64], part: &[f64], dom: &[u32], lab: &[u32], shown: &[(Vec<usize>, bool)], steps: &[Level], g: &Grid) {
    let (w, h) = (g.w, g.h);
    let mut img = vec![[0u8; 3]; w * h];
    let fin: Vec<f64> = scan.iter().copied().filter(|d| d.is_finite()).collect();
    let (lo, hi) = fin.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), d| (a.min(*d), b.max(*d)));
    for i in 0..w * h {
        if !scan[i].is_finite() {
            img[i] = [20, 20, 24];
            continue;
        }
        let t = ((scan[i] - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0);
        let gl = (60.0 + 120.0 * t) as u8;
        img[i] = if dom[i] == NONE {
            [gl, gl, gl]
        } else {
            let d = scan[i] - part[i];
            if d.abs() < 0.3 {
                [40, (90.0 + 100.0 * t) as u8, 60]
            } else if d > 0.0 {
                [(150.0 + 100.0 * (d / 3.0).min(1.0)) as u8, 110, 40]
            } else {
                [40, 90, (150.0 + 100.0 * (-d / 3.0).min(1.0)) as u8]
            }
        };
        let (x, y) = (i % w, i / w);
        if x + 1 < w && lab[i] != lab[i + 1] || y + 1 < h && lab[i] != lab[i + w] {
            img[i] = [img[i][0] / 2, img[i][1] / 2, img[i][2] / 2];
        }
    }
    for (cells, raised) in shown {
        for &i in cells {
            let c = img[i];
            img[i] = if *raised { [255, c[1] / 2, c[2] / 2] } else { [c[0] / 2, c[1] / 2, 255] };
        }
    }
    for st in steps {
        let pts: Vec<DVec2> = match &st.outline {
            Outline::Round { c, r } => (0..64).map(|k| *c + DVec2::from_angle(k as f64 / 64.0 * std::f64::consts::TAU) * *r).collect(),
            o => o.polygon().map(|x| x.0).unwrap_or_default(),
        };
        let n = pts.len();
        for k in 0..n {
            let (a, b) = (pts[k], pts[(k + 1) % n]);
            let steps_n = ((b - a).length() / (g.px * 0.5)).ceil().max(1.0) as usize;
            for j in 0..=steps_n {
                let q = a + (b - a) * (j as f64 / steps_n as f64);
                let (x, y) = (((q.x - g.u0) / g.px) as i64, ((q.y - g.v0) / g.px) as i64);
                if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                    img[y as usize * w + x as usize] = [255, 255, 255];
                }
            }
        }
    }
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    // rows top down: v up
    for y in (0..h).rev() {
        for x in 0..w {
            out.extend_from_slice(&img[y * w + x]);
        }
    }
    let _ = std::fs::write(path, out);
}

/// The 4-connected parts of a set of cells.
fn components(cells: &[usize], g: &Grid) -> Vec<Vec<usize>> {
    let set: std::collections::HashSet<usize> = cells.iter().copied().collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for &s in cells {
        if !seen.insert(s) {
            continue;
        }
        let mut comp = vec![s];
        let mut k = 0;
        while k < comp.len() {
            let a = comp[k];
            k += 1;
            for b in g.around(a) {
                if set.contains(&b) && seen.insert(b) {
                    comp.push(b);
                }
            }
        }
        out.push(comp);
    }
    out
}

/// A step's cells made its outline (traced on a crop of the grid; a pocket
/// reaching the face's border goes on past it, so no sliver is left).
#[allow(clippy::too_many_arguments)]
fn shape_step(comp: &[usize], raised: bool, face: u32, dom: &[u32], scan: &[f64], flat: &[bool], g: &Grid, base: f64, level: f64, fl: f64, face_cells: usize) -> Vec<Level> {
    let px = g.px;
    let close = ((0.3 / px).round() as usize).max(1);
    let grow = (0.6 / px).ceil() as usize;
    let m = close + grow + 3;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for &i in comp {
        let (x, y) = (i % g.w, i / g.w);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let (x0, y0) = (x0.saturating_sub(m), y0.saturating_sub(m));
    let (x1, y1) = ((x1 + m).min(g.w - 1), (y1 + m).min(g.h - 1));
    let sub = Grid { w: x1 - x0 + 1, h: y1 - y0 + 1, px, u0: g.u0 + x0 as f64 * px, v0: g.v0 + y0 as f64 * px };
    let to_sub = |i: usize| (i / g.w - y0) * sub.w + (i % g.w - x0);
    let to_full = |k: usize| (k / sub.w + y0) * g.w + (k % sub.w + x0);
    let mut mask = vec![false; sub.w * sub.h];
    for &i in comp {
        mask[to_sub(i)] = true;
    }
    // thin spurs off (a wall's cells along a neighbour), then gaps filled
    let open = ((0.35 / px).round() as usize).max(2);
    let opened = opening(&mask, &sub, open);
    if opened.iter().filter(|&&m| m).count() as f64 * px * px >= 0.5 * comp.len() as f64 * px * px {
        mask = opened;
    }
    primary::closing(&mut mask, &sub, close);
    if !raised {
        // past the face's border (grown from its cells there, off the face)
        let mut from: Vec<bool> = (0..mask.len()).map(|k| mask[k] && dom[to_full(k)] == face).collect();
        for _ in 0..grow {
            let add: Vec<usize> = (0..mask.len()).filter(|&k| !mask[k] && dom[to_full(k)] != face && sub.around(k).any(|j| from[j])).collect();
            if add.is_empty() {
                break;
            }
            for k in add {
                mask[k] = true;
                from[k] = true;
            }
        }
    }
    let area = comp.len() as f64 * px * px;
    let fit = |mask: &[bool], area: f64| -> Option<Outline> {
        let raw: Vec<DVec2> = primary::trace(mask, &sub).iter().map(|c| sub.corner(c.x, c.y)).collect();
        if raw.len() < 4 {
            return None;
        }
        fit_shape(&raw, px, area)
    };
    let mut outlines: Vec<(Outline, f64)> = vec![];
    match fit(&mask, area) {
        // a comb (blocks joined by thin bridges): its blocks, each its own
        Some(Outline::Poly { pts, .. }) if pts.len() > 8 => {
            let wide = opening(&mask, &sub, ((0.8 / px).round() as usize).max(3));
            let parts = mask_components(&wide, &sub);
            if parts.len() >= 2 {
                for cells in parts {
                    let a = cells.len() as f64 * px * px;
                    if a < 4.0 {
                        continue;
                    }
                    let mut m = vec![false; wide.len()];
                    for k in cells {
                        m[k] = true;
                    }
                    if let Some(o) = fit(&m, a) {
                        outlines.push((o, a));
                    }
                }
            } else if let Some(o) = fit(&mask, area) {
                outlines.push((o, area));
            }
        }
        Some(o) => outlines.push((o, area)),
        None => {}
    }
    let nflat = comp.iter().filter(|&&i| flat[i]).count();
    let mut fv: Vec<f64> = comp.iter().filter(|&&i| flat[i]).map(|&i| scan[i]).collect();
    fv.sort_by(f64::total_cmp);
    let spread = if fv.len() >= 4 { fv[fv.len() * 3 / 4] - fv[fv.len() / 4] } else { f64::NAN };
    outlines
        .into_iter()
        .map(|(outline, area)| Level {
            axis: DVec3::Z,
            u: DVec3::X,
            v: DVec3::Y,
            outline,
            base,
            level,
            face,
            face_level: fl,
            area,
            flat: nflat as f64 / comp.len() as f64,
            near: comp.iter().filter(|&&i| (scan[i] - level).abs() < NEAR).count() as f64 / comp.len() as f64,
            depth: 0,
            spread,
            cover: area / (face_cells.max(1) as f64 * px * px),
            at: DVec3::ZERO,
        })
        .collect()
}

/// The 4-connected parts of a mask (cell indices).
fn mask_components(mask: &[bool], g: &Grid) -> Vec<Vec<usize>> {
    let cells: Vec<usize> = (0..mask.len()).filter(|&k| mask[k]).collect();
    components(&cells, g)
}

/// The cells shrunk by a disc of radius `k` then grown by it (an opening):
/// the parts narrower than 2 `k` gone.
fn opening(mask: &[bool], g: &Grid, k: usize) -> Vec<bool> {
    let r2 = (k * k) as f64;
    let out: Vec<bool> = mask.iter().map(|&m| !m).collect();
    let shrunk: Vec<bool> = primary::distance2(&out, g).iter().map(|&d| d > r2).collect();
    primary::distance2(&shrunk, g).iter().map(|&d| d <= r2).collect()
}

/// The distance from `q` to a rounded rectangle's outline (centred, along
/// its own axes).
fn rrect_dist(q: DVec2, half: DVec2, r: f64) -> f64 {
    let d = q.abs() - (half - DVec2::splat(r));
    (d.max(DVec2::ZERO).length() + d.x.max(d.y).min(0.0) - r).abs()
}

/// The hard-surface shape of a traced outline (`raw`, mm, counter-
/// clockwise; `area` its cells'): a circle, else a rounded rectangle, else
/// straight sides (`primary::fit_outline` for a big one), whichever is the
/// simplest that follows it.
fn fit_shape(raw: &[DVec2], px: f64, area: f64) -> Option<Outline> {
    let n = raw.len();
    // the staircase off: the cell sides' middles, smoothed once
    let mut p: Vec<DVec2> = (0..n).map(|i| (raw[i] + raw[(i + 1) % n]) * 0.5).collect();
    p = (0..n).map(|i| (p[(i + n - 1) % n] + p[i] * 2.0 + p[(i + 1) % n]) * 0.25).collect();
    let size = area.sqrt();
    let tol = (0.6 * px).max(0.03 * size).max(0.08);
    let rms = |e: &mut dyn Iterator<Item = f64>| {
        let (s, c, mx) = e.fold((0.0, 0usize, 0.0f64), |(s, c, mx), x| (s + x * x, c + 1, mx.max(x.abs())));
        ((s / c.max(1) as f64).sqrt(), mx)
    };
    // a circle: its centre the outline's centroid, its radius the area's
    let a2 = primary::signed_area(raw);
    let c = if a2.abs() > 1e-12 {
        (0..n).map(|i| (raw[i] + raw[(i + 1) % n]) * raw[i].perp_dot(raw[(i + 1) % n])).sum::<DVec2>() / (6.0 * a2)
    } else {
        p.iter().copied().sum::<DVec2>() / n as f64
    };
    let r = (area / std::f64::consts::PI).sqrt();
    let (e_round, m_round) = rms(&mut p.iter().map(|q| q.distance(c) - r));
    if e_round < tol && m_round < 2.5 * tol {
        return Some(Outline::Round { c, r });
    }
    // a rounded rectangle: the turn that boxes it tightest (square to the
    // axes within 5°), its corners' radius searched
    let bbox = |t: f64| {
        let (x, y) = (DVec2::new(t.cos(), t.sin()), DVec2::new(-t.sin(), t.cos()));
        let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
        for q in &p {
            let l = DVec2::new(q.dot(x), q.dot(y));
            lo = lo.min(l);
            hi = hi.max(l);
        }
        (lo, hi)
    };
    let mut best = (f64::INFINITY, 0.0);
    for k in 0..180 {
        let t = (-45.0 + 0.5 * k as f64).to_radians();
        let (lo, hi) = bbox(t);
        let s = hi - lo;
        if s.x * s.y < best.0 {
            best = (s.x * s.y, t);
        }
    }
    let mut t = best.1;
    if t.abs() < 5f64.to_radians() {
        t = 0.0;
    }
    let (lo, hi) = bbox(t);
    let x = DVec2::new(t.cos(), t.sin());
    let y = x.perp();
    let mid = (lo + hi) * 0.5;
    let half = (hi - lo) * 0.5;
    let centre = x * mid.x + y * mid.y;
    let local: Vec<DVec2> = p.iter().map(|q| DVec2::new((*q - centre).dot(x), (*q - centre).dot(y))).collect();
    let hmin = half.min_element();
    let mut rb = (f64::INFINITY, 0.0);
    for k in 0..=80 {
        let r = hmin * k as f64 / 80.0;
        let (e, _) = rms(&mut local.iter().map(|q| rrect_dist(*q, half, r)));
        if e < rb.0 {
            rb = (e, r);
        }
    }
    let mut rr = rb.1;
    // a corner of a cell or two is sharp; near half the width, a slot
    if rr < (2.5 * px).max(0.3) {
        rr = 0.0;
    } else if rr > 0.9 * hmin {
        rr = hmin;
    }
    let (e_rect, m_rect) = rms(&mut local.iter().map(|q| rrect_dist(*q, half, rr)));
    // or close enough, its area near the region's: the hard-surface reading
    // of a slightly ragged block
    let ratio = (4.0 * half.x * half.y - (4.0 - std::f64::consts::PI) * rr * rr) / area;
    let loose = e_rect < (0.05 * size).max(tol) && m_rect < (0.18 * size).max(3.0 * tol) && (0.85..1.18).contains(&ratio);
    if (e_rect < tol && m_rect < 3.0 * tol) || loose {
        // the long side along x
        let (x, half) = if half.x >= half.y { (x, half) } else { (y, DVec2::new(half.y, half.x)) };
        return Some(Outline::Rect { c: centre, x, half, r: rr });
    }
    // straight sides, at 45° steps where they are near one
    let fitted = if size > 6.0 { primary::fit_outline(raw, px, size).filter(|(p, _)| p.len() >= 3) } else { None };
    let (pts, cuts) = fitted.unwrap_or_else(|| {
        let pts = primary::square_up(primary::simplify(&p, (0.05 * size).max(1.5 * px).max(0.25)), (2.0 * px).max(0.5));
        let k = pts.len();
        (pts, vec![0.0; k])
    });
    let (pts, cuts) = tidy(pts, cuts, size);
    // a small ragged patch is no hard-surface level (a measure reads the
    // small round parts)
    if pts.len() < 3 || area < 12.0 || (pts.len() > 12 && area < 150.0) {
        return None;
    }
    Some(orient(pts, cuts))
}

/// An outline's sides turned onto 45° steps where within 7° of one (each
/// through its middle), its corners where they meet, sides shorter than
/// 6 % of `size` (0.5 mm at least) and turns under 8° gone.
fn tidy(mut pts: Vec<DVec2>, mut cuts: Vec<f64>, size: f64) -> (Vec<DVec2>, Vec<f64>) {
    let short = (0.06 * size).max(0.5);
    for _ in 0..3 {
        let n = pts.len();
        if n < 3 {
            break;
        }
        // the sides' lines
        let lines: Vec<(DVec2, DVec2)> = (0..n)
            .map(|i| {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                let d = (b - a).normalize_or_zero();
                let ang = d.y.atan2(d.x);
                let step = std::f64::consts::FRAC_PI_4;
                let snapped = (ang / step).round() * step;
                let d = if (ang - snapped).abs() < 7f64.to_radians() { DVec2::new(snapped.cos(), snapped.sin()) } else { d };
                ((a + b) * 0.5, d)
            })
            .collect();
        let mut out = vec![];
        for i in 0..n {
            let (c1, d1) = lines[(i + n - 1) % n];
            let (c2, d2) = lines[i];
            let den = d1.perp_dot(d2);
            let x = if den.abs() > 0.05 { c1 + d1 * (c2 - c1).perp_dot(d2) / den } else { pts[i] };
            out.push(if x.distance(pts[i]) < 2.0 * short + 1.0 { x } else { pts[i] });
        }
        pts = out;
        // short sides and slight turns
        loop {
            let n = pts.len();
            if n <= 3 {
                break;
            }
            let shortest = (0..n).min_by(|&i, &j| pts[i].distance(pts[(i + 1) % n]).total_cmp(&pts[j].distance(pts[(j + 1) % n]))).unwrap();
            // a slight turn, or a spike (turning back by more than 155°)
            let flat = (0..n).find(|&i| {
                let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
                let (d1, d2) = ((b - a).normalize_or_zero(), (c - b).normalize_or_zero());
                let t = d1.perp_dot(d2).atan2(d1.dot(d2)).abs();
                t < 8f64.to_radians() || t > 155f64.to_radians()
            });
            if let Some(i) = flat {
                pts.remove(i);
                cuts.remove(i);
                continue;
            }
            if pts[shortest].distance(pts[(shortest + 1) % n]) < short {
                let j = (shortest + 1) % n;
                let (a, b) = (pts[(shortest + n - 1) % n], pts[(j + 1) % n]);
                let (d1, d2) = ((pts[shortest] - a).normalize_or_zero(), (b - pts[j]).normalize_or_zero());
                let den = d1.perp_dot(d2);
                let mid = (pts[shortest] + pts[j]) * 0.5;
                let x = if den.abs() > 0.05 { a + d1 * (pts[j] - a).perp_dot(d2) / den } else { mid };
                pts[shortest] = if x.distance(mid) < 2.0 * short { x } else { mid };
                cuts[shortest] = if cuts[shortest].abs() >= cuts[j].abs() { cuts[shortest] } else { cuts[j] };
                pts.remove(j);
                cuts.remove(j);
                continue;
            }
            break;
        }
    }
    (pts, cuts)
}

fn orient(mut pts: Vec<DVec2>, mut cuts: Vec<f64>) -> Outline {
    if primary::signed_area(&pts) < 0.0 {
        pts.reverse();
        cuts.reverse();
    }
    Outline::Poly { pts, cuts }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh() -> Mesh {
        Mesh { pos: vec![], tris: vec![], uv: vec![], tri_uv: vec![], unit_mm: 1.0 }
    }

    /// A box's surface, its faces cut in `n` × `n`, each triangle's face
    /// (0 −X, 1 +X, 2 −Y, 3 +Y, 4 −Z, 5 +Z).
    fn add_box(m: &mut Mesh, faces: &mut Vec<u32>, lo: DVec3, hi: DVec3, n: usize) {
        let list: [(usize, f64); 6] = [(0, lo.x), (0, hi.x), (1, lo.y), (1, hi.y), (2, lo.z), (2, hi.z)];
        for (k, (ax, at)) in list.iter().enumerate() {
            let (a, b) = ((ax + 1) % 3, (ax + 2) % 3);
            let base = m.pos.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    let mut p = [0.0; 3];
                    p[*ax] = *at;
                    p[a] = lo[a] + (hi[a] - lo[a]) * i as f64 / n as f64;
                    p[b] = lo[b] + (hi[b] - lo[b]) * j as f64 / n as f64;
                    m.pos.push(DVec3::from_array(p));
                }
            }
            for j in 0..n {
                for i in 0..n {
                    let q = base + (j * (n + 1) + i) as u32;
                    let (q1, q2, q3) = (q + 1, q + (n + 1) as u32, q + (n + 2) as u32);
                    if k % 2 == 1 {
                        m.tris.push([q, q1, q3]);
                        m.tris.push([q, q3, q2]);
                    } else {
                        m.tris.push([q, q3, q1]);
                        m.tris.push([q, q2, q3]);
                    }
                    faces.push(k as u32);
                    faces.push(k as u32);
                }
            }
        }
    }

    /// A height field over [x0, x1] × [y0, y1] at step `h` (its triangles
    /// facing +Z when `up`).
    fn add_field(m: &mut Mesh, x0: f64, x1: f64, y0: f64, y1: f64, h: f64, up: bool, f: impl Fn(f64, f64) -> f64) {
        let (nx, ny) = (((x1 - x0) / h).round() as usize, ((y1 - y0) / h).round() as usize);
        let base = m.pos.len() as u32;
        for j in 0..=ny {
            for i in 0..=nx {
                let (x, y) = (x0 + (x1 - x0) * i as f64 / nx as f64, y0 + (y1 - y0) * j as f64 / ny as f64);
                m.pos.push(DVec3::new(x, y, f(x, y)));
            }
        }
        for j in 0..ny {
            for i in 0..nx {
                let q = base + (j * (nx + 1) + i) as u32;
                let (q1, q2, q3) = (q + 1, q + (nx + 1) as u32, q + (nx + 2) as u32);
                if up {
                    m.tris.push([q, q1, q3]);
                    m.tris.push([q, q3, q2]);
                } else {
                    m.tris.push([q, q3, q1]);
                    m.tris.push([q, q2, q3]);
                }
            }
        }
    }

    /// A smooth step from 0 to 1 over `w` either side of 0.
    fn ramp(d: f64, w: f64) -> f64 {
        ((d / w) * 0.5 + 0.5).clamp(0.0, 1.0)
    }

    /// The scan of a 40 × 30 × 20 block whose top has a rectangular pocket
    /// (2 deep, corners 1), a round pocket (Ø 6, 1 deep), a slot-ended rib
    /// (3 × 15, 1.2 high) with a little wave, and whose bottom has a plate
    /// (10 × 10, 1.5): four steps on the part (the bare block).
    #[test]
    fn pocket_plate_rib() {
        let mut part = mesh();
        let mut faces = vec![];
        add_box(&mut part, &mut faces, DVec3::new(0.0, 0.0, -10.0), DVec3::new(40.0, 30.0, 10.0), 4);
        let mut scan = mesh();
        let wall = 0.12;
        add_field(&mut scan, 0.0, 40.0, 0.0, 30.0, 0.1, true, |x, y| {
            let wave = 0.04 * (x * 0.7).sin() * (y * 0.5).cos();
            // the pocket: a rounded rectangle 5..17 × 5..13, corners 1
            let q = DVec2::new(x - 11.0, y - 9.0);
            let pk = rrect_sd(q, DVec2::new(6.0, 4.0), 1.0);
            let round = DVec2::new(x - 30.0, y - 22.0).length() - 3.0;
            let rib = {
                // a slot from (22, 7) to (22, 19), 3 wide
                let t = (y - 7.0).clamp(0.0, 12.0);
                DVec2::new(x - 22.0, y - 7.0 - t).length() - 1.5
            };
            10.0 + wave - 2.0 * ramp(-pk, wall) - 1.0 * ramp(-round, wall) + 1.2 * ramp(-rib, wall)
        });
        add_field(&mut scan, 0.0, 40.0, 0.0, 30.0, 0.1, false, |x, y| {
            let pl = rrect_sd(DVec2::new(x - 30.0, y - 7.0), DVec2::new(5.0, 5.0), 0.0);
            -10.0 - 1.5 * ramp(-pl, wall)
        });
        let t = std::time::Instant::now();
        let got = read(&scan, &part, &faces, &[DVec3::Z, -DVec3::Z], 0.1, &Opts { px: Some(0.1), ..Default::default() });
        eprintln!("{:.0} ms", t.elapsed().as_secs_f64() * 1e3);
        for l in &got {
            eprintln!("{:?} {} {:.2} → {:.2} area {:.1} flat {:.2} {:?}", l.axis, l.outline.kind(), l.base, l.level, l.area, l.flat, l.outline);
        }
        assert_eq!(got.len(), 4, "four steps");
        let top: Vec<&Level> = got.iter().filter(|l| l.axis.z > 0.0).collect();
        let bot: Vec<&Level> = got.iter().filter(|l| l.axis.z < 0.0).collect();
        assert_eq!(top.len(), 3);
        assert_eq!(bot.len(), 1);
        // the pocket
        let pk = top.iter().find(|l| (l.level - 8.0).abs() < 0.1).expect("the pocket at 8");
        assert!(!pk.raised() && (pk.base - 10.0).abs() < 0.1);
        match &pk.outline {
            Outline::Rect { c, half, r, .. } => {
                assert!(c.distance(DVec2::new(11.0, 9.0)) < 0.2, "{c:?}");
                assert!((half.x - 6.0).abs() < 0.2 && (half.y - 4.0).abs() < 0.2, "{half:?}");
                assert!((r - 1.0).abs() < 0.3, "corners {r}");
            }
            o => panic!("pocket {o:?}"),
        }
        let rd = top.iter().find(|l| (l.level - 9.0).abs() < 0.1).expect("the round pocket at 9");
        match &rd.outline {
            Outline::Round { c, r } => assert!(c.distance(DVec2::new(30.0, 22.0)) < 0.2 && (r - 3.0).abs() < 0.15, "{c:?} {r}"),
            o => panic!("round {o:?}"),
        }
        let rib = top.iter().find(|l| l.raised()).expect("the rib");
        assert!((rib.level - 11.2).abs() < 0.1);
        match &rib.outline {
            Outline::Rect { c, half, r, .. } => {
                assert!(c.distance(DVec2::new(22.0, 13.0)) < 0.2, "{c:?}");
                assert!((half.x - 7.5).abs() < 0.25 && (half.y - 1.5).abs() < 0.2, "{half:?}");
                assert!((r - 1.5).abs() < 0.2, "a slot: {r}");
            }
            o => panic!("rib {o:?}"),
        }
        let pl = bot[0];
        // seen from below: the plate rises 1.5 out of the bottom (10 up there)
        assert!(pl.raised() && (pl.base - 10.0).abs() < 0.1 && (pl.level - 11.5).abs() < 0.1, "{} {}", pl.base, pl.level);
        let p0 = pl.point(pl.outline.centre(), pl.level);
        assert!(p0.distance(DVec3::new(30.0, 7.0, -11.5)) < 0.3, "{p0:?}");
    }

    /// A pocket with a boss standing in it that does not reach the face,
    /// and an island at the face's level: the boss is a step from the
    /// pocket's floor, the island from the floor up to the face.
    #[test]
    fn boss_in_a_pocket() {
        let mut part = mesh();
        let mut faces = vec![];
        add_box(&mut part, &mut faces, DVec3::new(0.0, 0.0, -10.0), DVec3::new(40.0, 30.0, 10.0), 4);
        let mut scan = mesh();
        let wall = 0.12;
        add_field(&mut scan, 0.0, 40.0, 0.0, 30.0, 0.1, true, |x, y| {
            let pk = rrect_sd(DVec2::new(x - 20.0, y - 15.0), DVec2::new(14.0, 10.0), 0.0);
            let boss = DVec2::new(x - 13.0, y - 15.0).length() - 3.0;
            let isl = rrect_sd(DVec2::new(x - 27.0, y - 15.0), DVec2::new(3.0, 4.0), 0.0);
            10.0 - 3.0 * ramp(-pk, wall) + 1.5 * ramp(-boss, wall) + 3.0 * ramp(-isl, wall)
        });
        add_field(&mut scan, 0.0, 40.0, 0.0, 30.0, 0.5, false, |_, _| -10.0);
        let got = read(&scan, &part, &faces, &[DVec3::Z], 0.1, &Opts { px: Some(0.1), ..Default::default() });
        for l in &got {
            eprintln!("{} {:.2} → {:.2} area {:.1} {:?}", l.outline.kind(), l.base, l.level, l.area, l.outline);
        }
        assert_eq!(got.len(), 3);
        assert!(got.iter().any(|l| !l.raised() && (l.base - 10.0).abs() < 0.1 && (l.level - 7.0).abs() < 0.1), "the pocket");
        assert!(got.iter().any(|l| l.raised() && (l.base - 7.0).abs() < 0.1 && (l.level - 8.5).abs() < 0.1 && matches!(l.outline, Outline::Round { .. })), "the boss");
        assert!(got.iter().any(|l| l.raised() && (l.base - 7.0).abs() < 0.1 && (l.level - 10.0).abs() < 0.1), "the island");
    }

    /// The pistol against a part written by the app (`FORME_LEVELS_DUMP`=dir
    /// when it reads levels: dir/part.txt), ignored: the levels printed, a
    /// picture of each side in dir.
    #[test]
    #[ignore]
    fn levels_probe() {
        let dir = std::env::var("FORME_LEVELS_DUMP").expect("FORME_LEVELS_DUMP=dir with part.txt");
        let scan = Mesh::load_obj(std::path::Path::new(&std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into()))).unwrap();
        let txt = std::fs::read_to_string(std::path::Path::new(&dir).join("part.txt")).unwrap();
        let mut part = mesh();
        let mut faces = vec![];
        for l in txt.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            match w.first() {
                Some(&"v") => part.pos.push(DVec3::new(w[1].parse().unwrap(), w[2].parse().unwrap(), w[3].parse().unwrap())),
                Some(&"f") => {
                    part.tris.push([w[1].parse().unwrap(), w[2].parse().unwrap(), w[3].parse().unwrap()]);
                    faces.push(w[4].parse().unwrap());
                }
                _ => {}
            }
        }
        let t = std::time::Instant::now();
        let axes = [DVec3::X, -DVec3::X, DVec3::Y, -DVec3::Y, DVec3::Z, -DVec3::Z];
        let got = read(&scan, &part, &faces, &axes, scan.edge_median(), &Opts::default());
        eprintln!("{} levels, {:.0} ms", got.len(), t.elapsed().as_secs_f64() * 1e3);
        for l in &got {
            eprintln!(
                "{:?} {:>17} face {:>3} {:7.2} → {:7.2} area {:6.1} flat {:.2} cover {:.2} at {:.1?}",
                l.axis, l.outline.kind(), l.face, l.base, l.level, l.area, l.flat, l.cover, l.at
            );
        }
    }

    /// The signed distance to a rounded rectangle.
    fn rrect_sd(q: DVec2, half: DVec2, r: f64) -> f64 {
        let d = q.abs() - (half - DVec2::splat(r));
        d.max(DVec2::ZERO).length() + d.x.max(d.y).min(0.0) - r
    }
}
