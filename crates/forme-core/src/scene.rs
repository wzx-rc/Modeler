//! Scene model: a tree of parametric shapes combined in order (Add, Carve, ...).
//!
//! Units are millimetres, Y is up. Every node keeps all its parameters, so the
//! scene stays fully editable (non destructive).

use glam::{Mat4, Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type NodeId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PrimKind {
    Box,
    Cylinder,
    Sphere,
    Capsule,
    Torus,
    Cone,
    Prism,
    Tube,
    Profile,
}

impl PrimKind {
    pub const PLACEABLE: [PrimKind; 8] = [
        PrimKind::Box,
        PrimKind::Cylinder,
        PrimKind::Sphere,
        PrimKind::Capsule,
        PrimKind::Torus,
        PrimKind::Cone,
        PrimKind::Prism,
        PrimKind::Tube,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PrimKind::Box => "Cube",
            PrimKind::Cylinder => "Cylindre",
            PrimKind::Sphere => "Sphère",
            PrimKind::Capsule => "Capsule",
            PrimKind::Torus => "Tore",
            PrimKind::Cone => "Cône",
            PrimKind::Prism => "Prisme",
            PrimKind::Tube => "Tube",
            PrimKind::Profile => "Profil",
        }
    }

    pub fn code(self) -> u32 {
        match self {
            PrimKind::Box => 0,
            PrimKind::Cylinder => 1,
            PrimKind::Sphere => 2,
            PrimKind::Capsule => 3,
            PrimKind::Torus => 4,
            PrimKind::Cone => 5,
            PrimKind::Prism => 6,
            PrimKind::Tube => 7,
            PrimKind::Profile => 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileMode {
    Extrude,
    Revolve,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfilePoint {
    pub pos: Vec2,
    /// Smooth points are rounded into curves, sharp points stay corners.
    pub smooth: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub points: Vec<ProfilePoint>,
    pub mode: ProfileMode,
}

impl Profile {
    /// Closed polyline actually evaluated: smooth points are replaced by a
    /// quadratic curve between the midpoints of their two edges.
    pub fn polyline(&self) -> Vec<Vec2> {
        let n = self.points.len();
        if n < 3 {
            return self.points.iter().map(|p| p.pos).collect();
        }
        let mut out = Vec::with_capacity(n * 6);
        for i in 0..n {
            let prev = self.points[(i + n - 1) % n].pos;
            let cur = self.points[i];
            let next = self.points[(i + 1) % n].pos;
            if cur.smooth {
                let a = (prev + cur.pos) * 0.5;
                let b = (cur.pos + next) * 0.5;
                const STEPS: usize = 6;
                for s in 0..=STEPS {
                    let t = s as f32 / STEPS as f32;
                    let p = a * (1.0 - t) * (1.0 - t) + cur.pos * 2.0 * (1.0 - t) * t + b * t * t;
                    out.push(p);
                }
            } else {
                out.push(cur.pos);
            }
        }
        out.dedup_by(|a, b| a.distance(*b) < 1e-4);
        out
    }
}

/// Parameters of a primitive. The meaning of `size` depends on the kind:
/// Box: half extents · Cylinder: (radius, half height, -) · Sphere: radii ·
/// Capsule: (radius, half length, -) · Torus: (major radius, -, minor radius) ·
/// Cone: (bottom radius, half height, top radius) · Prism: (radius, half height, -) ·
/// Tube: (outer radius, half height, inner radius) · Profile: (-, -, half thickness).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub kind: PrimKind,
    pub size: Vec3,
    pub round: f32,
    pub chamfer: f32,
    pub taper: f32,
    pub shell: f32,
    pub sides: u32,
    pub profile: Option<Profile>,
    /// Per-edge fillets (box: 12 edges, cylinder: top and bottom rims), in mm;
    /// a negative value is a chamfer of that size. Empty: every edge uses
    /// `round` / `chamfer`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<f32>,
}

impl Shape {
    /// Edges that can carry their own fillet (box edges, cylinder rims).
    pub fn edge_count(&self) -> usize {
        match self.kind {
            PrimKind::Box => 12,
            PrimKind::Cylinder => 2,
            _ => 0,
        }
    }

    /// Fillet of edge `i` (negative: chamfer).
    pub fn edge_radius(&self, i: usize) -> f32 {
        match self.edges.get(i) {
            Some(r) => *r,
            None if self.chamfer > 0.0 => -self.chamfer,
            None => self.round,
        }
    }

    /// Sets the fillet of one edge (negative: chamfer); back to the uniform
    /// `round` / `chamfer` when every edge ends up the same.
    pub fn set_edge(&mut self, i: usize, r: f32) {
        let n = self.edge_count();
        if i >= n {
            return;
        }
        if self.edges.len() != n {
            self.edges = (0..n).map(|k| self.edge_radius(k)).collect();
        }
        self.edges[i] = r;
        let first = self.edges[0];
        if self.edges.iter().all(|e| (*e - first).abs() < 1e-6) {
            self.set_all_edges(first.max(0.0), (-first).max(0.0));
        } else {
            self.round = 0.0;
            self.chamfer = 0.0;
        }
    }

    /// Same fillet on every edge.
    pub fn set_all_edges(&mut self, round: f32, chamfer: f32) {
        self.edges.clear();
        self.round = round;
        self.chamfer = chamfer;
    }

    pub fn new(kind: PrimKind) -> Self {
        let size = match kind {
            PrimKind::Box => Vec3::new(10.0, 10.0, 10.0),
            PrimKind::Cylinder => Vec3::new(10.0, 10.0, 0.0),
            PrimKind::Sphere => Vec3::splat(10.0),
            PrimKind::Capsule => Vec3::new(5.0, 8.0, 0.0),
            PrimKind::Torus => Vec3::new(10.0, 0.0, 3.0),
            PrimKind::Cone => Vec3::new(10.0, 10.0, 3.0),
            PrimKind::Prism => Vec3::new(10.0, 10.0, 0.0),
            PrimKind::Tube => Vec3::new(10.0, 10.0, 7.0),
            PrimKind::Profile => Vec3::new(0.0, 0.0, 4.0),
        };
        Shape {
            kind,
            size,
            round: 0.0,
            chamfer: 0.0,
            taper: 0.0,
            shell: 0.0,
            sides: 6,
            profile: None,
            edges: vec![],
        }
    }

    /// Main "radius-like" size shown by the radius handle.
    pub fn radius(&self) -> f32 {
        match self.kind {
            PrimKind::Box => self.size.x,
            _ => self.size.x,
        }
    }

    /// Half height along local Y (or Z for profiles).
    pub fn half_height(&self) -> f32 {
        match self.kind {
            PrimKind::Torus => self.size.z,
            PrimKind::Sphere => self.size.y,
            PrimKind::Profile => self.size.z,
            _ => self.size.y,
        }
    }

    /// Conservative local bounding box (min, max).
    pub fn local_bounds(&self) -> (Vec3, Vec3) {
        if let (PrimKind::Profile, Some(p)) = (self.kind, self.profile.as_ref()) {
            let pts = p.polyline();
            let mut lo = Vec2::splat(f32::MAX);
            let mut hi = Vec2::splat(f32::MIN);
            for q in &pts {
                lo = lo.min(*q);
                hi = hi.max(*q);
            }
            if pts.is_empty() {
                return (Vec3::splat(-1.0), Vec3::splat(1.0));
            }
            let pad = Vec3::splat(self.shell.max(0.0) + 0.5);
            return match p.mode {
                ProfileMode::Revolve => {
                    let r = lo.x.abs().max(hi.x.abs());
                    (Vec3::new(-r, lo.y, -r) - pad, Vec3::new(r, hi.y, r) + pad)
                }
                ProfileMode::Extrude => {
                    (Vec3::new(lo.x, lo.y, -self.size.z) - pad, Vec3::new(hi.x, hi.y, self.size.z) + pad)
                }
            };
        }
        let he = self.local_half_extents();
        (-he, he)
    }

    /// Conservative local bounding box (half extents).
    pub fn local_half_extents(&self) -> Vec3 {
        let s = self.size;
        let base = match self.kind {
            PrimKind::Box | PrimKind::Sphere => s,
            PrimKind::Cylinder | PrimKind::Prism => Vec3::new(s.x, s.y, s.x),
            PrimKind::Tube => Vec3::new(s.x, s.y, s.x),
            PrimKind::Capsule => Vec3::new(s.x, s.y + s.x, s.x),
            PrimKind::Torus => Vec3::new(s.x + s.z, s.z, s.x + s.z),
            PrimKind::Cone => {
                let r = s.x.max(s.z);
                Vec3::new(r, s.y, r)
            }
            PrimKind::Profile => {
                let pts = self.profile.as_ref().map(|p| p.polyline()).unwrap_or_default();
                let mut m = Vec2::ZERO;
                for p in &pts {
                    m = m.max(p.abs());
                }
                match self.profile.as_ref().map(|p| p.mode) {
                    Some(ProfileMode::Revolve) => Vec3::new(m.x, m.y, m.x),
                    _ => Vec3::new(m.x, m.y, s.z),
                }
            }
        };
        base + Vec3::splat(self.shell.max(0.0) + 0.5)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CombineMode {
    Add,
    Carve,
    Intersect,
    Groove,
    Tongue,
}

impl CombineMode {
    pub const ALL: [CombineMode; 5] = [
        CombineMode::Add,
        CombineMode::Carve,
        CombineMode::Intersect,
        CombineMode::Groove,
        CombineMode::Tongue,
    ];
    pub fn label(self) -> &'static str {
        match self {
            CombineMode::Add => "Add",
            CombineMode::Carve => "Carve",
            CombineMode::Intersect => "Intersect",
            CombineMode::Groove => "Groove",
            CombineMode::Tongue => "Tongue",
        }
    }
    pub fn code(self) -> u32 {
        match self {
            CombineMode::Add => 0,
            CombineMode::Carve => 1,
            CombineMode::Intersect => 2,
            CombineMode::Groove => 3,
            CombineMode::Tongue => 4,
        }
    }
    /// True for modes whose primitive removes or marks matter (shown as a ghost).
    pub fn is_cutter(self) -> bool {
        !matches!(self, CombineMode::Add)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendKind {
    Smooth,
    Chamfer,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Combine {
    pub mode: CombineMode,
    pub blend: f32,
    pub kind: BlendKind,
    /// Groove / Tongue depth.
    pub depth: f32,
    /// Groove / Tongue width.
    pub width: f32,
}

impl Default for Combine {
    fn default() -> Self {
        Combine { mode: CombineMode::Add, blend: 0.0, kind: BlendKind::Smooth, depth: 1.0, width: 0.6 }
    }
}

impl Combine {
    pub fn with_mode(mode: CombineMode) -> Self {
        Combine { mode, ..Default::default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn index(self) -> u32 {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }
    pub fn vec(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum Repeat {
    #[default]
    None,
    /// `count` copies around `axis` (of the parent space), the first one where the node is.
    Radial { count: u32, axis: Axis },
    /// `count` copies, each shifted by `step` (parent space).
    Linear { count: u32, step: Vec3 },
}

impl Repeat {
    pub fn count(&self) -> u32 {
        match self {
            Repeat::None => 1,
            Repeat::Radial { count, .. } | Repeat::Linear { count, .. } => *count,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Mirror {
    pub x: bool,
    pub y: bool,
    pub z: bool,
}

impl Mirror {
    pub fn any(&self) -> bool {
        self.x || self.y || self.z
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Transform { pos: Vec3::ZERO, rot: Quat::IDENTITY, scale: 1.0 }
    }
}

impl Transform {
    pub fn at(pos: Vec3) -> Self {
        Transform { pos, ..Default::default() }
    }
    pub fn with_rot(mut self, rot: Quat) -> Self {
        self.rot = rot;
        self
    }
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(Vec3::splat(self.scale), self.rot, self.pos)
    }
    pub fn is_identity(&self) -> bool {
        self.pos == Vec3::ZERO && self.rot == Quat::IDENTITY && self.scale == 1.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    Shape(Shape),
    /// `combined`: children are merged first, then the group combines as one shape.
    /// Otherwise children act directly on the parent's result (pure organisation).
    Group { children: Vec<NodeId>, combined: bool },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub kind: NodeKind,
    pub transform: Transform,
    pub combine: Combine,
    pub repeat: Repeat,
    pub mirror: Mirror,
    pub material: u32,
    pub visible: bool,
    pub expanded: bool,
}

impl Node {
    pub fn shape(&self) -> Option<&Shape> {
        match &self.kind {
            NodeKind::Shape(s) => Some(s),
            _ => None,
        }
    }
    pub fn shape_mut(&mut self) -> Option<&mut Shape> {
        match &mut self.kind {
            NodeKind::Shape(s) => Some(s),
            _ => None,
        }
    }
    pub fn children(&self) -> &[NodeId] {
        match &self.kind {
            NodeKind::Group { children, .. } => children,
            _ => &[],
        }
    }
    pub fn is_group(&self) -> bool {
        matches!(self.kind, NodeKind::Group { .. })
    }
    pub fn type_label(&self) -> String {
        match &self.kind {
            NodeKind::Shape(s) => s.kind.label().to_string(),
            NodeKind::Group { .. } => "Groupe".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub name: String,
    pub color: [f32; 3],
    pub metal: f32,
    pub rough: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub nodes: BTreeMap<NodeId, Node>,
    pub root: NodeId,
    pub next_id: NodeId,
    pub materials: Vec<Material>,
}

impl Default for Scene {
    fn default() -> Self {
        Scene::new()
    }
}

/// A node ready to be inserted (id assigned by the scene).
pub struct NodeSpec {
    pub name: String,
    pub kind: NodeKind,
    pub transform: Transform,
    pub combine: Combine,
    pub repeat: Repeat,
    pub mirror: Mirror,
    pub material: u32,
}

impl NodeSpec {
    pub fn shape(name: &str, shape: Shape) -> Self {
        NodeSpec {
            name: name.to_string(),
            kind: NodeKind::Shape(shape),
            transform: Transform::default(),
            combine: Combine::default(),
            repeat: Repeat::None,
            mirror: Mirror::default(),
            material: 0,
        }
    }
    pub fn group(name: &str, combined: bool) -> Self {
        NodeSpec {
            name: name.to_string(),
            kind: NodeKind::Group { children: vec![], combined },
            transform: Transform::default(),
            combine: Combine::default(),
            repeat: Repeat::None,
            mirror: Mirror::default(),
            material: 0,
        }
    }
    pub fn at(mut self, pos: Vec3) -> Self {
        self.transform.pos = pos;
        self
    }
    pub fn rot(mut self, rot: Quat) -> Self {
        self.transform.rot = rot;
        self
    }
    pub fn mode(mut self, mode: CombineMode) -> Self {
        self.combine.mode = mode;
        self
    }
    pub fn blend(mut self, blend: f32) -> Self {
        self.combine.blend = blend;
        self
    }
    pub fn chamfer_blend(mut self, blend: f32) -> Self {
        self.combine.blend = blend;
        self.combine.kind = BlendKind::Chamfer;
        self
    }
    pub fn groove(mut self, depth: f32, width: f32) -> Self {
        self.combine.depth = depth;
        self.combine.width = width;
        self
    }
    pub fn repeat(mut self, repeat: Repeat) -> Self {
        self.repeat = repeat;
        self
    }
    pub fn mirror(mut self, mirror: Mirror) -> Self {
        self.mirror = mirror;
        self
    }
    pub fn mat(mut self, material: u32) -> Self {
        self.material = material;
        self
    }
}

impl Scene {
    pub fn new() -> Self {
        let root = Node {
            id: 0,
            name: "Scène".into(),
            kind: NodeKind::Group { children: vec![], combined: false },
            transform: Transform::default(),
            combine: Combine::default(),
            repeat: Repeat::None,
            mirror: Mirror::default(),
            material: 0,
            visible: true,
            expanded: true,
        };
        let mut nodes = BTreeMap::new();
        nodes.insert(0, root);
        Scene { nodes, root: 0, next_id: 1, materials: default_materials() }
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    /// Inserts a node under `parent` (at `index`, or last).
    pub fn insert(&mut self, parent: NodeId, spec: NodeSpec, index: Option<usize>) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        let node = Node {
            id,
            name: spec.name,
            kind: spec.kind,
            transform: spec.transform,
            combine: spec.combine,
            repeat: spec.repeat,
            mirror: spec.mirror,
            material: spec.material,
            visible: true,
            expanded: false,
        };
        self.nodes.insert(id, node);
        if let Some(NodeKind::Group { children, .. }) = self.nodes.get_mut(&parent).map(|n| &mut n.kind) {
            match index {
                Some(i) if i <= children.len() => children.insert(i, id),
                _ => children.push(id),
            }
        }
        id
    }

    pub fn add(&mut self, parent: NodeId, spec: NodeSpec) -> NodeId {
        self.insert(parent, spec, None)
    }

    pub fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.values().find(|n| n.children().contains(&id)).map(|n| n.id)
    }

    /// Ancestors from the root (excluded) down to the parent of `id`.
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = vec![];
        let mut cur = self.parent_of(id);
        while let Some(p) = cur {
            if p == self.root {
                break;
            }
            chain.push(p);
            cur = self.parent_of(p);
        }
        chain.reverse();
        chain
    }

    pub fn remove(&mut self, id: NodeId) {
        if id == self.root {
            return;
        }
        if let Some(p) = self.parent_of(id) {
            if let Some(NodeKind::Group { children, .. }) = self.nodes.get_mut(&p).map(|n| &mut n.kind) {
                children.retain(|c| *c != id);
            }
        }
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            if let Some(node) = self.nodes.remove(&n) {
                stack.extend_from_slice(node.children());
            }
        }
    }

    /// Duplicates a node (and its subtree) right after it. Returns the copy.
    pub fn duplicate(&mut self, id: NodeId) -> Option<NodeId> {
        let parent = self.parent_of(id)?;
        let index = self.node(parent)?.children().iter().position(|c| *c == id)? + 1;
        let copy = self.clone_subtree(id)?;
        if let Some(NodeKind::Group { children, .. }) = self.nodes.get_mut(&parent).map(|n| &mut n.kind) {
            children.insert(index, copy);
        }
        Some(copy)
    }

    fn clone_subtree(&mut self, id: NodeId) -> Option<NodeId> {
        let mut node = self.node(id)?.clone();
        let new_id = self.next_id;
        self.next_id += 1;
        node.id = new_id;
        if let NodeKind::Group { children, .. } = &mut node.kind {
            let old = std::mem::take(children);
            let mut fresh = vec![];
            for c in old {
                if let Some(nc) = self.clone_subtree(c) {
                    fresh.push(nc);
                }
            }
            *children = fresh;
        }
        self.nodes.insert(new_id, node);
        Some(new_id)
    }

    /// Moves `id` to `new_parent` at `index`.
    pub fn reparent(&mut self, id: NodeId, new_parent: NodeId, index: usize) {
        if id == self.root || id == new_parent || self.is_descendant(new_parent, id) {
            return;
        }
        if let Some(p) = self.parent_of(id) {
            if let Some(NodeKind::Group { children, .. }) = self.nodes.get_mut(&p).map(|n| &mut n.kind) {
                children.retain(|c| *c != id);
            }
        }
        if let Some(NodeKind::Group { children, .. }) = self.nodes.get_mut(&new_parent).map(|n| &mut n.kind) {
            let i = index.min(children.len());
            children.insert(i, id);
        }
    }

    pub fn is_descendant(&self, maybe_child: NodeId, ancestor: NodeId) -> bool {
        let mut cur = self.parent_of(maybe_child);
        while let Some(p) = cur {
            if p == ancestor {
                return true;
            }
            cur = self.parent_of(p);
        }
        false
    }

    /// World matrix of a node's local frame (ignores repetitions).
    pub fn world_matrix(&self, id: NodeId) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        for a in self.ancestors(id) {
            m *= self.nodes[&a].transform.matrix();
        }
        if let Some(n) = self.node(id) {
            m *= n.transform.matrix();
        }
        m
    }

    /// Matrix of the parent frame of a node.
    pub fn parent_matrix(&self, id: NodeId) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        for a in self.ancestors(id) {
            m *= self.nodes[&a].transform.matrix();
        }
        m
    }

    /// Depth-first list (id, depth) of the visible tree rows, root excluded.
    pub fn rows(&self) -> Vec<(NodeId, usize)> {
        let mut out = vec![];
        fn walk(s: &Scene, id: NodeId, depth: usize, out: &mut Vec<(NodeId, usize)>) {
            let n = &s.nodes[&id];
            for c in n.children() {
                out.push((*c, depth));
                let cn = &s.nodes[c];
                if cn.is_group() && cn.expanded {
                    walk(s, *c, depth + 1, out);
                }
            }
        }
        walk(self, self.root, 0, &mut out);
        out
    }

    /// Number of shape nodes (not counting repetitions).
    pub fn shape_count(&self) -> usize {
        self.nodes.values().filter(|n| n.shape().is_some()).count()
    }

    /// Axis-aligned bounds of everything that adds matter (approximate, conservative).
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        fn walk(s: &Scene, id: NodeId, parent: Mat4, lo: &mut Vec3, hi: &mut Vec3) {
            let n = &s.nodes[&id];
            if !n.visible {
                return;
            }
            let m = parent * n.transform.matrix();
            match &n.kind {
                NodeKind::Shape(shape) => {
                    if n.combine.mode != CombineMode::Add && n.combine.mode != CombineMode::Tongue {
                        return;
                    }
                    let (l, h) = shape.local_bounds();
                    let pad = Vec3::splat(n.combine.blend + n.combine.depth);
                    let (l, h) = (l - pad, h + pad);
                    let mut corners = vec![];
                    for i in 0..8 {
                        let c = Vec3::new(
                            if i & 1 == 0 { l.x } else { h.x },
                            if i & 2 == 0 { l.y } else { h.y },
                            if i & 4 == 0 { l.z } else { h.z },
                        );
                        corners.push(m.transform_point3(c));
                    }
                    let expanded = expand_repeat(&corners, n, parent);
                    for c in expanded {
                        *lo = lo.min(c);
                        *hi = hi.max(c);
                    }
                }
                NodeKind::Group { children, .. } => {
                    let mut glo = Vec3::splat(f32::MAX);
                    let mut ghi = Vec3::splat(f32::MIN);
                    for c in children {
                        walk(s, *c, m, &mut glo, &mut ghi);
                    }
                    if glo.x <= ghi.x {
                        let mut corners = vec![];
                        for i in 0..8 {
                            corners.push(Vec3::new(
                                if i & 1 == 0 { glo.x } else { ghi.x },
                                if i & 2 == 0 { glo.y } else { ghi.y },
                                if i & 4 == 0 { glo.z } else { ghi.z },
                            ));
                        }
                        for c in expand_repeat(&corners, n, parent) {
                            *lo = lo.min(c);
                            *hi = hi.max(c);
                        }
                    }
                }
            }
        }
        // Repetition and mirror act in the parent frame; we approximate them in world space.
        fn expand_repeat(corners: &[Vec3], n: &Node, parent: Mat4) -> Vec<Vec3> {
            let inv = parent.inverse();
            let local: Vec<Vec3> = corners.iter().map(|c| inv.transform_point3(*c)).collect();
            let mut pts = local.clone();
            match n.repeat {
                Repeat::Radial { count, axis } => {
                    for i in 1..count.max(1) {
                        let q = Quat::from_axis_angle(axis.vec(), i as f32 * std::f32::consts::TAU / count as f32);
                        pts.extend(local.iter().map(|p| q * *p));
                    }
                }
                Repeat::Linear { count, step } => {
                    for i in 1..count.max(1) {
                        pts.extend(local.iter().map(|p| *p + step * i as f32));
                    }
                }
                Repeat::None => {}
            }
            if n.mirror.any() {
                let base = pts.clone();
                for p in base {
                    let mut q = p;
                    if n.mirror.x {
                        q.x = -q.x;
                    }
                    if n.mirror.y {
                        q.y = -q.y;
                    }
                    if n.mirror.z {
                        q.z = -q.z;
                    }
                    pts.push(q);
                }
            }
            pts.iter().map(|p| parent.transform_point3(*p)).collect()
        }
        walk(self, self.root, Mat4::IDENTITY, &mut lo, &mut hi);
        if lo.x > hi.x {
            return (Vec3::splat(-20.0), Vec3::splat(20.0));
        }
        (lo, hi)
    }

    /// World-space AABB of a shape node, including every copy made by its own
    /// and its ancestors' repetitions and mirrors (used to skip far primitives).
    pub fn shape_world_aabb(&self, id: NodeId) -> Option<(Vec3, Vec3)> {
        let n = self.node(id)?;
        let shape = n.shape()?;
        let pad = Vec3::splat(n.combine.blend.max(0.0) + n.combine.depth.max(0.0) + n.combine.width.max(0.0));
        let (l, h) = shape.local_bounds();
        let (l, h) = (l - pad, h + pad);
        let mut pts: Vec<Vec3> = (0..8)
            .map(|i| {
                Vec3::new(
                    if i & 1 == 0 { l.x } else { h.x },
                    if i & 2 == 0 { l.y } else { h.y },
                    if i & 4 == 0 { l.z } else { h.z },
                )
            })
            .collect();
        let mut chain = self.ancestors(id);
        chain.reverse();
        let mut levels = vec![n];
        for a in chain {
            levels.push(&self.nodes[&a]);
        }
        if let Some(root) = self.node(self.root) {
            levels.push(root);
        }
        for lvl in levels {
            let m = lvl.transform.matrix();
            pts = pts.iter().map(|p| m.transform_point3(*p)).collect();
            pts = expand_copies(&pts, lvl.repeat, lvl.mirror);
        }
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for p in pts {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        Some((lo, hi))
    }

    /// Centre of one copy of a node's content, in its parent's frame
    /// (anchor of linear repetitions: copies are numbered from there).
    pub fn content_center_parent(&self, id: NodeId) -> Vec3 {
        let Some(n) = self.node(id) else { return Vec3::ZERO };
        if n.shape().is_some() {
            return n.transform.pos;
        }
        let mut tmp = self.clone();
        if let Some(m) = tmp.node_mut(id) {
            m.repeat = Repeat::None;
            m.mirror = Mirror::default();
        }
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        let mut stack = n.children().to_vec();
        while let Some(c) = stack.pop() {
            if let Some(cn) = tmp.node(c) {
                stack.extend_from_slice(cn.children());
                if cn.shape().is_some() && cn.combine.mode != CombineMode::Carve {
                    if let Some((l, h)) = tmp.shape_world_aabb(c) {
                        lo = lo.min(l);
                        hi = hi.max(h);
                    }
                }
            }
        }
        if lo.x > hi.x {
            return n.transform.pos;
        }
        self.parent_matrix(id).inverse().transform_point3((lo + hi) * 0.5)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<Scene, serde_json::Error> {
        serde_json::from_str(s)
    }
}

pub fn default_materials() -> Vec<Material> {
    vec![
        Material { name: "Gris argile".into(), color: [0.50, 0.50, 0.49], metal: 0.0, rough: 0.55 },
        Material { name: "Vert militaire".into(), color: [0.34, 0.39, 0.24], metal: 0.0, rough: 0.45 },
        Material { name: "Orange".into(), color: [0.86, 0.42, 0.14], metal: 0.0, rough: 0.4 },
        Material { name: "Métal sombre".into(), color: [0.16, 0.17, 0.19], metal: 0.7, rough: 0.35 },
        Material { name: "Acier".into(), color: [0.56, 0.57, 0.6], metal: 1.0, rough: 0.3 },
        Material { name: "Caoutchouc".into(), color: [0.08, 0.08, 0.09], metal: 0.0, rough: 0.8 },
    ]
}

/// Copies of points made by a repetition and a mirror (in the parent frame).
pub fn expand_copies(local: &[Vec3], repeat: Repeat, mirror: Mirror) -> Vec<Vec3> {
    let mut pts = local.to_vec();
    match repeat {
        Repeat::Radial { count, axis } => {
            for i in 1..count.max(1) {
                let q = Quat::from_axis_angle(axis.vec(), i as f32 * std::f32::consts::TAU / count as f32);
                pts.extend(local.iter().map(|p| q * *p));
            }
        }
        Repeat::Linear { count, step } => {
            for i in 1..count.max(1) {
                pts.extend(local.iter().map(|p| *p + step * i as f32));
            }
        }
        Repeat::None => {}
    }
    if mirror.any() {
        let base = pts.clone();
        for p in base {
            let mut q = p;
            if mirror.x {
                q.x = -q.x;
            }
            if mirror.y {
                q.y = -q.y;
            }
            if mirror.z {
                q.z = -q.z;
            }
            pts.push(q);
        }
    }
    pts
}
