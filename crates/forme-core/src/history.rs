//! Undo / redo by scene snapshots (scenes are small: a few kB of parameters).

use crate::scene::Scene;

#[derive(Default, Clone)]
pub struct History {
    undo: Vec<(String, Scene)>,
    redo: Vec<(String, Scene)>,
    limit: usize,
}

impl History {
    pub fn new() -> Self {
        History { undo: vec![], redo: vec![], limit: 200 }
    }

    /// Call before mutating the scene.
    pub fn record(&mut self, label: &str, before: &Scene) {
        if self.undo.last().map(|(_, s)| s == before).unwrap_or(false) {
            return;
        }
        self.undo.push((label.to_string(), before.clone()));
        if self.undo.len() > self.limit.max(1) {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: &mut Scene) -> Option<String> {
        let (label, prev) = self.undo.pop()?;
        let now = std::mem::replace(current, prev);
        self.redo.push((label.clone(), now));
        Some(label)
    }

    pub fn redo(&mut self, current: &mut Scene) -> Option<String> {
        let (label, next) = self.redo.pop()?;
        let now = std::mem::replace(current, next);
        self.undo.push((label.clone(), now));
        Some(label)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
