//! Export meshing: dual contouring on a narrow band of the distance field.
//! Vertices are placed by a regularised QEF on the exact SDF normals, which
//! keeps hard-surface edges sharp instead of rounding them like marching cubes.

use crate::program::Program;
use glam::{Mat3, Vec3};

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub materials: Vec<u32>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

const BLOCK: usize = 8;

struct Grid {
    lo: Vec3,
    cell: f32,
    n: [usize; 3], // cells per axis
}

impl Grid {
    fn corner_index(&self, i: usize, j: usize, k: usize) -> usize {
        (k * (self.n[1] + 1) + j) * (self.n[0] + 1) + i
    }
    fn cell_index(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.n[1] + j) * self.n[0] + i
    }
    fn corner_pos(&self, i: usize, j: usize, k: usize) -> Vec3 {
        self.lo + Vec3::new(i as f32, j as f32, k as f32) * self.cell
    }
}

fn par_map<T: Send, F: Fn(usize) -> T + Sync>(count: usize, f: F) -> Vec<T> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(16);
    if count < 2048 || threads == 1 {
        return (0..count).map(&f).collect();
    }
    let chunk = count.div_ceil(threads);
    let f = &f;
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                s.spawn(move || {
                    let start = t * chunk;
                    let end = ((t + 1) * chunk).min(count);
                    (start..end).map(f).collect::<Vec<T>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

/// Meshes the program inside [lo, hi] with cubic cells of size `cell`.
pub fn dual_contour(prog: &Program, lo: Vec3, hi: Vec3, cell: f32) -> Mesh {
    let pad = cell * 2.0;
    let lo = lo - Vec3::splat(pad);
    let hi = hi + Vec3::splat(pad);
    let ext = hi - lo;
    let mut n = [0usize; 3];
    for a in 0..3 {
        n[a] = ((ext[a] / cell).ceil() as usize).max(2);
        n[a] = n[a].div_ceil(BLOCK) * BLOCK;
    }
    let g = Grid { lo, cell, n };
    let nb = [n[0] / BLOCK, n[1] / BLOCK, n[2] / BLOCK];
    let block_count = nb[0] * nb[1] * nb[2];

    // 1. active blocks (narrow band)
    let half_diag = cell * BLOCK as f32 * 0.5 * 3f32.sqrt();
    let active: Vec<bool> = par_map(block_count, |b| {
        let bi = b % nb[0];
        let bj = (b / nb[0]) % nb[1];
        let bk = b / (nb[0] * nb[1]);
        let c = g.lo + (Vec3::new(bi as f32, bj as f32, bk as f32) + Vec3::splat(0.5)) * cell * BLOCK as f32;
        prog.dist(c).abs() <= half_diag * 1.5 + cell
    });

    // 2. corners needed
    let corner_count = (n[0] + 1) * (n[1] + 1) * (n[2] + 1);
    let mut needed = vec![false; corner_count];
    for b in 0..block_count {
        if !active[b] {
            continue;
        }
        let bi = b % nb[0];
        let bj = (b / nb[0]) % nb[1];
        let bk = b / (nb[0] * nb[1]);
        for k in bk * BLOCK..=(bk + 1) * BLOCK {
            for j in bj * BLOCK..=(bj + 1) * BLOCK {
                for i in bi * BLOCK..=(bi + 1) * BLOCK {
                    needed[g.corner_index(i, j, k)] = true;
                }
            }
        }
    }
    let list: Vec<usize> = (0..corner_count).filter(|c| needed[*c]).collect();
    let values: Vec<f32> = par_map(list.len(), |idx| {
        let c = list[idx];
        let i = c % (n[0] + 1);
        let j = (c / (n[0] + 1)) % (n[1] + 1);
        let k = c / ((n[0] + 1) * (n[1] + 1));
        prog.dist(g.corner_pos(i, j, k))
    });
    let mut field = vec![f32::NAN; corner_count];
    for (idx, c) in list.iter().enumerate() {
        field[*c] = values[idx];
    }

    // 3. cells with a sign change → one vertex each
    let mut cells: Vec<(usize, usize, usize)> = vec![];
    for b in 0..block_count {
        if !active[b] {
            continue;
        }
        let bi = b % nb[0];
        let bj = (b / nb[0]) % nb[1];
        let bk = b / (nb[0] * nb[1]);
        for k in bk * BLOCK..(bk + 1) * BLOCK {
            for j in bj * BLOCK..(bj + 1) * BLOCK {
                for i in bi * BLOCK..(bi + 1) * BLOCK {
                    let mut inside = 0;
                    for c in 0..8 {
                        let v = field[g.corner_index(i + (c & 1), j + ((c >> 1) & 1), k + ((c >> 2) & 1))];
                        if v < 0.0 {
                            inside += 1;
                        }
                    }
                    if inside > 0 && inside < 8 {
                        cells.push((i, j, k));
                    }
                }
            }
        }
    }

    const EDGES: [(usize, usize); 12] = [
        (0, 1), (2, 3), (4, 5), (6, 7),
        (0, 2), (1, 3), (4, 6), (5, 7),
        (0, 4), (1, 5), (2, 6), (3, 7),
    ];
    let solved: Vec<(Vec3, Vec3, u32)> = par_map(cells.len(), |ci| {
        let (i, j, k) = cells[ci];
        let mut corner_p = [Vec3::ZERO; 8];
        let mut corner_v = [0f32; 8];
        for c in 0..8 {
            let (ii, jj, kk) = (i + (c & 1), j + ((c >> 1) & 1), k + ((c >> 2) & 1));
            corner_p[c] = g.corner_pos(ii, jj, kk);
            corner_v[c] = field[g.corner_index(ii, jj, kk)];
        }
        let mut ata = Mat3::ZERO;
        let mut atb = Vec3::ZERO;
        let mut mass = Vec3::ZERO;
        let mut count: f32 = 0.0;
        for (a, b) in EDGES {
            let (va, vb) = (corner_v[a], corner_v[b]);
            if (va < 0.0) == (vb < 0.0) {
                continue;
            }
            let mut t = va / (va - vb);
            // one secant refinement on the true field
            let p0 = corner_p[a].lerp(corner_p[b], t);
            let d0 = prog.dist(p0);
            let dir = corner_p[b] - corner_p[a];
            let len = dir.length();
            if len > 0.0 {
                let slope = (vb - va) / len;
                if slope.abs() > 1e-6 {
                    t = (t - d0 / slope / len).clamp(0.0, 1.0);
                }
            }
            let p = corner_p[a].lerp(corner_p[b], t);
            let nrm = prog.normal(p, cell * 0.1);
            ata += Mat3::from_cols(nrm * nrm.x, nrm * nrm.y, nrm * nrm.z);
            atb += nrm * nrm.dot(p);
            mass += p;
            count += 1.0;
        }
        mass /= count.max(1.0);
        let lambda = 0.08 * count.max(1.0) * 0.25;
        let m = ata + Mat3::from_diagonal(Vec3::splat(lambda));
        let rhs = atb + mass * lambda;
        let mut v = if m.determinant().abs() > 1e-9 { m.inverse() * rhs } else { mass };
        let cmin = corner_p[0] - Vec3::splat(cell * 0.25);
        let cmax = corner_p[7] + Vec3::splat(cell * 0.25);
        if !v.is_finite() {
            v = mass;
        }
        v = v.clamp(cmin, cmax);
        let nrm = prog.normal(v, cell * 0.25);
        let mat = prog.eval(v).1;
        (v, nrm, mat)
    });

    let mut cell_vert = vec![u32::MAX; n[0] * n[1] * n[2]];
    let mut mesh = Mesh::default();
    for (ci, (i, j, k)) in cells.iter().enumerate() {
        cell_vert[g.cell_index(*i, *j, *k)] = ci as u32;
        let (p, nrm, m) = solved[ci];
        mesh.positions.push(p);
        mesh.normals.push(nrm);
        mesh.materials.push(m);
    }

    // 4. one quad per sign-changing edge, joining the 4 cells around it
    let cv = |i: isize, j: isize, k: isize| -> Option<u32> {
        if i < 0 || j < 0 || k < 0 || i as usize >= n[0] || j as usize >= n[1] || k as usize >= n[2] {
            return None;
        }
        let v = cell_vert[g.cell_index(i as usize, j as usize, k as usize)];
        (v != u32::MAX).then_some(v)
    };
    for &c in &list {
        let i = c % (n[0] + 1);
        let j = (c / (n[0] + 1)) % (n[1] + 1);
        let k = c / ((n[0] + 1) * (n[1] + 1));
        let v0 = field[c];
        for axis in 0..3 {
            let (ni, nj, nk) = match axis {
                0 => (i + 1, j, k),
                1 => (i, j + 1, k),
                _ => (i, j, k + 1),
            };
            if ni > n[0] || nj > n[1] || nk > n[2] {
                continue;
            }
            let v1 = field[g.corner_index(ni, nj, nk)];
            if v1.is_nan() || (v0 < 0.0) == (v1 < 0.0) {
                continue;
            }
            let (i, j, k) = (i as isize, j as isize, k as isize);
            let quad = match axis {
                0 => [cv(i, j - 1, k - 1), cv(i, j, k - 1), cv(i, j, k), cv(i, j - 1, k)],
                1 => [cv(i - 1, j, k - 1), cv(i - 1, j, k), cv(i, j, k), cv(i, j, k - 1)],
                _ => [cv(i - 1, j - 1, k), cv(i, j - 1, k), cv(i, j, k), cv(i - 1, j, k)],
            };
            if let [Some(a), Some(b), Some(cc), Some(d)] = quad {
                let outward_pos = v0 < 0.0;
                let (a, b, cc, d) = if outward_pos { (a, b, cc, d) } else { (a, d, cc, b) };
                // split along the shorter diagonal
                let pa = mesh.positions[a as usize];
                let pb = mesh.positions[b as usize];
                let pc = mesh.positions[cc as usize];
                let pd = mesh.positions[d as usize];
                if pa.distance_squared(pc) <= pb.distance_squared(pd) {
                    mesh.indices.extend_from_slice(&[a, b, cc, a, cc, d]);
                } else {
                    mesh.indices.extend_from_slice(&[a, b, d, b, cc, d]);
                }
            }
        }
    }
    mesh
}
