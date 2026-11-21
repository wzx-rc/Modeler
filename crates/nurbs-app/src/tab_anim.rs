//! The change of tab animated (Romain, 20 nov. 2026, 17 h 15: "un effet
//! animé de blending ou d'animation" between the three tabs, "de façon à ce
//! que, si je te le dis, tu puisses la retirer").
//!
//! For about a third of a second after a new tab: the accent (and every
//! colour made from it: the tabs, the rail's cards, the selection) fades
//! from the old tab's to the new one's, the tabs' lit pill slides from the
//! old tab to the new one, and the new tab's own tools (the rail's cards,
//! the centre island's controls) come in, faded and a little
//! offset. Settings › Display › Tab animation turns it off.
//!
//! **To take it out**: delete this file, its `pub mod tab_anim;` in lib.rs,
//! the lines marked `tab_anim` (lib.rs `run`; theme.rs `BLEND`, `blend`,
//! `set_blend`, in `ws` and `set_theme`; ui.rs `tabs`, the centre island,
//! the rail, the Display toggle) and `Settings.tab_anim`.
//! Nothing else depends on it.

use crate::workspace::Workspace;
use egui::{emath::TSTransform, pos2, vec2, Context, Id, Rect, Ui};

/// How long it lasts, in seconds.
pub const DURATION: f64 = 0.34;

#[derive(Clone, Copy, Debug)]
struct Anim {
    from: Workspace,
    to: Workspace,
    start: f64,
}

fn anim_key() -> Id {
    Id::new("tab-anim")
}

fn last_key() -> Id {
    Id::new("tab-anim-last")
}

/// Once per frame, before the theme is set: a new tab starts it, and the
/// theme gets the blend of the colours meanwhile.
pub fn tick(ctx: &Context, now: Workspace, on: bool) {
    let last: Option<u8> = ctx.data(|d| d.get_temp(last_key()));
    let time = ctx.input(|i| i.time);
    if let Some(l) = last {
        if l != now.index() {
            if on {
                // a change while one runs starts from where that one goes
                ctx.data_mut(|d| d.insert_temp(anim_key(), Anim { from: Workspace::from_index(l), to: now, start: time }));
            } else {
                ctx.data_mut(|d| d.remove::<Anim>(anim_key()));
            }
        }
    }
    ctx.data_mut(|d| d.insert_temp(last_key(), now.index()));
    let p = progress(ctx);
    crate::theme::set_blend(p.map(|(from, _, k)| (from, k)));
    if p.is_some() {
        ctx.request_repaint();
    }
}

/// While it runs: the tab it comes from, the tab it goes to, and how far,
/// eased out (0 to 1).
pub fn progress(ctx: &Context) -> Option<(Workspace, Workspace, f32)> {
    let a: Anim = ctx.data(|d| d.get_temp(anim_key()))?;
    let time = ctx.input(|i| i.time);
    let t = ((time - a.start) / DURATION).clamp(0.0, 1.0) as f32;
    if t >= 1.0 {
        ctx.data_mut(|d| d.remove::<Anim>(anim_key()));
        return None;
    }
    Some((a.from, a.to, ease_out(t)))
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// The tabs' lit pill: from the old tab's place to the new one's.
pub fn pill(ctx: &Context, rects: &[Rect; 4], now: Workspace) -> Rect {
    let to = rects[now.index() as usize];
    match progress(ctx) {
        Some((from, _, k)) => {
            let a = rects[from.index() as usize];
            Rect::from_min_max(a.min.lerp(to.min, k), a.max.lerp(to.max, k))
        }
        None => to,
    }
}

/// How lit a tab's label is: 1 on the current one, sliding with the pill.
pub fn lit(ctx: &Context, w: Workspace, now: Workspace) -> f32 {
    match progress(ctx) {
        Some((from, _, k)) if w == from => 1.0 - k,
        Some((_, _, k)) if w == now => k,
        _ => (w == now) as u8 as f32,
    }
}

/// The new tab's own tools coming in: faded, and moved by `slide` points
/// (x, y) at first.
pub fn enter<R>(ui: &mut Ui, slide: egui::Vec2, add: impl FnOnce(&mut Ui) -> R) -> R {
    let Some((_, _, k)) = progress(ui.ctx()) else { return add(ui) };
    let off = slide * (1.0 - k);
    ui.scope(|ui| {
        ui.multiply_opacity(0.15 + 0.85 * k);
        ui.with_visual_transform(TSTransform::from_translation(off), add).inner
    })
    .inner
}

/// A soft streak of the new accent under the tabs, swept from the old tab
/// to the new one and fading out (drawn on the view, under the islands).
pub fn streak(ctx: &Context, rects: &[Rect; 4]) {
    let Some((from, to, k)) = progress(ctx) else { return };
    let a = rects[from.index() as usize].center();
    let b = rects[to.index() as usize].center();
    let x = a.x + (b.x - a.x) * k;
    let y = rects[0].max.y + 7.0;
    let c = crate::theme::accent();
    let fade = (1.0 - k).powf(0.7);
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, Id::new("tab-anim-streak")));
    // a few stacked bars, wider and fainter outwards: a glow
    for (w, h, alpha) in [(150.0, 10.0, 0.05), (110.0, 6.0, 0.09), (70.0, 3.0, 0.22), (36.0, 1.5, 0.55)] {
        let r = Rect::from_center_size(pos2(x, y), vec2(w, h));
        p.rect_filled(r, h * 0.5, c.gamma_multiply(alpha * fade));
    }
}
