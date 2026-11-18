//! The triangle mesh: positions in millimetres, triangles, and what the .obj
//! carried besides (texture coordinates per corner) so that the export keeps
//! the original UVs: Rectify only moves vertices, it never changes the
//! triangles.

use glam::DVec3;
use rayon::prelude::*;
use std::path::Path;

#[derive(Clone, Default)]
pub struct Mesh {
    pub pos: Vec<DVec3>,
    pub tris: Vec<[u32; 3]>,
    /// Texture coordinates and, per triangle, the index of each corner's.
    pub uv: Vec<[f32; 2]>,
    pub tri_uv: Vec<[u32; 3]>,
    /// Millimetres per unit of the file (10 for a file in centimetres).
    pub unit_mm: f64,
}

/// Who touches what, built once (the triangles never change).
pub struct Topo {
    /// Triangles around each vertex (CSR: `vt_start[v]..vt_start[v + 1]`).
    pub vt_start: Vec<u32>,
    pub vt: Vec<u32>,
    /// Across each side of each triangle (side k = corners k, k + 1): the
    /// triangle on the other side, u32::MAX on a border.
    pub adj: Vec<[u32; 3]>,
    /// Vertices next to each vertex (CSR).
    pub vv_start: Vec<u32>,
    pub vv: Vec<u32>,
}

pub const NONE: u32 = u32::MAX;

impl Topo {
    pub fn new(m: &Mesh) -> Topo {
        let nv = m.pos.len();
        let mut count = vec![0u32; nv + 1];
        for t in &m.tris {
            for &v in t {
                count[v as usize + 1] += 1;
            }
        }
        for i in 0..nv {
            count[i + 1] += count[i];
        }
        let vt_start = count.clone();
        let mut fill = count;
        let mut vt = vec![0u32; m.tris.len() * 3];
        for (i, t) in m.tris.iter().enumerate() {
            for &v in t {
                vt[fill[v as usize] as usize] = i as u32;
                fill[v as usize] += 1;
            }
        }
        // across each side: the other triangle around the side's first vertex
        // that holds the side reversed (or in the same order on a badly
        // oriented mesh)
        let adj: Vec<[u32; 3]> = (0..m.tris.len())
            .into_par_iter()
            .map(|i| {
                let t = m.tris[i];
                let mut out = [NONE; 3];
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    for &j in &vt[vt_start[a as usize] as usize..vt_start[a as usize + 1] as usize] {
                        if j as usize != i && m.tris[j as usize].contains(&b) {
                            out[k] = j;
                            break;
                        }
                    }
                }
                out
            })
            .collect();
        // vertex neighbours
        let vv_lists: Vec<Vec<u32>> = (0..nv)
            .into_par_iter()
            .map(|v| {
                let mut n: Vec<u32> = vec![];
                for &t in &vt[vt_start[v] as usize..vt_start[v + 1] as usize] {
                    for &w in &m.tris[t as usize] {
                        if w as usize != v && !n.contains(&w) {
                            n.push(w);
                        }
                    }
                }
                n
            })
            .collect();
        let mut vv_start = Vec::with_capacity(nv + 1);
        let mut vv = Vec::with_capacity(nv * 6);
        vv_start.push(0);
        for l in vv_lists {
            vv.extend_from_slice(&l);
            vv_start.push(vv.len() as u32);
        }
        Topo { vt_start, vt, adj, vv_start, vv }
    }

    pub fn tris_of(&self, v: usize) -> &[u32] {
        &self.vt[self.vt_start[v] as usize..self.vt_start[v + 1] as usize]
    }

    pub fn verts_of(&self, v: usize) -> &[u32] {
        &self.vv[self.vv_start[v] as usize..self.vv_start[v + 1] as usize]
    }
}

impl Mesh {
    pub fn tri_pts(&self, t: usize) -> [DVec3; 3] {
        let [a, b, c] = self.tris[t];
        [self.pos[a as usize], self.pos[b as usize], self.pos[c as usize]]
    }

    /// Area-weighted normal (length = 2 × area).
    pub fn tri_cross(&self, t: usize) -> DVec3 {
        let [a, b, c] = self.tri_pts(t);
        (b - a).cross(c - a)
    }

    pub fn tri_normal(&self, t: usize) -> DVec3 {
        self.tri_cross(t).normalize_or_zero()
    }

    pub fn tri_area(&self, t: usize) -> f64 {
        self.tri_cross(t).length() * 0.5
    }

    pub fn tri_centre(&self, t: usize) -> DVec3 {
        let [a, b, c] = self.tri_pts(t);
        (a + b + c) / 3.0
    }

    pub fn normals(&self) -> Vec<DVec3> {
        (0..self.tris.len()).into_par_iter().map(|t| self.tri_normal(t)).collect()
    }

    pub fn centres(&self) -> Vec<DVec3> {
        (0..self.tris.len()).into_par_iter().map(|t| self.tri_centre(t)).collect()
    }

    pub fn areas(&self) -> Vec<f64> {
        (0..self.tris.len()).into_par_iter().map(|t| self.tri_area(t)).collect()
    }

    pub fn bounds(&self) -> (DVec3, DVec3) {
        let mut lo = DVec3::splat(f64::MAX);
        let mut hi = DVec3::splat(f64::MIN);
        for p in &self.pos {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        (lo, hi)
    }

    /// Vertex normals, area-weighted.
    pub fn vertex_normals(&self, topo: &Topo) -> Vec<DVec3> {
        (0..self.pos.len())
            .into_par_iter()
            .map(|v| topo.tris_of(v).iter().map(|&t| self.tri_cross(t as usize)).sum::<DVec3>().normalize_or_zero())
            .collect()
    }

    /// Median edge length (sampled).
    pub fn edge_median(&self) -> f64 {
        let step = (self.tris.len() / 20000).max(1);
        let mut l: Vec<f64> = self.tris.iter().step_by(step).map(|t| (self.pos[t[0] as usize] - self.pos[t[1] as usize]).length()).collect();
        if l.is_empty() {
            return 1.0;
        }
        l.sort_by(|a, b| a.partial_cmp(b).unwrap());
        l[l.len() / 2]
    }

    // ------------------------------------------------------------ .obj

    /// Reads an .obj (v, vt, f; polygons are fanned into triangles). A file
    /// that says it is in centimetres (Maya's header) is scaled to mm.
    pub fn load_obj(path: &Path) -> Result<Mesh, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse_obj(&bytes)
    }

    pub fn parse_obj(bytes: &[u8]) -> Result<Mesh, String> {
        let head = &bytes[..bytes.len().min(4096)];
        let head = String::from_utf8_lossy(head).to_lowercase();
        let unit_mm = if head.contains("centimeter") {
            10.0
        } else if head.contains("meters") && !head.contains("millimeter") {
            1000.0
        } else {
            1.0
        };
        // split in chunks on line ends, parse each in parallel, then join
        let n = rayon::current_num_threads().max(1) * 4;
        let mut cuts = vec![0usize];
        for k in 1..n {
            let mut at = bytes.len() * k / n;
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
            cuts.push((at + 1).min(bytes.len()));
        }
        cuts.push(bytes.len());
        cuts.dedup();
        struct Part {
            v: Vec<DVec3>,
            vt: Vec<[f32; 2]>,
            // faces as (v, vt) corner lists, indices as written (1-based or negative)
            f: Vec<Vec<(i64, i64)>>,
        }
        let parts: Vec<Result<Part, String>> = cuts
            .par_windows(2)
            .map(|w| {
                let mut p = Part { v: vec![], vt: vec![], f: vec![] };
                for line in bytes[w[0]..w[1]].split(|&c| c == b'\n') {
                    let line = line.strip_suffix(b"\r").unwrap_or(line);
                    if line.len() < 2 {
                        continue;
                    }
                    let s = std::str::from_utf8(line).map_err(|_| "not text".to_string())?;
                    let mut it = s.split_ascii_whitespace();
                    match it.next() {
                        Some("v") => {
                            let mut c = [0f64; 3];
                            for x in c.iter_mut() {
                                *x = it.next().and_then(|t| t.parse().ok()).ok_or_else(|| format!("bad vertex: {s}"))?;
                            }
                            p.v.push(DVec3::from(c));
                        }
                        Some("vt") => {
                            let u: f32 = it.next().and_then(|t| t.parse().ok()).unwrap_or(0.0);
                            let v: f32 = it.next().and_then(|t| t.parse().ok()).unwrap_or(0.0);
                            p.vt.push([u, v]);
                        }
                        Some("f") => {
                            let mut corners = vec![];
                            for c in it {
                                let mut sp = c.split('/');
                                let v: i64 = sp.next().and_then(|t| t.parse().ok()).ok_or_else(|| format!("bad face: {s}"))?;
                                let t: i64 = sp.next().and_then(|t| t.parse().ok()).unwrap_or(0);
                                corners.push((v, t));
                            }
                            if corners.len() >= 3 {
                                p.f.push(corners);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(p)
            })
            .collect();
        let mut m = Mesh { unit_mm, ..Default::default() };
        let mut faces = vec![];
        let mut has_uv = true;
        for p in parts {
            let p = p?;
            // negative indices are relative to what is read so far
            let (nv, nt) = (m.pos.len() as i64, m.uv.len() as i64);
            m.pos.extend(p.v.iter().map(|v| *v * unit_mm));
            m.uv.extend(p.vt);
            for f in p.f {
                let fix = |i: i64, n: i64| if i < 0 { n + i } else { i - 1 };
                let f: Vec<(i64, i64)> = f.into_iter().map(|(v, t)| (fix(v, nv + 0), if t == 0 { -1 } else { fix(t, nt) })).collect();
                faces.push(f);
            }
        }
        for f in &faces {
            for k in 1..f.len() - 1 {
                let c = [f[0], f[k], f[k + 1]];
                if c.iter().any(|(v, _)| *v < 0 || *v >= m.pos.len() as i64) {
                    return Err("face index out of range".into());
                }
                m.tris.push(c.map(|(v, _)| v as u32));
                if c.iter().any(|(_, t)| *t < 0 || *t >= m.uv.len() as i64) {
                    has_uv = false;
                }
                m.tri_uv.push(c.map(|(_, t)| t.max(0) as u32));
            }
        }
        if !has_uv || m.uv.is_empty() {
            m.uv.clear();
            m.tri_uv.clear();
        }
        if m.tris.is_empty() {
            return Err("no triangles".into());
        }
        Ok(m)
    }

    /// Writes an .obj in the file's own unit, with its UVs and the vertex
    /// normals given (if any).
    pub fn obj(&self, normals: Option<&[DVec3]>) -> String {
        use std::fmt::Write;
        let k = 1.0 / self.unit_mm;
        let mut s = String::with_capacity(self.pos.len() * 40 + self.tris.len() * 40);
        let unit = if self.unit_mm == 10.0 { "centimeters" } else if self.unit_mm == 1000.0 { "meters" } else { "millimeters" };
        let _ = writeln!(s, "# Forme Rectify\n# This file uses {unit} as units for non-parametric coordinates.");
        for p in &self.pos {
            let _ = writeln!(s, "v {:.6} {:.6} {:.6}", p.x * k, p.y * k, p.z * k);
        }
        for t in &self.uv {
            let _ = writeln!(s, "vt {:.6} {:.6}", t[0], t[1]);
        }
        if let Some(n) = normals {
            for v in n {
                let _ = writeln!(s, "vn {:.4} {:.4} {:.4}", v.x, v.y, v.z);
            }
        }
        let uv = !self.tri_uv.is_empty();
        for (i, t) in self.tris.iter().enumerate() {
            s.push('f');
            for c in 0..3 {
                let v = t[c] + 1;
                match (uv, normals.is_some()) {
                    (true, true) => write!(s, " {v}/{}/{v}", self.tri_uv[i][c] + 1),
                    (true, false) => write!(s, " {v}/{}", self.tri_uv[i][c] + 1),
                    (false, true) => write!(s, " {v}//{v}"),
                    (false, false) => write!(s, " {v}"),
                }
                .ok();
            }
            s.push('\n');
        }
        s
    }
}

/// A uniform grid over points, for neighbours within a radius.
pub struct Grid {
    cell: f64,
    lo: DVec3,
    dims: [usize; 3],
    start: Vec<u32>,
    items: Vec<u32>,
}

impl Grid {
    pub fn new(pts: &[DVec3], cell: f64) -> Grid {
        let mut lo = DVec3::splat(f64::MAX);
        let mut hi = DVec3::splat(f64::MIN);
        for p in pts {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        if pts.is_empty() {
            lo = DVec3::ZERO;
            hi = DVec3::ONE;
        }
        // not too many cells
        let mut cell = cell.max(1e-6);
        let ext = hi - lo;
        while (ext.x / cell + 1.0) * (ext.y / cell + 1.0) * (ext.z / cell + 1.0) > 6.0e7 {
            cell *= 1.5;
        }
        let dims = [(ext.x / cell) as usize + 1, (ext.y / cell) as usize + 1, (ext.z / cell) as usize + 1];
        let n = dims[0] * dims[1] * dims[2];
        let key = |p: &DVec3| -> usize {
            let q = ((*p - lo) / cell).floor();
            let (x, y, z) = ((q.x as usize).min(dims[0] - 1), (q.y as usize).min(dims[1] - 1), (q.z as usize).min(dims[2] - 1));
            (z * dims[1] + y) * dims[0] + x
        };
        let keys: Vec<usize> = pts.par_iter().map(key).collect();
        let mut start = vec![0u32; n + 1];
        for &k in &keys {
            start[k + 1] += 1;
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        let mut fill = start.clone();
        let mut items = vec![0u32; pts.len()];
        for (i, &k) in keys.iter().enumerate() {
            items[fill[k] as usize] = i as u32;
            fill[k] += 1;
        }
        Grid { cell, lo, dims, start, items }
    }

    /// Calls `f` with every item whose cell is within `r` of `p` (the caller
    /// checks the real distance).
    pub fn near(&self, p: DVec3, r: f64, mut f: impl FnMut(u32)) {
        let a = ((p - r - self.lo) / self.cell).floor();
        let b = ((p + r - self.lo) / self.cell).floor();
        let cl = |v: f64, d: usize| (v.max(0.0) as usize).min(d - 1);
        if b.x < 0.0 || b.y < 0.0 || b.z < 0.0 {
            return;
        }
        let (x0, x1) = (cl(a.x, self.dims[0]), cl(b.x, self.dims[0]));
        let (y0, y1) = (cl(a.y, self.dims[1]), cl(b.y, self.dims[1]));
        let (z0, z1) = (cl(a.z, self.dims[2]), cl(b.z, self.dims[2]));
        for z in z0..=z1 {
            for y in y0..=y1 {
                let row = (z * self.dims[1] + y) * self.dims[0];
                for x in x0..=x1 {
                    let k = row + x;
                    for &i in &self.items[self.start[k] as usize..self.start[k + 1] as usize] {
                        f(i);
                    }
                }
            }
        }
    }

    /// The nearest item to `p` within `r` (None if none).
    pub fn nearest(&self, pts: &[DVec3], p: DVec3, r: f64) -> Option<u32> {
        let mut best = (r * r, None);
        self.near(p, r, |i| {
            let d = (pts[i as usize] - p).length_squared();
            if d < best.0 {
                best = (d, Some(i));
            }
        });
        best.1
    }
}

/// Closest point to `p` on the triangle (a, b, c) (Ericson).
pub fn closest_on_tri(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> DVec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    a + ab * v + ac * w
}

/// Symmetric 3×3 eigen decomposition (Jacobi): eigenvalues ascending and
/// their unit eigenvectors.
pub fn eigen_sym(m: [[f64; 3]; 3]) -> ([f64; 3], [DVec3; 3]) {
    let mut a = m;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off < 1e-18 {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-300 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            for k in 0..3 {
                let akp = a[k][p];
                let akq = a[k][q];
                a[k][p] = c * akp - s * akq;
                a[k][q] = s * akp + c * akq;
            }
            for k in 0..3 {
                let apk = a[p][k];
                let aqk = a[q][k];
                a[p][k] = c * apk - s * aqk;
                a[q][k] = s * apk + c * aqk;
            }
            for k in 0..3 {
                let vkp = v[k][p];
                let vkq = v[k][q];
                v[k][p] = c * vkp - s * vkq;
                v[k][q] = s * vkp + c * vkq;
            }
        }
    }
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&i, &j| a[i][i].partial_cmp(&a[j][j]).unwrap_or(std::cmp::Ordering::Equal));
    let vals = idx.map(|i| a[i][i]);
    let vecs = idx.map(|i| DVec3::new(v[0][i], v[1][i], v[2][i]).normalize_or_zero());
    (vals, vecs)
}

/// Outer product accumulation helper.
pub fn add_outer(m: &mut [[f64; 3]; 3], a: DVec3, w: f64) {
    let v = [a.x, a.y, a.z];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] += w * v[i] * v[j];
        }
    }
}

/// Display normals per triangle corner: smooth across the mesh, sharp where
/// the surface turns more than `angle` degrees.
pub fn crease_normals(m: &Mesh, topo: &Topo, angle: f64) -> Vec<[DVec3; 3]> {
    let cos = angle.to_radians().cos();
    let fnorm = m.normals();
    (0..m.tris.len())
        .into_par_iter()
        .map(|t| {
            let n0 = fnorm[t];
            let mut out = [n0; 3];
            for c in 0..3 {
                let v = m.tris[t][c] as usize;
                let mut s = DVec3::ZERO;
                for &o in topo.tris_of(v) {
                    if fnorm[o as usize].dot(n0) >= cos {
                        s += m.tri_cross(o as usize);
                    }
                }
                out[c] = s.normalize_or(n0);
            }
            out
        })
        .collect()
}

/// The same normals for some triangles only (after a hand edit).
pub fn crease_normals_of(m: &Mesh, topo: &Topo, angle: f64, tris: &[u32]) -> Vec<[DVec3; 3]> {
    let cos = angle.to_radians().cos();
    tris.par_iter()
        .map(|&t| {
            let t = t as usize;
            let n0 = m.tri_normal(t);
            let mut out = [n0; 3];
            for c in 0..3 {
                let v = m.tris[t][c] as usize;
                let mut s = DVec3::ZERO;
                for &o in topo.tris_of(v) {
                    let x = m.tri_cross(o as usize);
                    if x.normalize_or_zero().dot(n0) >= cos {
                        s += x;
                    }
                }
                out[c] = s.normalize_or(n0);
            }
            out
        })
        .collect()
}

/// The closest point on the surface: a grid over the triangles' centres.
pub struct Surface {
    grid: Grid,
    centres: Vec<DVec3>,
    /// The largest distance from a triangle's centre to its corners.
    reach: f64,
}

impl Surface {
    pub fn new(m: &Mesh) -> Surface {
        let centres = m.centres();
        let reach = (0..m.tris.len())
            .into_par_iter()
            .map(|t| m.tri_pts(t).iter().map(|p| (*p - centres[t]).length()).fold(0.0, f64::max))
            .reduce(|| 0.0, f64::max);
        let cell = (m.edge_median() * 3.0).max(1e-3);
        Surface { grid: Grid::new(&centres, cell), centres, reach }
    }

    /// The closest point of the surface to `p` within `r`, and its triangle.
    pub fn closest(&self, m: &Mesh, p: DVec3, r: f64) -> Option<(u32, DVec3)> {
        self.closest_where(m, p, r, |_| true)
    }

    /// The same among the triangles that pass `keep`.
    pub fn closest_where(&self, m: &Mesh, p: DVec3, r: f64, keep: impl Fn(u32) -> bool) -> Option<(u32, DVec3)> {
        let mut best: (f64, Option<(u32, DVec3)>) = (r * r, None);
        self.grid.near(p, r + self.reach, |t| {
            if (self.centres[t as usize] - p).length_squared() > (r + self.reach).powi(2) || !keep(t) {
                return;
            }
            let [a, b, c] = m.tri_pts(t as usize);
            let q = closest_on_tri(p, a, b, c);
            let d = (q - p).length_squared();
            if d < best.0 {
                best = (d, Some((t, q)));
            }
        });
        best.1
    }
}

impl Surface {
    /// Where the line through `p` along `d` (unit) meets the surface within
    /// `r` of `p`, either way, the nearest to `p` first, on the triangles
    /// that pass `keep`: the triangle and how far along `d` (signed).
    pub fn hit_along(&self, m: &Mesh, p: DVec3, d: DVec3, r: f64, keep: impl Fn(u32) -> bool) -> Option<(u32, f64)> {
        let cell = self.grid.cell;
        let n = ((2.0 * r / cell).ceil() as usize).max(1);
        let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut best: Option<(u32, f64)> = None;
        for k in 0..=n {
            let q = p + d * (-r + 2.0 * r * k as f64 / n as f64);
            self.grid.near(q, cell * 0.87 + self.reach, |t| {
                if !seen.insert(t) || !keep(t) {
                    return;
                }
                let [a, b, c] = m.tri_pts(t as usize);
                if let Some(s) = line_tri(p, d, a, b, c) {
                    if s.abs() <= r && best.map(|bb| s.abs() < bb.1.abs()).unwrap_or(true) {
                        best = Some((t, s));
                    }
                }
            });
        }
        best
    }
}

/// Where the line `o + s d` crosses the triangle (any sign of `s`).
fn line_tri(o: DVec3, d: DVec3, a: DVec3, b: DVec3, c: DVec3) -> Option<f64> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-14 {
        return None;
    }
    let inv = 1.0 / det;
    let t = o - a;
    let u = t.dot(p) * inv;
    if !(-1e-9..=1.0 + 1e-9).contains(&u) {
        return None;
    }
    let q = t.cross(e1);
    let v = d.dot(q) * inv;
    if v < -1e-9 || u + v > 1.0 + 1e-9 {
        return None;
    }
    Some(e2.dot(q) * inv)
}

/// A plane through the points (weighted): its centre and unit normal (the
/// direction of least spread), or None with too few points.
pub fn fit_plane(pts: &[(DVec3, f64)]) -> Option<(DVec3, DVec3)> {
    let w: f64 = pts.iter().map(|p| p.1).sum();
    if pts.len() < 3 || w <= 0.0 {
        return None;
    }
    let c = pts.iter().map(|(p, w)| *p * *w).sum::<DVec3>() / w;
    let mut cov = [[0.0; 3]; 3];
    for (p, wi) in pts {
        add_outer(&mut cov, *p - c, *wi);
    }
    let (_, v) = eigen_sym(cov);
    Some((c, v[0]))
}
