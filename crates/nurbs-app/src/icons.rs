//! Vector icons, drawn in a 20×20 design space (same drawings as the mock-up).

use egui::{epaint::PathStroke, pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Select,
    Box,
    Cylinder,
    Sphere,
    Capsule,
    Torus,
    Cone,
    Prism,
    Tube,
    Profile,
    Undo,
    Redo,
    Mirror,
    Magnet,
    Grid,
    Export,
    Duplicate,
    Trash,
    Eye,
    EyeOff,
    Folder,
    Chevron,
    ChevronRight,
    ChevronLeft,
    Up,
    Down,
    Cube,
    Frame,
    Settings,
    Light,
    Fillet,
    /// Rotation gizmo (E).
    Rotate,
    /// Move gizmo (W).
    Move,
    /// Scale gizmo (R).
    Scale,
    /// Sketch (Forme NURBS): an outline of corners and a curve, its points.
    Sketch,
    /// Path: an open curve with a rounded corner, its two ends.
    Path,
    /// Quads view: a patch of quads bending round a corner.
    Quads,
    /// Selection modes: a cube with a corner, an edge, a face, or all of it.
    ModeVertex,
    ModeEdge,
    ModeFace,
    ModeObject,
    /// Inset face: a square inside a square, corners joined.
    Inset,
    /// Extrude face: a face and an arrow up out of it.
    Extrude,
    /// Remesh: quads flowing round a bend, the whole part at once.
    Remesh,
    /// Knife: a blade (as in Forme Rectify).
    Knife,
    /// Point on edge: an edge and a point on it.
    EdgePoint,
    /// The reference mesh: a lumpy outline, triangulated.
    Scan,
    /// The reference seen through: a square seen through another.
    Ghost,
    /// The deviation: a surface over a reference line, the gap marked.
    Deviation,
    /// X-ray: a cube seen through, its hidden edges shown.
    Xray,
    Isolate,
    /// The Curves set: a rectangle with rounded corners, a circle, an
    /// ellipse, a hexagon, a star.
    CurveRect,
    CurveCircle,
    CurveEllipse,
    CurvePolygon,
    CurveStar,
    /// The 2D booleans: two circles side by side, the result drawn, the
    /// rest dotted.
    BoolUnion,
    BoolSubtract,
    BoolIntersect,
    /// The workspaces (tabs): Forme (a filleted block), Sculpt (a lump of
    /// clay), Poly (a cube's cage).
    TabForme,
    TabSculpt,
    TabPoly,
    /// The Sculpt brushes.
    BrushDraw,
    BrushClay,
    BrushBuildup,
    BrushSmooth,
    BrushGrab,
    BrushInflate,
    BrushFlatten,
    BrushPinch,
    /// Poly: a plane of quads, an edge loop across a cube, a quad cut in
    /// four, a rounded cage, a cage sent to the clay.
    PolyPlane,
    /// Poly: an edge bevelled into rows (a corner cut round), the Cut tool's
    /// line across a quad.
    PolyBevel,
    PolyCut,
    EdgeLoop,
    Subdivide,
    SmoothMesh,
    ToSculpt,
}

struct Pen<'a> {
    p: &'a Painter,
    r: Rect,
    s: Stroke,
}

impl Pen<'_> {
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        let k = self.r.width() / 20.0;
        pos2(self.r.min.x + x * k, self.r.min.y + y * k)
    }
    fn k(&self) -> f32 {
        self.r.width() / 20.0
    }
    fn line(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.pt(*x, *y)).collect();
        self.p.add(Shape::line(v, PathStroke::new(self.s.width, self.s.color)));
    }
    fn closed(&self, pts: &[(f32, f32)]) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.pt(*x, *y)).collect();
        self.p.add(Shape::closed_line(v, PathStroke::new(self.s.width, self.s.color)));
    }
    fn ellipse(&self, cx: f32, cy: f32, rx: f32, ry: f32) {
        let k = self.k();
        self.p.add(Shape::ellipse_stroke(self.pt(cx, cy), vec2(rx * k, ry * k), self.s));
    }
    fn circle(&self, cx: f32, cy: f32, r: f32) {
        self.p.circle_stroke(self.pt(cx, cy), r * self.k(), self.s);
    }
    fn dot(&self, cx: f32, cy: f32, r: f32) {
        self.p.circle_filled(self.pt(cx, cy), r * self.k(), self.s.color);
    }
    fn fill(&self, pts: &[(f32, f32)], alpha: f32) {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| self.pt(*x, *y)).collect();
        self.p.add(Shape::convex_polygon(v, self.s.color.gamma_multiply(alpha), Stroke::NONE));
    }
    /// Half ellipse (lower half when `lower`).
    fn arc(&self, cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32) {
        let n = 16;
        let pts: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / n as f32;
                (cx + rx * a.cos(), cy + ry * a.sin())
            })
            .collect();
        self.line(&pts);
    }
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let pen = Pen { p, r: rect, s: Stroke::new(1.6 * rect.width() / 20.0, color) };
    use std::f32::consts::PI;
    match icon {
        Icon::Select => pen.closed(&[(5.0, 3.0), (15.0, 9.0), (10.5, 10.5), (8.8, 15.0)]),
        Icon::Box | Icon::Cube => {
            pen.closed(&[(10.0, 2.5), (16.5, 6.0), (16.5, 14.0), (10.0, 17.5), (3.5, 14.0), (3.5, 6.0)]);
            pen.line(&[(3.5, 6.0), (10.0, 9.5), (16.5, 6.0)]);
            pen.line(&[(10.0, 9.5), (10.0, 17.5)]);
        }
        Icon::Cylinder => {
            pen.ellipse(10.0, 5.0, 6.0, 2.5);
            pen.line(&[(4.0, 5.0), (4.0, 15.0)]);
            pen.line(&[(16.0, 5.0), (16.0, 15.0)]);
            pen.arc(10.0, 15.0, 6.0, 2.5, 0.0, PI);
        }
        Icon::Sphere => {
            pen.circle(10.0, 10.0, 7.0);
            pen.ellipse(10.0, 10.0, 7.0, 2.5);
        }
        Icon::Capsule => {
            pen.arc(10.0, 6.5, 4.0, 4.0, PI, 2.0 * PI);
            pen.arc(10.0, 13.5, 4.0, 4.0, 0.0, PI);
            pen.line(&[(6.0, 6.5), (6.0, 13.5)]);
            pen.line(&[(14.0, 6.5), (14.0, 13.5)]);
        }
        Icon::Torus => {
            pen.ellipse(10.0, 10.0, 8.0, 4.5);
            pen.ellipse(10.0, 9.6, 3.5, 1.5);
        }
        Icon::Cone => {
            pen.line(&[(4.0, 15.0), (10.0, 2.5), (16.0, 15.0)]);
            pen.ellipse(10.0, 15.0, 6.0, 2.3);
        }
        Icon::Prism => {
            pen.closed(&[(10.0, 2.5), (16.5, 6.25), (16.5, 13.75), (10.0, 17.5), (3.5, 13.75), (3.5, 6.25)]);
            pen.line(&[(10.0, 2.5), (10.0, 17.5)]);
        }
        Icon::Tube => {
            pen.ellipse(10.0, 5.0, 6.0, 2.5);
            pen.ellipse(10.0, 5.0, 2.8, 1.1);
            pen.line(&[(4.0, 5.0), (4.0, 15.0)]);
            pen.line(&[(16.0, 5.0), (16.0, 15.0)]);
            pen.arc(10.0, 15.0, 6.0, 2.5, 0.0, PI);
        }
        Icon::Profile => {
            let pts: Vec<(f32, f32)> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    let x = (1.0 - t) * (1.0 - t) * 3.0 + 2.0 * (1.0 - t) * t * 5.0 + t * t * 12.0;
                    let y = (1.0 - t) * (1.0 - t) * 16.0 + 2.0 * (1.0 - t) * t * 7.0 + t * t * 6.0;
                    (x, y)
                })
                .collect();
            pen.line(&pts);
            pen.closed(&[(11.0, 3.5), (16.5, 9.0), (14.5, 11.0), (9.0, 5.5)]);
            pen.dot(3.0, 16.0, 1.3);
        }
        Icon::Undo => {
            pen.line(&[(7.5, 5.0), (4.0, 8.5), (7.5, 12.0)]);
            pen.line(&[(4.0, 8.5), (12.0, 8.5)]);
            pen.arc(12.0, 12.5, 4.0, 4.0, -PI / 2.0, PI / 2.0);
            pen.line(&[(12.0, 16.5), (9.0, 16.5)]);
        }
        Icon::Redo => {
            pen.line(&[(12.5, 5.0), (16.0, 8.5), (12.5, 12.0)]);
            pen.line(&[(16.0, 8.5), (8.0, 8.5)]);
            pen.arc(8.0, 12.5, 4.0, 4.0, PI / 2.0, 1.5 * PI);
            pen.line(&[(8.0, 16.5), (11.0, 16.5)]);
        }
        Icon::Mirror => {
            for i in 0..6 {
                let y = 2.5 + i as f32 * 2.6;
                pen.line(&[(10.0, y), (10.0, y + 1.3)]);
            }
            pen.line(&[(7.0, 5.0), (3.0, 10.0), (7.0, 15.0)]);
            pen.line(&[(13.0, 5.0), (17.0, 10.0), (13.0, 15.0)]);
        }
        Icon::Magnet => {
            pen.line(&[(6.0, 3.0), (6.0, 9.0)]);
            pen.line(&[(14.0, 3.0), (14.0, 9.0)]);
            pen.arc(10.0, 9.0, 4.0, 4.0, 0.0, PI);
            pen.line(&[(6.0, 6.0), (9.0, 6.0)]);
            pen.line(&[(11.0, 6.0), (14.0, 6.0)]);
        }
        Icon::Sketch => {
            pen.line(&[(3.0, 16.0), (3.0, 6.0), (9.0, 6.0)]);
            pen.line(&[(9.0, 6.0), (14.5, 6.5), (16.5, 11.0), (14.0, 15.5), (8.0, 16.0), (3.0, 16.0)]);
            for (x, y) in [(3.0, 16.0), (3.0, 6.0), (9.0, 6.0)] {
                pen.closed(&[(x - 1.3, y - 1.3), (x + 1.3, y - 1.3), (x + 1.3, y + 1.3), (x - 1.3, y + 1.3)]);
            }
        }
        Icon::Knife => {
            pen.line(&[(3.0, 17.0), (15.5, 4.5), (17.0, 6.5), (7.5, 16.0), (3.0, 17.0)]);
            pen.line(&[(11.0, 9.0), (13.0, 11.0)]);
        }
        Icon::EdgePoint => {
            pen.line(&[(3.0, 15.0), (17.0, 5.0)]);
            pen.fill(&[(8.4, 11.1), (10.0, 9.1), (12.0, 10.7), (10.4, 12.7)], 1.0);
        }
        Icon::Path => {
            pen.line(&[(3.5, 15.5), (3.5, 9.0), (4.0, 6.8), (5.6, 5.2), (7.8, 4.6), (16.5, 4.6)]);
            pen.line(&[(16.5, 4.6), (16.5, 9.5), (10.0, 16.0)]);
            for (x, y) in [(3.5, 15.5), (10.0, 16.0)] {
                pen.closed(&[(x - 1.3, y - 1.3), (x + 1.3, y - 1.3), (x + 1.3, y + 1.3), (x - 1.3, y + 1.3)]);
            }
        }
        Icon::CurveRect => {
            pen.arc(6.0, 7.0, 2.5, 2.5, PI, 1.5 * PI);
            pen.arc(14.0, 7.0, 2.5, 2.5, 1.5 * PI, 2.0 * PI);
            pen.arc(14.0, 13.0, 2.5, 2.5, 0.0, 0.5 * PI);
            pen.arc(6.0, 13.0, 2.5, 2.5, 0.5 * PI, PI);
            pen.line(&[(6.0, 4.5), (14.0, 4.5)]);
            pen.line(&[(16.5, 7.0), (16.5, 13.0)]);
            pen.line(&[(14.0, 15.5), (6.0, 15.5)]);
            pen.line(&[(3.5, 13.0), (3.5, 7.0)]);
        }
        Icon::CurveCircle => {
            pen.circle(10.0, 10.0, 6.5);
            pen.dot(10.0, 10.0, 0.9);
        }
        Icon::CurveEllipse => pen.ellipse(10.0, 10.0, 7.5, 4.8),
        Icon::CurvePolygon => {
            let v: Vec<(f32, f32)> = (0..6).map(|k| { let a = PI / 6.0 + PI / 3.0 * k as f32; (10.0 + 7.0 * a.cos(), 10.0 + 7.0 * a.sin()) }).collect();
            pen.closed(&v);
        }
        Icon::CurveStar => {
            let v: Vec<(f32, f32)> = (0..10).map(|k| { let a = -PI / 2.0 + PI / 5.0 * k as f32; let r = if k % 2 == 0 { 7.5 } else { 3.3 }; (10.0 + r * a.cos(), 10.6 + r * a.sin()) }).collect();
            pen.closed(&v);
        }
        Icon::BoolUnion | Icon::BoolSubtract | Icon::BoolIntersect => {
            // circles of radius 5.5 at x 7.5 and 12.5: they cross at ±1.14
            // rad on the left one, π ∓ 1.14 on the right one
            let (r, a, b, c) = (5.5, 7.5, 12.5, 1.141);
            let dotted = |cx: f32, a0: f32, a1: f32| {
                for i in 0..=6 {
                    let t = a0 + (a1 - a0) * i as f32 / 6.0;
                    pen.dot(cx + r * t.cos(), 10.0 + r * t.sin(), 0.55);
                }
            };
            let (a_out, a_in, b_out, b_in) = ((c, 2.0 * PI - c), (-c, c), (PI + c, 3.0 * PI - c), (PI - c, PI + c));
            let (solid, dots): (Vec<(f32, (f32, f32))>, Vec<(f32, (f32, f32))>) = match icon {
                Icon::BoolUnion => (vec![(a, a_out), (b, b_out)], vec![(a, a_in), (b, b_in)]),
                Icon::BoolSubtract => (vec![(a, a_out), (b, b_in)], vec![(b, b_out)]),
                _ => (vec![(a, a_in), (b, b_in)], vec![(a, a_out), (b, b_out)]),
            };
            for (cx, (a0, a1)) in solid {
                pen.arc(cx, 10.0, r, r, a0, a1);
            }
            for (cx, (a0, a1)) in dots {
                dotted(cx, a0, a1);
            }
        }
        Icon::Quads => {
            // a flat run that bends down: three rows of quads
            pen.line(&[(3.0, 4.0), (11.0, 4.0), (15.0, 6.0), (17.0, 10.0), (17.0, 17.0)]);
            pen.line(&[(3.0, 10.0), (9.0, 10.0), (11.0, 11.0), (12.0, 13.0), (12.0, 17.0)]);
            pen.line(&[(3.0, 4.0), (3.0, 10.0)]);
            pen.line(&[(7.0, 4.0), (7.0, 10.0)]);
            pen.line(&[(11.0, 4.0), (9.0, 10.0)]);
            pen.line(&[(15.0, 6.0), (11.0, 11.0)]);
            pen.line(&[(17.0, 10.0), (12.0, 13.0)]);
            pen.line(&[(17.0, 17.0), (12.0, 17.0)]);
        }
        Icon::Remesh => {
            // two families of curves crossing: a quad field round a bend
            pen.arc(17.0, 17.0, 14.0, 14.0, PI, 1.5 * PI);
            pen.arc(17.0, 17.0, 9.5, 9.5, PI, 1.5 * PI);
            pen.arc(17.0, 17.0, 5.0, 5.0, PI, 1.5 * PI);
            pen.line(&[(3.0, 17.0), (7.5, 17.0)]);
            pen.line(&[(17.0, 3.0), (17.0, 7.5)]);
            pen.line(&[(7.1, 7.1), (10.3, 10.3)]);
            pen.line(&[(4.1, 11.6), (8.3, 13.4)]);
            pen.line(&[(11.6, 4.1), (13.4, 8.3)]);
        }
        Icon::Scan => {
            pen.closed(&[(3.0, 12.0), (5.0, 5.0), (11.0, 3.0), (17.0, 6.5), (17.0, 13.0), (11.0, 17.0), (5.0, 16.0)]);
            pen.line(&[(5.0, 5.0), (10.0, 10.0), (17.0, 6.5)]);
            pen.line(&[(3.0, 12.0), (10.0, 10.0), (11.0, 17.0)]);
            pen.line(&[(11.0, 3.0), (10.0, 10.0), (17.0, 13.0)]);
        }
        Icon::Deviation => {
            pen.line(&[(2.5, 14.0), (17.5, 14.0)]);
            pen.line(&[(2.5, 10.0), (6.0, 10.0), (10.0, 5.5), (14.0, 8.5), (17.5, 8.5)]);
            pen.line(&[(10.0, 8.0), (10.0, 12.0)]);
        }
        Icon::Ghost => {
            pen.closed(&[(3.0, 3.0), (12.0, 3.0), (12.0, 12.0), (3.0, 12.0)]);
            pen.line(&[(8.0, 8.0), (8.0, 17.0)]);
            pen.line(&[(8.0, 17.0), (17.0, 17.0)]);
            pen.line(&[(17.0, 17.0), (17.0, 8.0)]);
            pen.line(&[(17.0, 8.0), (8.0, 8.0)]);
        }
        Icon::Grid => {
            pen.closed(&[(3.0, 3.0), (17.0, 3.0), (17.0, 17.0), (3.0, 17.0)]);
            pen.line(&[(3.0, 10.0), (17.0, 10.0)]);
            pen.line(&[(10.0, 3.0), (10.0, 17.0)]);
        }
        Icon::Export => {
            pen.line(&[(10.0, 13.0), (10.0, 3.0)]);
            pen.line(&[(6.0, 7.0), (10.0, 3.0), (14.0, 7.0)]);
            pen.line(&[(4.0, 12.0), (4.0, 16.0), (16.0, 16.0), (16.0, 12.0)]);
        }
        Icon::Duplicate => {
            pen.closed(&[(7.0, 7.0), (17.0, 7.0), (17.0, 17.0), (7.0, 17.0)]);
            pen.line(&[(13.0, 7.0), (13.0, 3.0), (3.0, 3.0), (3.0, 13.0), (7.0, 13.0)]);
        }
        Icon::Trash => {
            pen.line(&[(4.0, 6.0), (16.0, 6.0)]);
            pen.line(&[(8.0, 6.0), (8.0, 4.0), (12.0, 4.0), (12.0, 6.0)]);
            pen.line(&[(6.0, 6.0), (7.0, 17.0), (13.0, 17.0), (14.0, 6.0)]);
        }
        Icon::Eye | Icon::EyeOff => {
            pen.arc(10.0, 13.0, 8.0, 7.0, PI * 1.15, PI * 1.85);
            pen.arc(10.0, 7.0, 8.0, 7.0, PI * 0.15, PI * 0.85);
            pen.circle(10.0, 10.0, 2.4);
            if icon == Icon::EyeOff {
                pen.line(&[(3.5, 16.5), (16.5, 3.5)]);
            }
        }
        Icon::Folder => {
            pen.closed(&[(3.0, 5.5), (8.0, 5.5), (9.5, 7.0), (17.0, 7.0), (17.0, 15.5), (3.0, 15.5)]);
        }
        Icon::Chevron => pen.line(&[(5.0, 8.0), (10.0, 13.0), (15.0, 8.0)]),
        Icon::ChevronRight => pen.line(&[(8.0, 5.0), (13.0, 10.0), (8.0, 15.0)]),
        Icon::ChevronLeft => pen.line(&[(12.0, 5.0), (7.0, 10.0), (12.0, 15.0)]),
        Icon::Up => pen.line(&[(5.0, 12.0), (10.0, 7.0), (15.0, 12.0)]),
        Icon::Down => pen.line(&[(5.0, 8.0), (10.0, 13.0), (15.0, 8.0)]),
        Icon::Frame => {
            pen.line(&[(3.0, 7.0), (3.0, 3.0), (7.0, 3.0)]);
            pen.line(&[(13.0, 3.0), (17.0, 3.0), (17.0, 7.0)]);
            pen.line(&[(17.0, 13.0), (17.0, 17.0), (13.0, 17.0)]);
            pen.line(&[(7.0, 17.0), (3.0, 17.0), (3.0, 13.0)]);
            pen.circle(10.0, 10.0, 2.5);
        }
        Icon::Light => {
            // a light bulb with its rays
            pen.arc(10.0, 8.6, 4.2, 4.2, PI * 0.78, PI * 2.22);
            pen.line(&[(7.4, 11.9), (7.9, 14.2), (12.1, 14.2), (12.6, 11.9)]);
            pen.line(&[(8.2, 16.3), (11.8, 16.3)]);
            pen.line(&[(10.0, 1.4), (10.0, 2.6)]);
            pen.line(&[(3.4, 4.0), (4.3, 4.9)]);
            pen.line(&[(16.6, 4.0), (15.7, 4.9)]);
        }
        Icon::Settings => {
            pen.circle(10.0, 10.0, 2.6);
            pen.circle(10.0, 10.0, 5.6);
            for i in 0..8 {
                let a = i as f32 * PI / 4.0;
                pen.line(&[(10.0 + 5.6 * a.cos(), 10.0 + 5.6 * a.sin()), (10.0 + 7.8 * a.cos(), 10.0 + 7.8 * a.sin())]);
            }
        }
        Icon::Fillet => {
            pen.arc(10.0, 10.0, 6.5, 6.5, PI, 1.5 * PI);
            pen.line(&[(3.5, 10.0), (3.5, 17.0)]);
            pen.line(&[(10.0, 3.5), (17.0, 3.5)]);
        }
        Icon::Rotate => {
            // a circular arrow
            pen.arc(10.0, 10.0, 6.5, 6.5, -PI * 0.35, PI * 1.35);
            pen.line(&[(14.6, 2.4), (15.3, 5.4), (12.2, 6.0)]);
        }
        Icon::Move => {
            pen.line(&[(10.0, 2.5), (10.0, 17.5)]);
            pen.line(&[(2.5, 10.0), (17.5, 10.0)]);
            pen.line(&[(8.0, 4.5), (10.0, 2.5), (12.0, 4.5)]);
            pen.line(&[(8.0, 15.5), (10.0, 17.5), (12.0, 15.5)]);
            pen.line(&[(4.5, 8.0), (2.5, 10.0), (4.5, 12.0)]);
            pen.line(&[(15.5, 8.0), (17.5, 10.0), (15.5, 12.0)]);
        }
        Icon::ModeVertex | Icon::ModeEdge | Icon::ModeFace | Icon::ModeObject => {
            let hex = [(10.0, 2.5), (16.5, 6.0), (16.5, 14.0), (10.0, 17.5), (3.5, 14.0), (3.5, 6.0)];
            let front = [(10.0, 9.5), (16.5, 6.0), (16.5, 14.0), (10.0, 17.5)];
            match icon {
                Icon::ModeFace => pen.fill(&front, 0.9),
                Icon::ModeObject => pen.fill(&hex, 0.45),
                _ => {}
            }
            let thin = Pen { p, r: rect, s: Stroke::new(pen.s.width * 0.75, pen.s.color.gamma_multiply(if icon == Icon::ModeObject { 1.0 } else { 0.75 })) };
            thin.closed(&hex);
            thin.line(&[(3.5, 6.0), (10.0, 9.5), (16.5, 6.0)]);
            thin.line(&[(10.0, 9.5), (10.0, 17.5)]);
            match icon {
                Icon::ModeVertex => pen.dot(10.0, 9.5, 2.6),
                Icon::ModeEdge => {
                    let bold = Pen { p, r: rect, s: Stroke::new(pen.s.width * 2.0, pen.s.color) };
                    bold.line(&[(10.0, 9.5), (10.0, 17.5)]);
                }
                _ => {}
            }
        }
        Icon::Isolate => {
            // one block alone in front, the others faded behind it (Romain
            // 20 nov., 10 h 57: the four corners read as a focus)
            let ghost = Pen { p, r: rect, s: Stroke::new(pen.s.width * 0.75, pen.s.color.gamma_multiply(0.32)) };
            for cx in [4.2, 15.8] {
                let h = [(cx, 2.5), (cx + 3.2, 4.3), (cx + 3.2, 8.3), (cx, 10.1), (cx - 3.2, 8.3), (cx - 3.2, 4.3)];
                ghost.closed(&h);
                ghost.line(&[(cx - 3.2, 4.3), (cx, 6.1), (cx + 3.2, 4.3)]);
                ghost.line(&[(cx, 6.1), (cx, 10.1)]);
            }
            let hex = [(10.0, 6.0), (15.4, 9.0), (15.4, 15.0), (10.0, 18.0), (4.6, 15.0), (4.6, 9.0)];
            let top = [(10.0, 6.0), (15.4, 9.0), (10.0, 12.0), (4.6, 9.0)];
            pen.fill(&hex, 0.45);
            pen.fill(&top, 0.5);
            pen.closed(&hex);
            pen.line(&[(4.6, 9.0), (10.0, 12.0), (15.4, 9.0)]);
            pen.line(&[(10.0, 12.0), (10.0, 18.0)]);
        }
        Icon::Xray => {
            let hex = [(10.0, 2.5), (16.5, 6.0), (16.5, 14.0), (10.0, 17.5), (3.5, 14.0), (3.5, 6.0)];
            pen.fill(&hex, 0.25);
            pen.closed(&hex);
            pen.line(&[(3.5, 6.0), (10.0, 9.5), (16.5, 6.0)]);
            pen.line(&[(10.0, 9.5), (10.0, 17.5)]);
            let thin = Pen { p, r: rect, s: Stroke::new(pen.s.width * 0.75, pen.s.color.gamma_multiply(0.55)) };
            thin.line(&[(3.5, 14.0), (10.0, 10.5), (16.5, 14.0)]);
            thin.line(&[(10.0, 10.5), (10.0, 2.5)]);
        }
        Icon::Inset => {
            pen.closed(&[(3.0, 3.0), (17.0, 3.0), (17.0, 17.0), (3.0, 17.0)]);
            pen.fill(&[(7.0, 7.0), (13.0, 7.0), (13.0, 13.0), (7.0, 13.0)], 0.5);
            pen.closed(&[(7.0, 7.0), (13.0, 7.0), (13.0, 13.0), (7.0, 13.0)]);
            for (a, b) in [((3.0, 3.0), (7.0, 7.0)), ((17.0, 3.0), (13.0, 7.0)), ((17.0, 17.0), (13.0, 13.0)), ((3.0, 17.0), (7.0, 13.0))] {
                pen.line(&[a, b]);
            }
        }
        Icon::Extrude => {
            let face = [(2.5, 13.5), (10.0, 10.0), (17.5, 13.5), (10.0, 17.0)];
            pen.fill(&face, 0.45);
            pen.closed(&face);
            pen.line(&[(10.0, 13.5), (10.0, 2.5)]);
            pen.line(&[(7.0, 5.5), (10.0, 2.5), (13.0, 5.5)]);
        }
        Icon::TabForme => {
            // a block drilled through its top (the B-rep part)
            pen.closed(&[(10.0, 2.5), (16.5, 6.0), (16.5, 14.0), (10.0, 17.5), (3.5, 14.0), (3.5, 6.0)]);
            pen.line(&[(3.5, 6.0), (10.0, 9.5), (16.5, 6.0)]);
            pen.line(&[(10.0, 9.5), (10.0, 17.5)]);
            pen.ellipse(10.0, 6.0, 3.0, 1.6);
        }
        Icon::TabSculpt => {
            // a lump of clay, a thumb's dent
            let pts: Vec<(f32, f32)> = (0..=40)
                .map(|i| {
                    let a = i as f32 / 40.0 * 2.0 * PI;
                    let r = 6.6 + 0.9 * (3.0 * a).sin() + 0.5 * (5.0 * a + 1.0).cos();
                    (10.0 + r * a.cos(), 10.5 + r * 0.92 * a.sin())
                })
                .collect();
            pen.line(&pts);
            pen.arc(8.5, 9.0, 2.6, 1.8, 0.2, PI - 0.2);
        }
        Icon::TabPoly => {
            pen.closed(&[(10.0, 2.5), (16.5, 6.0), (16.5, 14.0), (10.0, 17.5), (3.5, 14.0), (3.5, 6.0)]);
            pen.line(&[(3.5, 6.0), (10.0, 9.5), (16.5, 6.0)]);
            pen.line(&[(10.0, 9.5), (10.0, 17.5)]);
            pen.line(&[(6.75, 4.25), (13.25, 7.75), (13.25, 15.75)]);
            pen.line(&[(3.5, 10.0), (10.0, 13.5), (16.5, 10.0)]);
            for (x, y) in [(10.0, 2.5), (16.5, 6.0), (16.5, 14.0), (10.0, 17.5), (3.5, 14.0), (3.5, 6.0), (10.0, 9.5)] {
                pen.dot(x, y, 1.0);
            }
        }
        Icon::BrushDraw => {
            // a stroke raised along a line
            pen.line(&[(2.5, 15.5), (17.5, 15.5)]);
            pen.arc(10.0, 15.5, 6.0, 7.0, PI, 2.0 * PI);
            pen.line(&[(10.0, 6.0), (10.0, 2.5)]);
        }
        Icon::BrushClay => {
            // strips laid flat, one over the other
            pen.closed(&[(3.0, 12.0), (14.0, 12.0), (17.0, 15.5), (6.0, 15.5)]);
            pen.closed(&[(5.0, 7.5), (13.0, 7.5), (15.5, 10.0), (7.5, 10.0)]);
            pen.fill(&[(5.0, 7.5), (13.0, 7.5), (15.5, 10.0), (7.5, 10.0)], 0.4);
            pen.line(&[(8.0, 3.5), (12.0, 3.5)]);
        }
        Icon::BrushBuildup => {
            // square slabs laid one over the other, the top one filled
            pen.closed(&[(2.5, 14.5), (13.0, 14.5), (17.5, 17.5), (7.0, 17.5)]);
            pen.closed(&[(4.0, 10.0), (12.5, 10.0), (16.0, 12.8), (7.5, 12.8)]);
            let top = [(5.5, 5.5), (12.0, 5.5), (14.5, 8.2), (8.0, 8.2)];
            pen.closed(&top);
            pen.fill(&top, 0.45);
            pen.line(&[(10.0, 1.5), (10.0, 4.0)]);
        }
        Icon::BrushSmooth => {
            // a wave evened out
            let wave: Vec<(f32, f32)> = (0..=24).map(|i| {
                let x = 2.5 + 15.0 * i as f32 / 24.0;
                (x, 7.0 + 2.2 * ((x - 2.5) * 0.9).sin())
            }).collect();
            pen.line(&wave);
            pen.line(&[(10.0, 10.5), (10.0, 13.0)]);
            pen.line(&[(8.5, 11.8), (10.0, 13.3), (11.5, 11.8)]);
            pen.line(&[(2.5, 15.5), (17.5, 15.5)]);
        }
        Icon::BrushGrab => {
            // a bump pulled up by its top
            pen.line(&[(2.5, 16.0), (6.0, 16.0)]);
            pen.line(&[(14.0, 16.0), (17.5, 16.0)]);
            pen.line(&[(6.0, 16.0), (8.5, 9.0), (10.0, 7.5), (11.5, 9.0), (14.0, 16.0)]);
            pen.line(&[(13.0, 6.5), (16.5, 3.0)]);
            pen.line(&[(13.5, 3.0), (16.5, 3.0), (16.5, 6.0)]);
        }
        Icon::BrushInflate => {
            pen.circle(10.0, 10.0, 4.0);
            for a in [0.0f32, 0.5, 1.0, 1.5] {
                let a = a * PI;
                let (c, s) = (a.cos(), a.sin());
                pen.line(&[(10.0 + c * 5.5, 10.0 + s * 5.5), (10.0 + c * 8.0, 10.0 + s * 8.0)]);
            }
        }
        Icon::BrushFlatten => {
            // a bump pressed flat by a plane
            pen.line(&[(2.5, 16.0), (5.0, 16.0), (7.0, 11.0), (13.0, 11.0), (15.0, 16.0), (17.5, 16.0)]);
            pen.line(&[(3.5, 8.5), (16.5, 8.5)]);
            pen.line(&[(10.0, 2.5), (10.0, 6.5)]);
            pen.line(&[(8.5, 5.0), (10.0, 6.5), (11.5, 5.0)]);
        }
        Icon::BrushPinch => {
            // two sides drawn to a crease
            pen.line(&[(2.5, 7.0), (10.0, 15.0), (17.5, 7.0)]);
            pen.line(&[(3.0, 13.0), (6.5, 13.0)]);
            pen.line(&[(5.0, 11.5), (6.5, 13.0), (5.0, 14.5)]);
            pen.line(&[(17.0, 13.0), (13.5, 13.0)]);
            pen.line(&[(15.0, 11.5), (13.5, 13.0), (15.0, 14.5)]);
        }
        Icon::PolyPlane => {
            let q = [(2.5, 12.0), (10.0, 8.0), (17.5, 12.0), (10.0, 16.0)];
            pen.closed(&q);
            pen.line(&[(6.25, 10.0), (13.75, 14.0)]);
            pen.line(&[(13.75, 10.0), (6.25, 14.0)]);
        }
        Icon::PolyBevel => {
            // a block whose corner is rounded in three rows
            let o = [(3.0, 17.0), (3.0, 10.0), (3.9, 6.6), (6.6, 3.9), (10.0, 3.0), (17.0, 3.0), (17.0, 17.0)];
            pen.closed(&o);
            pen.line(&[(3.9, 6.6), (8.0, 8.4)]);
            pen.line(&[(6.6, 3.9), (8.4, 8.0)]);
            pen.line(&[(3.0, 10.0), (8.0, 10.0)]);
            pen.line(&[(10.0, 3.0), (10.0, 8.0)]);
            for (x, y) in [(3.9, 6.6), (6.6, 3.9)] {
                pen.dot(x, y, 0.9);
            }
        }
        Icon::PolyCut => {
            // a quad cut across by a line of points
            pen.closed(&[(3.0, 4.0), (17.0, 4.0), (17.0, 16.0), (3.0, 16.0)]);
            pen.line(&[(7.0, 4.0), (12.5, 16.0)]);
            pen.dot(7.0, 4.0, 1.4);
            pen.dot(12.5, 16.0, 1.4);
        }
        Icon::EdgeLoop => {
            pen.closed(&[(10.0, 2.5), (16.5, 6.0), (16.5, 14.0), (10.0, 17.5), (3.5, 14.0), (3.5, 6.0)]);
            pen.line(&[(3.5, 6.0), (10.0, 9.5), (16.5, 6.0)]);
            pen.line(&[(10.0, 9.5), (10.0, 17.5)]);
            pen.line(&[(3.5, 10.0), (10.0, 13.5), (16.5, 10.0)]);
            pen.dot(3.5, 10.0, 1.1);
            pen.dot(16.5, 10.0, 1.1);
        }
        Icon::Subdivide => {
            pen.closed(&[(3.0, 3.0), (17.0, 3.0), (17.0, 17.0), (3.0, 17.0)]);
            pen.line(&[(10.0, 3.0), (10.0, 17.0)]);
            pen.line(&[(3.0, 10.0), (17.0, 10.0)]);
            pen.dot(10.0, 10.0, 1.2);
        }
        Icon::SmoothMesh => {
            // the cage, the smooth shape inside it
            let cage = [(3.0, 3.0), (17.0, 3.0), (17.0, 17.0), (3.0, 17.0)];
            for i in 0..4 {
                let (a, b) = (cage[i], cage[(i + 1) % 4]);
                for k in 0..4 {
                    let t0 = k as f32 / 4.0 + 0.04;
                    let t1 = t0 + 0.14;
                    pen.line(&[(a.0 + (b.0 - a.0) * t0, a.1 + (b.1 - a.1) * t0), (a.0 + (b.0 - a.0) * t1, a.1 + (b.1 - a.1) * t1)]);
                }
            }
            pen.circle(10.0, 10.0, 5.2);
        }
        Icon::ToSculpt => {
            pen.closed(&[(2.5, 5.0), (8.5, 5.0), (8.5, 11.0), (2.5, 11.0)]);
            pen.line(&[(9.5, 8.0), (12.0, 8.0)]);
            pen.line(&[(11.0, 6.8), (12.2, 8.0), (11.0, 9.2)]);
            let pts: Vec<(f32, f32)> = (0..=24)
                .map(|i| {
                    let a = i as f32 / 24.0 * 2.0 * PI;
                    let r = 3.6 + 0.5 * (3.0 * a).sin();
                    (14.5 + r * a.cos(), 12.0 + r * a.sin())
                })
                .collect();
            pen.line(&pts);
        }
        Icon::Scale => {
            // a small square growing into a big one
            pen.closed(&[(3.0, 9.0), (11.0, 9.0), (11.0, 17.0), (3.0, 17.0)]);
            pen.line(&[(7.0, 3.0), (17.0, 3.0), (17.0, 13.0)]);
            pen.line(&[(11.0, 9.0), (16.5, 3.5)]);
            pen.line(&[(13.0, 3.5), (16.5, 3.5), (16.5, 7.0)]);
        }
    }
}
