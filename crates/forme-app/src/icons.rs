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
    Bake,
    /// Rotation gizmo (E).
    Rotate,
    /// Move gizmo (W).
    Move,
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
        Icon::Bake => {
            pen.closed(&[(3.0, 3.0), (17.0, 3.0), (17.0, 17.0), (3.0, 17.0)]);
            pen.line(&[(3.0, 7.7), (17.0, 7.7)]);
            pen.line(&[(3.0, 12.3), (17.0, 12.3)]);
            pen.line(&[(7.7, 3.0), (7.7, 17.0)]);
            pen.line(&[(12.3, 3.0), (12.3, 17.0)]);
        }
    }
}
