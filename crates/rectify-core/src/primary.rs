//! The scan's primary shapes (Forme NURBS, the "scan → shapes" button's
//! first stage; Romain, 19 nov. 2026, 18 h 09: "interpréter d'abord les
//! grosses primary shapes"): the big volumes a hard-surface modeller blocks
//! out first, before the big fillets and cuts (secondary) and the buttons,
//! vents and rivets (tertiary).
//!
//! A part like the pistol shows most of its flat faces to one side (its
//! flanks): it is read as slabs seen from that side, each an outline pulled
//! from one flank's level to the other's. The scan is drawn into two depth
//! maps along the side axis (the nearest surface from the front and from
//! the back); the flat plateaus of the front map are the slabs' flanks,
//! each grown over the map up to the steep walls round it (a watershed of
//! the map's slope from the plateaus), so a vent or a button on a flank
//! stays inside its slab. Each region's back level is the back map's
//! plateau it overlaps most. Regions of about one thickness that touch are
//! one slab; a small one goes into its neighbour (a detail is not a primary
//! shape). Each slab's outline is traced, simplified into straight sides,
//! the sides within 5° of the plane's axes made square to them.
//!
//! The hard-surface rule holds: a slab has one level per flank (the median
//! of its plateau), the scan's waves are not followed; how much of it was
//! read flat (`Primary::flat`) says how sure the read is.

use crate::mesh::Mesh;
use glam::{DVec2, DVec3};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// A slab read on the scan.
#[derive(Clone, Debug)]
pub struct Primary {
    /// The outline in the side plane (`Primaries::u`, `v`), counter-clockwise.
    pub outline: Vec<DVec2>,
    /// Its extent along the side axis (mm): back < front.
    pub back: f64,
    pub front: f64,
    /// The region's area seen from the side (mm²).
    pub area: f64,
    /// How much of it was read flat, front and back (0 to 1).
    pub flat: f64,
    /// Its outline is a rectangle (four square corners): a Cube.
    pub rect: bool,
}

impl Primary {
    pub fn thickness(&self) -> f64 {
        self.front - self.back
    }
}

/// The primary shapes of a scan: slabs along `w`, outlines in (`u`, `v`).
#[derive(Clone, Debug)]
pub struct Primaries {
    pub u: DVec3,
    pub v: DVec3,
    pub w: DVec3,
    /// The biggest first (by volume).
    pub parts: Vec<Primary>,
    /// The depth maps' step (mm).
    pub px: f64,
}

impl Primaries {
    /// A point of the side plane at depth `d`.
    pub fn at(&self, p: DVec2, d: f64) -> DVec3 {
        self.u * p.x + self.v * p.y + self.w * d
    }
}

/// The depth maps' size: about this many steps along the longer side.
const STEPS: f64 = 480.0;
/// A flank's slope (depth per mm) under this is flat (4.6°).
const FLAT_SLOPE: f64 = 0.08;
/// Two neighbouring flat steps of one plateau differ by less than this
/// (mm); the plateau stays within `PLATEAU_SPAN` of its first step.
const PLATEAU_STEP: f64 = 0.06;
const PLATEAU_SPAN: f64 = 0.6;
/// A plateau seeds a slab from this share of the silhouette's area.
const SEED_SHARE: f64 = 0.005;
/// Two touching regions whose front and back levels are within this (mm)
/// are one slab.
const SAME_LEVEL: f64 = 0.6;
/// A region under this share of the silhouette's area goes into its
/// neighbour.
const SMALL_SHARE: f64 = 0.012;
/// A slab's outline is closed over grooves and notches up to twice this
/// wide (mm).
const CLOSE: f64 = 4.0;
/// A side within this of the plane's axes is made square to them.
const AXIS_SNAP_DEG: f64 = 5.0;

/// The side the scan shows most of its flat faces to: the world axis with
/// the most area facing it (within 10°, either way).
pub fn side_axis(m: &Mesh) -> DVec3 {
    let cos = 10f64.to_radians().cos();
    let mut area = [0.0f64; 3];
    for t in &m.tris {
        let [a, b, c] = t.map(|i| m.pos[i as usize]);
        let n = (b - a).cross(c - a);
        let l = n.length();
        if l < 1e-15 {
            continue;
        }
        let n = n / l;
        for (k, ax) in [n.x, n.y, n.z].iter().enumerate() {
            if ax.abs() >= cos {
                area[k] += l * 0.5;
            }
        }
    }
    let k = (0..3).max_by(|&i, &j| area[i].total_cmp(&area[j])).unwrap_or(2);
    [DVec3::X, DVec3::Y, DVec3::Z][k]
}

/// The plane's two axes for a side axis (u × v = w).
fn plane_axes(w: DVec3) -> (DVec3, DVec3) {
    if w.x.abs() > 0.5 {
        (DVec3::Y * w.x.signum(), DVec3::Z)
    } else if w.y.abs() > 0.5 {
        (DVec3::Z * w.y.signum(), DVec3::X)
    } else {
        (DVec3::X * w.z.signum(), DVec3::Y)
    }
}

/// A grid over the side plane: step `px`, the corner of cell (0, 0) at
/// (`u0`, `v0`).
#[derive(Clone, Copy)]
struct Grid {
    w: usize,
    h: usize,
    px: f64,
    u0: f64,
    v0: f64,
}

impl Grid {
    fn corner(&self, x: f64, y: f64) -> DVec2 {
        DVec2::new(self.u0 + x * self.px, self.v0 + y * self.px)
    }
    /// The 4 neighbours of cell `i` inside the grid.
    fn around(&self, i: usize) -> impl Iterator<Item = usize> {
        let (x, y, w, h) = (i % self.w, i / self.w, self.w, self.h);
        [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)].into_iter().flatten()
    }
}

/// The nearest surface along +w (`front`, the largest depth) and along −w
/// (`back`, the smallest), per cell; ±∞ where the scan does not cover it.
fn depth_maps(m: &Mesh, g: &Grid, u: DVec3, v: DVec3, w: DVec3) -> (Vec<f64>, Vec<f64>) {
    let mut front = vec![f64::NEG_INFINITY; g.w * g.h];
    let mut back = vec![f64::INFINITY; g.w * g.h];
    for t in &m.tris {
        let p = t.map(|i| {
            let q = m.pos[i as usize];
            DVec3::new((q.dot(u) - g.u0) / g.px - 0.5, (q.dot(v) - g.v0) / g.px - 0.5, q.dot(w))
        });
        let d = (p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[1].y - p[0].y) * (p[2].x - p[0].x);
        if d.abs() < 1e-12 {
            continue;
        }
        let x0 = p.iter().map(|q| q.x).fold(f64::INFINITY, f64::min).ceil().max(0.0) as usize;
        let x1 = p.iter().map(|q| q.x).fold(f64::NEG_INFINITY, f64::max).floor().min(g.w as f64 - 1.0);
        let y0 = p.iter().map(|q| q.y).fold(f64::INFINITY, f64::min).ceil().max(0.0) as usize;
        let y1 = p.iter().map(|q| q.y).fold(f64::NEG_INFINITY, f64::max).floor().min(g.h as f64 - 1.0);
        if x1 < 0.0 || y1 < 0.0 {
            continue;
        }
        for y in y0..=y1 as usize {
            for x in x0..=x1 as usize {
                let (fx, fy) = (x as f64, y as f64);
                let l1 = ((p[1].x - fx) * (p[2].y - fy) - (p[1].y - fy) * (p[2].x - fx)) / d;
                let l2 = ((p[2].x - fx) * (p[0].y - fy) - (p[2].y - fy) * (p[0].x - fx)) / d;
                let l3 = 1.0 - l1 - l2;
                if l1 < -1e-9 || l2 < -1e-9 || l3 < -1e-9 {
                    continue;
                }
                let z = l1 * p[0].z + l2 * p[1].z + l3 * p[2].z;
                let i = y * g.w + x;
                front[i] = front[i].max(z);
                back[i] = back[i].min(z);
            }
        }
    }
    (front, back)
}

/// The map with each covered cell set to the median of the covered cells
/// round it (3 × 3): a stray triangle does not make a step.
fn smooth(d: &[f64], g: &Grid) -> Vec<f64> {
    let mut out = d.to_vec();
    let mut vals = Vec::with_capacity(9);
    for y in 0..g.h {
        for x in 0..g.w {
            let i = y * g.w + x;
            if !d[i].is_finite() {
                continue;
            }
            vals.clear();
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                    if xx >= 0 && yy >= 0 && (xx as usize) < g.w && (yy as usize) < g.h {
                        let q = d[yy as usize * g.w + xx as usize];
                        if q.is_finite() {
                            vals.push(q);
                        }
                    }
                }
            }
            vals.sort_by(|a, b| a.total_cmp(b));
            out[i] = vals[vals.len() / 2];
        }
    }
    out
}

/// Each covered cell's slope (depth per mm) and whether it is flat; a cell
/// next to an uncovered one is steep (the silhouette).
fn slopes(d: &[f64], g: &Grid) -> (Vec<f64>, Vec<bool>) {
    let mut s = vec![f64::INFINITY; d.len()];
    let mut flat = vec![false; d.len()];
    for y in 0..g.h {
        for x in 0..g.w {
            let i = y * g.w + x;
            if !d[i].is_finite() {
                continue;
            }
            let at = |xx: i64, yy: i64| -> Option<f64> {
                if xx < 0 || yy < 0 || xx as usize >= g.w || yy as usize >= g.h {
                    return None;
                }
                let q = d[yy as usize * g.w + xx as usize];
                q.is_finite().then_some(q)
            };
            let (xi, yi) = (x as i64, y as i64);
            let (l, r, b, t) = (at(xi - 1, yi), at(xi + 1, yi), at(xi, yi - 1), at(xi, yi + 1));
            let edge = l.is_none() || r.is_none() || b.is_none() || t.is_none();
            let dx = match (l, r) {
                (Some(l), Some(r)) => (r - l) / (2.0 * g.px),
                (Some(l), None) => (d[i] - l) / g.px,
                (None, Some(r)) => (r - d[i]) / g.px,
                _ => 0.0,
            };
            let dy = match (b, t) {
                (Some(b), Some(t)) => (t - b) / (2.0 * g.px),
                (Some(b), None) => (d[i] - b) / g.px,
                (None, Some(t)) => (t - d[i]) / g.px,
                _ => 0.0,
            };
            let slope = dx.hypot(dy);
            s[i] = if edge { slope.max(10.0) } else { slope.min(100.0) };
            flat[i] = !edge && slope < FLAT_SLOPE;
        }
    }
    (s, flat)
}

/// The flat plateaus of a map: each a run of flat cells, neighbours within
/// `PLATEAU_STEP`, all within `PLATEAU_SPAN` of the first. Labels (0: none)
/// and each plateau's cells.
fn plateaus(d: &[f64], flat: &[bool], g: &Grid) -> (Vec<u32>, Vec<Vec<usize>>) {
    let mut lab = vec![0u32; d.len()];
    let mut list: Vec<Vec<usize>> = vec![vec![]];
    let mut stack = vec![];
    for s in 0..d.len() {
        if !flat[s] || lab[s] != 0 {
            continue;
        }
        let id = list.len() as u32;
        let base = d[s];
        let mut cells = vec![s];
        lab[s] = id;
        stack.push(s);
        while let Some(a) = stack.pop() {
            for b in g.around(a) {
                if flat[b] && lab[b] == 0 && (d[b] - d[a]).abs() < PLATEAU_STEP && (d[b] - base).abs() < PLATEAU_SPAN {
                    lab[b] = id;
                    cells.push(b);
                    stack.push(b);
                }
            }
        }
        list.push(cells);
    }
    (lab, list)
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// Every covered cell given the label of the seed it is reached from by the
/// lowest slopes (a watershed from the seeds: regions meet on the walls).
fn flood(slope: &[f64], seeds: &[u32], g: &Grid) -> Vec<u32> {
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
            if lab[b] == 0 && slope[b].is_finite() {
                lab[b] = lab[a];
                heap.push(Reverse((key(slope[b]), n, b)));
                n += 1;
            }
        }
    }
    lab
}

/// A region of the side view while it is being read.
#[derive(Clone, Debug)]
struct Region {
    cells: usize,
    front: f64,
    back: f64,
    flat: usize,
    alive: bool,
}

/// The squared distance (in cells) from each cell to the nearest cell set
/// in `mask` (Felzenszwalb and Huttenlocher's exact transform).
fn distance2(mask: &[bool], g: &Grid) -> Vec<f64> {
    const BIG: f64 = 1e20;
    fn pass(f: &[f64], out: &mut [f64]) {
        let n = f.len();
        let mut v = vec![0usize; n];
        let mut z = vec![0.0f64; n + 1];
        let mut k = 0;
        z[0] = f64::NEG_INFINITY;
        z[1] = f64::INFINITY;
        for q in 1..n {
            loop {
                let p = v[k];
                let s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * (q as f64 - p as f64));
                if s <= z[k] && k > 0 {
                    k -= 1;
                    continue;
                }
                if s <= z[k] {
                    // k == 0: replace
                    v[0] = q;
                    z[1] = f64::INFINITY;
                    break;
                }
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f64::INFINITY;
                break;
            }
        }
        k = 0;
        for q in 0..n {
            while z[k + 1] < q as f64 {
                k += 1;
            }
            let p = v[k];
            out[q] = (q as f64 - p as f64).powi(2) + f[p];
        }
    }
    let mut d: Vec<f64> = mask.iter().map(|&m| if m { 0.0 } else { BIG }).collect();
    let mut col = vec![0.0; g.h];
    let mut res = vec![0.0; g.h.max(g.w)];
    for x in 0..g.w {
        for y in 0..g.h {
            col[y] = d[y * g.w + x];
        }
        pass(&col, &mut res[..g.h]);
        for y in 0..g.h {
            d[y * g.w + x] = res[y];
        }
    }
    for y in 0..g.h {
        let row: Vec<f64> = d[y * g.w..(y + 1) * g.w].to_vec();
        pass(&row, &mut res[..g.w]);
        d[y * g.w..(y + 1) * g.w].copy_from_slice(&res[..g.w]);
    }
    d
}

/// The cells grown by a disc of radius `k` then shrunk by it (a closing):
/// the gaps narrower than 2 `k` filled, a V-groove too.
fn closing(mask: &mut Vec<bool>, g: &Grid, k: usize) {
    let r2 = (k * k) as f64;
    let grown: Vec<bool> = distance2(mask, g).iter().map(|&d| d <= r2).collect();
    let out: Vec<bool> = grown.iter().map(|&m| !m).collect();
    *mask = distance2(&out, g).iter().map(|&d| d > r2).collect();
}

/// The outer boundary of a set of cells, counter-clockwise, through cell
/// corners (in cell units).
fn trace(mask: &[bool], g: &Grid) -> Vec<DVec2> {
    let at = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < g.w && (y as usize) < g.h && mask[y as usize * g.w + x as usize];
    // directed sides, inside on the left
    let mut edges: Vec<((i64, i64), (i64, i64))> = vec![];
    for y in 0..g.h as i64 {
        for x in 0..g.w as i64 {
            if !at(x, y) {
                continue;
            }
            if !at(x, y - 1) {
                edges.push(((x, y), (x + 1, y)));
            }
            if !at(x + 1, y) {
                edges.push(((x + 1, y), (x + 1, y + 1)));
            }
            if !at(x, y + 1) {
                edges.push(((x + 1, y + 1), (x, y + 1)));
            }
            if !at(x - 1, y) {
                edges.push(((x, y + 1), (x, y)));
            }
        }
    }
    let mut from: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (k, e) in edges.iter().enumerate() {
        from.entry(e.0).or_default().push(k);
    }
    let mut used = vec![false; edges.len()];
    let mut best: Vec<DVec2> = vec![];
    let mut best_area = 0.0;
    for s in 0..edges.len() {
        if used[s] {
            continue;
        }
        let mut lp = vec![];
        let mut k = s;
        loop {
            used[k] = true;
            let (a, b) = edges[k];
            lp.push(DVec2::new(a.0 as f64, a.1 as f64));
            let d = (b.0 - a.0, b.1 - a.1);
            // where two cells touch at a corner only, turn left (they stay
            // one outline)
            let next = from.get(&b).and_then(|c| {
                c.iter().copied().filter(|&j| !used[j]).max_by_key(|&j| {
                    let e = edges[j];
                    let o = (e.1 .0 - e.0 .0, e.1 .1 - e.0 .1);
                    d.0 * o.1 - d.1 * o.0
                })
            });
            match next {
                Some(j) => k = j,
                None => break,
            }
        }
        let area = signed_area(&lp);
        if area > best_area {
            best_area = area;
            best = lp;
        }
    }
    best
}

fn signed_area(p: &[DVec2]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum::<f64>() * 0.5
}

/// Douglas-Peucker on an open run.
fn simplify_run(p: &[DVec2], tol: f64, out: &mut Vec<DVec2>) {
    let (a, b) = (p[0], p[p.len() - 1]);
    let ab = b - a;
    let l = ab.length();
    let mut far = (0.0, 0);
    for (k, q) in p.iter().enumerate().take(p.len() - 1).skip(1) {
        let d = if l > 1e-12 { (*q - a).perp_dot(ab).abs() / l } else { (*q - a).length() };
        if d > far.0 {
            far = (d, k);
        }
    }
    if far.0 > tol {
        simplify_run(&p[..=far.1], tol, out);
        simplify_run(&p[far.1..], tol, out);
    } else {
        out.push(a);
    }
}

/// Douglas-Peucker on a closed outline.
fn simplify(p: &[DVec2], tol: f64) -> Vec<DVec2> {
    if p.len() < 4 {
        return p.to_vec();
    }
    let far = (1..p.len()).max_by(|&i, &j| p[i].distance(p[0]).total_cmp(&p[j].distance(p[0]))).unwrap();
    let mut out = vec![];
    let mut a: Vec<DVec2> = p[..=far].to_vec();
    simplify_run(&a, tol, &mut out);
    a = p[far..].to_vec();
    a.push(p[0]);
    simplify_run(&a, tol, &mut out);
    out
}

/// The outline's sides made straight lines: a side within `AXIS_SNAP_DEG`
/// of an axis made square to it (through its middle), each corner where
/// its two sides meet; sides shorter than `short` and turns under 8° gone.
fn square_up(p: Vec<DVec2>, short: f64) -> Vec<DVec2> {
    let mut p = p;
    let snap = AXIS_SNAP_DEG.to_radians().sin();
    for _ in 0..3 {
        // drop turns under 8° and short sides
        let mut changed = true;
        while changed && p.len() > 3 {
            changed = false;
            let n = p.len();
            for i in 0..n {
                let (a, b, c) = (p[(i + n - 1) % n], p[i], p[(i + 1) % n]);
                let (d1, d2) = ((b - a).normalize_or_zero(), (c - b).normalize_or_zero());
                if d1.perp_dot(d2).abs() < 8f64.to_radians().sin() && d1.dot(d2) > 0.0 {
                    p.remove(i);
                    changed = true;
                    break;
                }
                if b.distance(c) < short {
                    p[i] = (b + c) * 0.5;
                    p.remove((i + 1) % n);
                    changed = true;
                    break;
                }
            }
        }
        let n = p.len();
        if n < 3 {
            return p;
        }
        // each side's line, near-axis ones squared
        let lines: Vec<(DVec2, DVec2)> = (0..n)
            .map(|i| {
                let (a, b) = (p[i], p[(i + 1) % n]);
                let d = (b - a).normalize_or_zero();
                let d = if d.y.abs() < snap {
                    DVec2::new(d.x.signum(), 0.0)
                } else if d.x.abs() < snap {
                    DVec2::new(0.0, d.y.signum())
                } else {
                    d
                };
                ((a + b) * 0.5, d)
            })
            .collect();
        let mut q = Vec::with_capacity(n);
        for i in 0..n {
            let (pa, da) = lines[(i + n - 1) % n];
            let (pb, db) = lines[i];
            let den = da.perp_dot(db);
            if den.abs() < 0.17 {
                q.push(p[i]);
                continue;
            }
            let t = (pb - pa).perp_dot(db) / den;
            let x = pa + da * t;
            q.push(if x.distance(p[i]) < short * 4.0 { x } else { p[i] });
        }
        p = q;
    }
    // a few short sides cutting across a right angle between the sides
    // round them (a corner the closing rounded): the corner again
    let mut k = 0;
    'next: while p.len() > 4 && k < p.len() {
        let n = p.len();
        for m in 1..=3usize.min(n - 4) {
            let a = p[(k + n - 1) % n];
            let b = p[k];
            let c = p[(k + m) % n];
            let d = p[(k + m + 1) % n];
            let run: f64 = (0..m).map(|j| p[(k + j) % n].distance(p[(k + j + 1) % n])).sum();
            // the closing rounds concave corners only, each into a quarter
            // circle of `CLOSE`: two or three sides, or one short one (a
            // longer single side is a chamfer, kept)
            if run > CLOSE * 1.8 || (m == 1 && run > CLOSE * 0.9) {
                if m == 1 {
                    continue;
                }
                break;
            }
            let (d0, d2) = ((b - a).normalize_or_zero(), (d - c).normalize_or_zero());
            if d0.dot(d2).abs() > 10f64.to_radians().sin() {
                continue;
            }
            let turn = d0.perp_dot(d2).signum();
            if turn > 0.0 {
                // a convex corner (the outline runs counter-clockwise)
                continue;
            }
            let mut prev = d0;
            let mut same = true;
            for j in 0..=m {
                let dir = (p[(k + j + 1) % n] - p[(k + j) % n]).normalize_or_zero();
                if prev.perp_dot(dir) * turn < -1e-9 {
                    same = false;
                }
                prev = dir;
            }
            if !same {
                continue;
            }
            let den = d0.perp_dot(d2);
            let x = b + d0 * (c - b).perp_dot(d2) / den;
            if x.distance(b) > CLOSE * 2.5 {
                continue;
            }
            p[k] = x;
            for _ in 0..m {
                let at = (k + 1) % p.len();
                p.remove(at);
                if at < k {
                    k -= 1;
                }
            }
            continue 'next;
        }
        k += 1;
    }
    p
}

/// Whether an outline is a rectangle: four corners, each within 3° of square.
fn is_rect(p: &[DVec2]) -> bool {
    if p.len() != 4 {
        return false;
    }
    (0..4).all(|i| {
        let (a, b, c) = (p[(i + 3) % 4], p[i], p[(i + 1) % 4]);
        let (d1, d2) = ((b - a).normalize_or_zero(), (c - b).normalize_or_zero());
        d1.dot(d2).abs() < 3f64.to_radians().sin()
    })
}

/// The scan's primary shapes, read from its side (`side_axis`); `grain`:
/// the scan's median edge (mm).
pub fn read(m: &Mesh, grain: f64) -> Primaries {
    let w = side_axis(m);
    read_along(m, w, grain)
}

/// The primary shapes read as slabs along `w` (a world axis).
pub fn read_along(m: &Mesh, w: DVec3, grain: f64) -> Primaries {
    let (u, v) = plane_axes(w);
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for q in &m.pos {
        let p = DVec2::new(q.dot(u), q.dot(v));
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let ext = (hi - lo).max_element();
    let px = (ext / STEPS).max(grain * 0.8).max(1e-3);
    let close = (CLOSE / px).ceil() as usize;
    let margin = close + 2;
    let g = Grid {
        w: ((hi.x - lo.x) / px).ceil() as usize + 2 * margin,
        h: ((hi.y - lo.y) / px).ceil() as usize + 2 * margin,
        px,
        u0: lo.x - margin as f64 * px,
        v0: lo.y - margin as f64 * px,
    };
    let (front, back) = depth_maps(m, &g, u, v, w);
    let front = smooth(&front, &g);
    // the back map turned to face the other way: its plateaus read alike
    let back_up: Vec<f64> = smooth(&back, &g).iter().map(|d| -d).collect();
    let (fs, fflat) = slopes(&front, &g);
    let (_, bflat) = slopes(&back_up, &g);
    let inside = front.iter().filter(|d| d.is_finite()).count();
    let cell = px * px;
    let seed_min = (inside as f64 * SEED_SHARE).max(16.0);
    let (flab, fpl) = plateaus(&front, &fflat, &g);
    let (blab, bpl) = plateaus(&back_up, &bflat, &g);

    // the seeds: the front's big plateaus
    let mut seeds = vec![0u32; front.len()];
    let mut regions: Vec<Region> = vec![Region { cells: 0, front: 0.0, back: 0.0, flat: 0, alive: false }];
    for (k, cells) in fpl.iter().enumerate().skip(1) {
        if (cells.len() as f64) < seed_min {
            continue;
        }
        let id = regions.len() as u32;
        for &c in cells {
            seeds[c] = id;
        }
        regions.push(Region { cells: 0, front: median(cells.iter().map(|&c| front[c]).collect()), back: 0.0, flat: 0, alive: true });
        let _ = k;
    }
    let mut lab = flood(&fs, &seeds, &g);

    // each region's back: the back plateau it overlaps most
    let mut over: HashMap<(u32, u32), usize> = HashMap::new();
    for i in 0..lab.len() {
        if lab[i] == 0 {
            continue;
        }
        let r = &mut regions[lab[i] as usize];
        r.cells += 1;
        if fflat[i] {
            r.flat += 1;
        }
        if bflat[i] {
            r.flat += 1;
        }
        let b = blab[i];
        if b != 0 && bpl[b as usize].len() as f64 >= seed_min * 0.5 {
            *over.entry((lab[i], b)).or_default() += 1;
        }
    }
    let back_med: Vec<f64> = bpl.iter().map(|c| -median(c.iter().map(|&i| back_up[i]).collect())).collect();
    for (id, r) in regions.iter_mut().enumerate().skip(1) {
        let best = over.iter().filter(|((a, _), _)| *a == id as u32).max_by_key(|(_, n)| **n).map(|((_, b), _)| *b);
        r.back = match best {
            Some(b) => back_med[b as usize],
            None => {
                let cells: Vec<f64> = (0..lab.len()).filter(|&i| lab[i] == id as u32 && back[i].is_finite()).map(|i| back[i]).collect();
                median(cells)
            }
        };
    }

    // which regions touch, how long
    let touching = |lab: &[u32]| {
        let mut t: HashMap<(u32, u32), usize> = HashMap::new();
        for y in 0..g.h {
            for x in 0..g.w {
                let i = y * g.w + x;
                for j in [(x + 1 < g.w).then(|| i + 1), (y + 1 < g.h).then(|| i + g.w)].into_iter().flatten() {
                    let (a, b) = (lab[i], lab[j]);
                    if (a != 0 || b != 0) && a != b {
                        *t.entry((a.min(b), a.max(b))).or_default() += 1;
                    }
                }
            }
        }
        t
    };
    let mut parent: Vec<u32> = (0..regions.len() as u32).collect();
    fn root(p: &mut [u32], mut a: u32) -> u32 {
        while p[a as usize] != a {
            p[a as usize] = p[p[a as usize] as usize];
            a = p[a as usize];
        }
        a
    }
    let join = |regions: &mut Vec<Region>, parent: &mut Vec<u32>, a: u32, b: u32, keep_levels_of: Option<u32>| {
        let (ra, rb) = (root(parent, a), root(parent, b));
        if ra == rb {
            return;
        }
        let (x, y) = (regions[ra as usize].clone(), regions[rb as usize].clone());
        let n = (x.cells + y.cells).max(1) as f64;
        let (front, back) = match keep_levels_of {
            Some(k) if root(parent, k) == ra => (x.front, x.back),
            Some(_) => (y.front, y.back),
            None => ((x.front * x.cells as f64 + y.front * y.cells as f64) / n, (x.back * x.cells as f64 + y.back * y.cells as f64) / n),
        };
        parent[rb as usize] = ra;
        let r = &mut regions[ra as usize];
        r.cells = x.cells + y.cells;
        r.flat = x.flat + y.flat;
        r.front = front;
        r.back = back;
        regions[rb as usize].alive = false;
    };
    // touching regions of one thickness: one slab
    let t = touching(&lab);
    let mut pairs: Vec<_> = t.into_iter().filter(|((a, _), n)| *a != 0 && *n as f64 * px >= 2.0).collect();
    pairs.sort_by_key(|(_, n)| Reverse(*n));
    for ((a, b), _) in pairs {
        let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
        let (x, y) = (&regions[ra as usize], &regions[rb as usize]);
        if (x.front - y.front).abs() < SAME_LEVEL && (x.back - y.back).abs() < SAME_LEVEL {
            join(&mut regions, &mut parent, a, b, None);
        }
    }
    for l in lab.iter_mut() {
        *l = root(&mut parent, *l);
    }
    // small regions, and details a region holds (a button or a pocket on
    // a flank: most of its outline against that one region, much smaller
    // than it), into the neighbour they share most with
    let small = inside as f64 * SMALL_SHARE;
    loop {
        let t = touching(&lab);
        let mut perim: HashMap<u32, usize> = HashMap::new();
        let mut most: HashMap<u32, (u32, usize)> = HashMap::new();
        for (&(a, b), &n) in &t {
            for (x, y) in [(a, b), (b, a)] {
                if x == 0 {
                    continue;
                }
                *perim.entry(x).or_default() += n;
                if y != 0 && most.get(&x).map_or(true, |m| n > m.1) {
                    most.insert(x, (y, n));
                }
            }
        }
        let held = |r: u32| {
            most.get(&r).is_some_and(|&(o, n)| {
                n as f64 >= 0.75 * perim[&r] as f64 && regions[r as usize].cells * 6 < regions[o as usize].cells
            })
        };
        let mut cand: Vec<u32> = (1..regions.len() as u32).filter(|&r| regions[r as usize].alive && ((regions[r as usize].cells as f64) < small || held(r))).collect();
        cand.sort_by_key(|&r| regions[r as usize].cells);
        let Some(&r) = cand.first() else { break };
        let best = most.get(&r).map(|m| m.0);
        match best {
            Some(o) => {
                join(&mut regions, &mut parent, o, r, Some(o));
                for l in lab.iter_mut() {
                    *l = root(&mut parent, *l);
                }
            }
            None => {
                regions[r as usize].alive = false;
                for l in lab.iter_mut() {
                    if *l == r {
                        *l = 0;
                    }
                }
            }
        }
    }

    // each slab's outline, grown a little under the thicker slabs round it
    // (no faces laid flush on each other)
    let grow = (0.5 / px).ceil() as usize;
    let mut parts = vec![];
    for (id, r) in regions.iter().enumerate().skip(1) {
        if !r.alive || r.cells == 0 || !(r.front > r.back) {
            continue;
        }
        let id = id as u32;
        let mut mask: Vec<bool> = lab.iter().map(|&l| l == id).collect();
        let thick = r.front - r.back;
        for _ in 0..grow {
            let add: Vec<usize> = (0..mask.len())
                .filter(|&i| {
                    !mask[i] && lab[i] != 0 && {
                        let o = &regions[lab[i] as usize];
                        o.front - o.back > thick + 0.2
                    } && g.around(i).any(|j| mask[j])
                })
                .collect();
            for i in add {
                mask[i] = true;
            }
        }
        // grooves and notches narrower than twice `CLOSE` filled (the
        // ribs across a top, a gap between two vents at the border are
        // later stages' cuts)
        closing(&mut mask, &g, close);
        let lp = trace(&mask, &g);
        if lp.len() < 4 {
            continue;
        }
        let pts: Vec<DVec2> = lp.iter().map(|c| g.corner(c.x, c.y)).collect();
        // the outline's sides within about 2 % of its size: a primary
        // shape has few corners
        let tol = (px * 2.5).max(0.6).max((r.cells as f64 * cell).sqrt() * 0.02);
        let mut outline = square_up(simplify(&pts, tol), tol * 1.5);
        if signed_area(&outline) < 0.0 {
            outline.reverse();
        }
        if outline.len() < 3 {
            continue;
        }
        let rect = is_rect(&outline);
        parts.push(Primary { outline, back: r.back, front: r.front, area: r.cells as f64 * cell, flat: r.flat as f64 / (2.0 * r.cells as f64), rect });
    }
    parts.sort_by(|a, b| (b.area * b.thickness()).total_cmp(&(a.area * a.thickness())));
    let _ = flab;
    Primaries { u, v, w, parts, px }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box's surface, its sides cut in `n` × `n` (as a scan's triangles).
    fn add_box(m: &mut Mesh, lo: DVec3, hi: DVec3, n: usize) {
        let faces: [(usize, f64); 6] = [(0, lo.x), (0, hi.x), (1, lo.y), (1, hi.y), (2, lo.z), (2, hi.z)];
        for (k, (ax, at)) in faces.iter().enumerate() {
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
                }
            }
        }
    }

    fn mesh() -> Mesh {
        Mesh { pos: vec![], tris: vec![], uv: vec![], tri_uv: vec![], unit_mm: 1.0 }
    }

    fn bounds(p: &[DVec2]) -> (DVec2, DVec2) {
        p.iter().fold((DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)), |(a, b), q| (a.min(*q), b.max(*q)))
    }

    /// A thick block, a thinner one beside it, a small button on the
    /// first's flank and a pocket in the second's: two slabs, the button and
    /// the pocket left to later stages.
    #[test]
    fn two_blocks() {
        let mut m = mesh();
        add_box(&mut m, DVec3::new(0.0, 0.0, -10.0), DVec3::new(40.0, 30.0, 10.0), 40);
        add_box(&mut m, DVec3::new(-30.0, 0.0, -6.0), DVec3::new(0.5, 15.0, 6.0), 40);
        add_box(&mut m, DVec3::new(10.0, 10.0, 9.0), DVec3::new(16.0, 16.0, 11.0), 6);
        add_box(&mut m, DVec3::new(-20.0, 5.0, 4.0), DVec3::new(-15.0, 9.0, 6.01), 6);
        assert_eq!(side_axis(&m), DVec3::Z);
        let p = read(&m, 0.5);
        for s in &p.parts {
            eprintln!("{:?} {:.2}..{:.2} area {:.0} flat {:.2} rect {}", s.outline, s.back, s.front, s.area, s.flat, s.rect);
        }
        assert_eq!(p.parts.len(), 2, "two slabs");
        let a = &p.parts[0];
        assert!((a.front - 10.0).abs() < 0.05 && (a.back + 10.0).abs() < 0.05, "{:.2} {:.2}", a.back, a.front);
        assert!(a.rect, "the big block is a rectangle");
        let (lo, hi) = bounds(&a.outline);
        assert!(lo.distance(DVec2::new(0.0, 0.0)) < 0.6 && hi.distance(DVec2::new(40.0, 30.0)) < 0.6, "{lo:?} {hi:?}");
        let b = &p.parts[1];
        assert!((b.front - 6.0).abs() < 0.05 && (b.back + 6.0).abs() < 0.05, "{:.2} {:.2}", b.back, b.front);
        let (lo, hi) = bounds(&b.outline);
        // grown a little under the thick block
        assert!((lo.x + 30.0).abs() < 0.6 && lo.y.abs() < 0.6 && hi.x > 0.4 && hi.x < 1.6 && (hi.y - 15.0).abs() < 0.6, "{lo:?} {hi:?}");
        assert!(b.rect);
    }

    /// A slab whose outline is an L, turned 30° in its plane: one slab, six
    /// corners, its slanted sides kept.
    #[test]
    fn l_outline_turned() {
        let mut m = mesh();
        add_box(&mut m, DVec3::new(0.0, 0.0, -5.0), DVec3::new(30.0, 10.0, 5.0), 30);
        add_box(&mut m, DVec3::new(0.0, 9.5, -5.0), DVec3::new(10.0, 30.0, 5.0), 30);
        let q = glam::DQuat::from_rotation_z(30f64.to_radians());
        for p in m.pos.iter_mut() {
            *p = q * *p;
        }
        let p = read(&m, 0.3);
        assert_eq!(p.parts.len(), 1);
        let s = &p.parts[0];
        eprintln!("{:?}", s.outline);
        assert_eq!(s.outline.len(), 6, "{:?}", s.outline);
        assert!(!s.rect);
        assert!((s.front - 5.0).abs() < 0.05 && (s.back + 5.0).abs() < 0.05);
        let area = signed_area(&s.outline);
        assert!((area - (300.0 + 200.0)).abs() < 25.0, "area {area}");
    }

    /// The pistol (ignored: the mesh is not in the repository): its primary
    /// shapes printed. `FORME_REF` = another .obj.
    #[test]
    #[ignore]
    fn primary_probe() {
        let path = std::env::var("FORME_REF").unwrap_or_else(|_| "/mnt/project-files/GUN_A_ORIGIN.obj".into());
        let m = Mesh::load_obj(std::path::Path::new(&path)).unwrap();
        let t = std::time::Instant::now();
        let p = read(&m, m.edge_median());
        eprintln!("side {:?}, step {:.3} mm, {} parts, {:.0} ms", p.w, p.px, p.parts.len(), t.elapsed().as_secs_f64() * 1e3);
        for s in &p.parts {
            let pts: Vec<String> = s.outline.iter().map(|q| format!("[{:.2},{:.2}]", q.x, q.y)).collect();
            println!("{{\"back\":{:.3},\"front\":{:.3},\"area\":{:.1},\"flat\":{:.3},\"rect\":{},\"outline\":[{}]}}", s.back, s.front, s.area, s.flat, s.rect, pts.join(","));
        }
    }
}
