//! Vector icons, drawn in a 20×20 design space (same drawings as the mock-up).

use egui::{epaint::PathStroke, pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// Align (3ds Max's): two bars lined up on a line.
    Align,
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
    /// Pattern / Array: one shape and its copies in a row.
    Array,
    /// Shell: a block hollowed, open at the top (its walls in section).
    Shell,
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
    /// A group: two shapes inside the corners of a box.
    Group,
    /// A group closed (a padlock shut) or open (its shackle free).
    Lock,
    Unlock,
    /// The navigation island (`nav.rs`): a magnifier (zoom), four arrows
    /// (pan).
    Zoom,
    Pan,
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
        Icon::Array => {
            // the original filled, its copies drawn
            let sq = |x: f32, y: f32| [(x - 2.4, y - 2.4), (x + 2.4, y - 2.4), (x + 2.4, y + 2.4), (x - 2.4, y + 2.4)];
            pen.fill(&sq(4.5, 13.0), 0.9);
            pen.closed(&sq(4.5, 13.0));
            pen.closed(&sq(10.0, 10.0));
            pen.closed(&sq(15.5, 7.0));
        }
        Icon::Shell => {
            // a cup in section: its walls and floor, open at the top
            pen.closed(&[(3.0, 4.5), (3.0, 16.0), (17.0, 16.0), (17.0, 4.5), (14.0, 4.5), (14.0, 13.0), (6.0, 13.0), (6.0, 4.5)]);
            pen.fill(&[(3.0, 13.0), (3.0, 16.0), (17.0, 16.0), (17.0, 13.0)], 0.35);
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
        Icon::Align => {
            pen.line(&[(4.0, 2.5), (4.0, 17.5)]);
            pen.closed(&[(6.5, 5.0), (16.5, 5.0), (16.5, 9.0), (6.5, 9.0)]);
            pen.closed(&[(6.5, 11.5), (12.5, 11.5), (12.5, 15.5), (6.5, 15.5)]);
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
        Icon::Group => {
            for (cx, cy, dx, dy) in [(2.5, 2.5, 1.0, 1.0), (17.5, 2.5, -1.0, 1.0), (17.5, 17.5, -1.0, -1.0), (2.5, 17.5, 1.0, -1.0)] {
                pen.line(&[(cx + 3.6 * dx, cy), (cx, cy), (cx, cy + 3.6 * dy)]);
            }
            pen.closed(&[(5.6, 5.6), (11.2, 5.6), (11.2, 11.2), (5.6, 11.2)]);
            pen.circle(12.2, 12.2, 2.9);
        }
        Icon::Lock | Icon::Unlock => {
            pen.closed(&[(4.5, 9.2), (15.5, 9.2), (15.5, 17.0), (4.5, 17.0)]);
            pen.dot(10.0, 13.1, 1.15);
            if icon == Icon::Lock {
                pen.arc(10.0, 6.6, 3.4, 3.8, PI, 2.0 * PI);
                pen.line(&[(6.6, 6.6), (6.6, 9.2)]);
                pen.line(&[(13.4, 6.6), (13.4, 9.2)]);
            } else {
                // the shackle swung open: its left leg holds, the right end is free
                pen.arc(10.0, 5.2, 3.4, 3.2, PI, 2.0 * PI);
                pen.line(&[(6.6, 5.2), (6.6, 9.2)]);
                pen.line(&[(13.4, 5.2), (13.4, 6.4)]);
            }
        }
        Icon::Zoom => {
            pen.circle(8.6, 8.6, 5.6);
            pen.line(&[(12.7, 12.7), (17.0, 17.0)]);
            pen.line(&[(6.0, 8.6), (11.2, 8.6)]);
            pen.line(&[(8.6, 6.0), (8.6, 11.2)]);
        }
        Icon::Pan => {
            pen.line(&[(10.0, 2.5), (10.0, 17.5)]);
            pen.line(&[(2.5, 10.0), (17.5, 10.0)]);
            for (x, y, dx, dy) in [(10.0, 2.5, 0.0, 1.0), (10.0, 17.5, 0.0, -1.0), (2.5, 10.0, 1.0, 0.0), (17.5, 10.0, -1.0, 0.0)] {
                // the arrow's two barbs, square to its line
                pen.line(&[(x + 2.6 * dx - 2.6 * dy, y + 2.6 * dy - 2.6 * dx), (x, y), (x + 2.6 * dx + 2.6 * dy, y + 2.6 * dy + 2.6 * dx)]);
            }
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
