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

/// The least-squares solution of `a x = b` (a symmetric, positive) using
/// only the eigen directions whose singular value is at least `tau` of the
/// largest (Ju et al.'s truncated pseudo-inverse).
fn qef_truncated(a: Mat3, b: Vec3, tau: f32) -> Vec3 {
    let (e, v) = eigen_sym(a);
    let top = e[0].max(e[1]).max(e[2]).max(1e-12);
    let mut x = Vec3::ZERO;
    for i in 0..3 {
        if e[i] > top * tau * tau {
            let u = v.col(i);
            x += u * (u.dot(b) / e[i]);
        }
    }
    x
}

/// Eigenvalues and eigenvectors (columns) of a symmetric 3 × 3 matrix,
/// by Jacobi rotations.
fn eigen_sym(m: Mat3) -> ([f32; 3], Mat3) {
    let mut a = [[0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            a[i][j] = m.col(j)[i];
        }
    }
    let mut v = [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..12 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off < 1e-9 {
            break;
        }
        for (p, q) in [(0usize, 1usize), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-12 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            for k in 0..3 {
                let (akp, akq) = (a[k][p], a[k][q]);
                a[k][p] = c * akp - s * akq;
                a[k][q] = s * akp + c * akq;
            }
            for k in 0..3 {
                let (apk, aqk) = (a[p][k], a[q][k]);
                a[p][k] = c * apk - s * aqk;
                a[q][k] = s * apk + c * aqk;
            }
            for k in 0..3 {
                let (vkp, vkq) = (v[k][p], v[k][q]);
                v[k][p] = c * vkp - s * vkq;
                v[k][q] = s * vkp + c * vkq;
            }
        }
    }
    let cols = Mat3::from_cols(
        Vec3::new(v[0][0], v[1][0], v[2][0]),
        Vec3::new(v[0][1], v[1][1], v[2][1]),
        Vec3::new(v[0][2], v[1][2], v[2][2]),
    );
    ([a[0][0], a[1][1], a[2][2]], cols)
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
    // each cell's vertex from its own edges' planes (the planes kept, and
    // whether they saw one surface only)
    let solved: Vec<(Vec3, bool, Vec<(Vec3, Vec3)>)> = par_map(cells.len(), |ci| {
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
        let mut planes = Vec::with_capacity(4);
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
            planes.push((p, nrm));
            mass += p;
            count += 1.0;
        }
        mass /= count.max(1.0);
        // the planes' point nearest the edges' mean, only along the
        // directions the normals really span (a crease, a corner): a
        // curved patch (a blend, a ball) whose normals turn a little would
        // otherwise throw the vertex far off and grid-aligned steps showed
        let mut v = mass + qef_truncated(ata, atb - ata * mass, 0.12);
        let lo = corner_p[0];
        let hi = corner_p[7];
        let in_box = |p: Vec3, m: f32| p.cmpge(lo - Vec3::splat(m)).all() && p.cmple(hi + Vec3::splat(m)).all();
        // a crease or a corner found on the surface a little past the cell
        // (the material's wedge slipping between the grid's corners, its
        // tip in the next cell, which then has no vertex): taken there, else
        // the triangles cut across the rim and left teeth along it
        let sharp = v.is_finite() && in_box(v, cell * 1.5) && prog.dist(v).abs() < cell * 0.05;
        if !sharp {
            let cmin = lo - Vec3::splat(cell * 0.25);
            let cmax = hi + Vec3::splat(cell * 0.25);
            if !v.is_finite() {
                v = mass;
            }
            v = v.clamp(cmin, cmax);
            // back onto the surface (one clamped off it stood out as a spike)
            for _ in 0..3 {
                let d = prog.dist(v);
                if d.abs() < cell * 1e-3 {
                    break;
                }
                let g = prog.normal(v, cell * 0.05);
                if !g.is_finite() || g == Vec3::ZERO {
                    break;
                }
                let w = v - g * d;
                if !w.is_finite() {
                    break;
                }
                v = w.clamp(cmin - Vec3::splat(cell * 0.25), cmax + Vec3::splat(cell * 0.25));
            }
        }
        let (e, _) = eigen_sym(ata);
        let top = e[0].max(e[1]).max(e[2]);
        let mid = e[0] + e[1] + e[2] - top - e[0].min(e[1]).min(e[2]);
        (v, !sharp && mid <= top * 0.1 * 0.1, planes)
    });

    let mut cell_vert = vec![u32::MAX; n[0] * n[1] * n[2]];
    for (ci, (i, j, k)) in cells.iter().enumerate() {
        cell_vert[g.cell_index(*i, *j, *k)] = ci as u32;
    }
    // a cell that saw one surface only, the crease passing through it all
    // the same (its edges all on one side of it): its vertex stayed on that
    // surface and the triangles from it to the far side of the crease cut
    // across the rim (thin teeth). Its neighbours' planes find the crease;
    // taken when it lies in the cell and on the surface.
    let finals: Vec<(Vec3, Vec3, u32)> = par_map(cells.len(), |ci| {
        let (mut v, flat, _) = solved[ci];
        if flat {
            let (i, j, k) = cells[ci];
            let mut ata = Mat3::ZERO;
            let mut atb = Vec3::ZERO;
            for dk in -1isize..=1 {
                for dj in -1isize..=1 {
                    for di in -1isize..=1 {
                        let (ii, jj, kk) = (i as isize + di, j as isize + dj, k as isize + dk);
                        if ii < 0 || jj < 0 || kk < 0 || ii as usize >= n[0] || jj as usize >= n[1] || kk as usize >= n[2] {
                            continue;
                        }
                        let c = cell_vert[g.cell_index(ii as usize, jj as usize, kk as usize)];
                        if c == u32::MAX {
                            continue;
                        }
                        for (p, nrm) in &solved[c as usize].2 {
                            ata += Mat3::from_cols(*nrm * nrm.x, *nrm * nrm.y, *nrm * nrm.z);
                            atb += *nrm * nrm.dot(*p);
                        }
                    }
                }
            }
            let (e, _) = eigen_sym(ata);
            let top = e[0].max(e[1]).max(e[2]);
            let mid = e[0] + e[1] + e[2] - top - e[0].min(e[1]).min(e[2]);
            if mid > top * 0.08 * 0.08 {
                let mut w = v + qef_truncated(ata, atb - ata * v, 0.08);
                // the planes of curved faces meet a little off them: back on
                if w.is_finite() && prog.dist(w).abs() < cell * 0.2 {
                    for _ in 0..2 {
                        let d = prog.dist(w);
                        let gr = prog.normal(w, cell * 0.02);
                        if gr.is_finite() {
                            w -= gr * d;
                        }
                    }
                    let lo = g.corner_pos(i, j, k);
                    let hi = lo + Vec3::splat(cell);
                    let m = Vec3::splat(cell * 0.25);
                    if w.is_finite() && w.cmpge(lo - m).all() && w.cmple(hi + m).all() && prog.dist(w).abs() < cell * 0.02 {
                        v = w;
                    }
                }
            }
        }
        let nrm = prog.normal(v, cell * 0.25);
        let mat = prog.eval(v).1;
        (v, nrm, mat)
    });
    let mut mesh = Mesh::default();
    for (p, nrm, m) in finals {
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
                // split along the diagonal whose two triangles follow the
                // field's normals best (along a crease: the diagonal on the
                // crease, else a tooth across it), else the shorter one
                let pa = mesh.positions[a as usize];
                let pb = mesh.positions[b as usize];
                let pc = mesh.positions[cc as usize];
                let pd = mesh.positions[d as usize];
                let nr = |i: u32| mesh.normals[i as usize];
                let fit = |p: [Vec3; 3], n: [Vec3; 3]| -> f32 {
                    let f = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
                    n.iter().map(|m| f.dot(*m)).fold(f32::MAX, f32::min)
                };
                let ac = fit([pa, pb, pc], [nr(a), nr(b), nr(cc)]).min(fit([pa, pc, pd], [nr(a), nr(cc), nr(d)]));
                let bd = fit([pa, pb, pd], [nr(a), nr(b), nr(d)]).min(fit([pb, pc, pd], [nr(b), nr(cc), nr(d)]));
                let shorter_ac = pa.distance_squared(pc) <= pb.distance_squared(pd);
                if (ac - bd).abs() > 0.05 && ac > bd || (ac - bd).abs() <= 0.05 && shorter_ac {
                    mesh.indices.extend_from_slice(&[a, b, cc, a, cc, d]);
                } else {
                    mesh.indices.extend_from_slice(&[a, b, d, b, cc, d]);
                }
            }
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eigen_of_a_symmetric_matrix() {
        let m = Mat3::from_cols(Vec3::new(4.0, 1.0, 0.5), Vec3::new(1.0, 3.0, 0.2), Vec3::new(0.5, 0.2, 1.0));
        let (e, v) = eigen_sym(m);
        for i in 0..3 {
            let u = v.col(i);
            assert!((m * u - u * e[i]).length() < 1e-4, "{i}: {e:?}");
            assert!((u.length() - 1.0).abs() < 1e-4);
        }
        // a crease: two planes, the point on both kept, the third way left free
        let n1 = Vec3::X;
        let n2 = Vec3::Y;
        let p = Vec3::new(1.0, 2.0, 0.0);
        let ata = Mat3::from_cols(n1 * n1.x, n1 * n1.y, n1 * n1.z) + Mat3::from_cols(n2 * n2.x, n2 * n2.y, n2 * n2.z);
        let atb = n1 * n1.dot(p) + n2 * n2.dot(p);
        let mass = Vec3::new(0.5, 1.5, 3.0);
        let x = mass + qef_truncated(ata, atb - ata * mass, 0.12);
        assert!((x - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-4, "{x}");
    }
}
