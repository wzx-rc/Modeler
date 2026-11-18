//! A hand edit: the vertices it moves, from where to where. The triangles
//! never change, so undo and redo only put positions back.

use crate::mesh::Mesh;
use glam::DVec3;

#[derive(Clone, Default, Debug)]
pub struct Moves {
    pub idx: Vec<u32>,
    pub from: Vec<DVec3>,
    pub to: Vec<DVec3>,
}

impl Moves {
    /// From targets (vertex, new position); vertices that do not move are
    /// left out.
    pub fn new(m: &Mesh, targets: impl IntoIterator<Item = (u32, DVec3)>) -> Moves {
        let mut out = Moves::default();
        for (v, p) in targets {
            let from = m.pos[v as usize];
            if (p - from).length_squared() > 1e-16 && p.is_finite() {
                out.idx.push(v);
                out.from.push(from);
                out.to.push(p);
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }

    pub fn apply(&self, m: &mut Mesh) {
        for (i, &v) in self.idx.iter().enumerate() {
            m.pos[v as usize] = self.to[i];
        }
    }

    pub fn undo(&self, m: &mut Mesh) {
        for (i, &v) in self.idx.iter().enumerate() {
            m.pos[v as usize] = self.from[i];
        }
    }

    /// The largest move (mm).
    pub fn max_move(&self) -> f64 {
        self.from.iter().zip(&self.to).map(|(a, b)| (*a - *b).length()).fold(0.0, f64::max)
    }
}
