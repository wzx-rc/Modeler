//! The pen (Romain, 22 nov. 2026, 16 h 49: "La navigation doit pouvoir se
//! faire full stylet").
//!
//! On Windows a pen with Windows Ink reaches winit as WM_POINTER messages,
//! which winit makes touches: egui then took the pen for a finger (a press
//! held still 0.8 s was a long touch that dropped the drag, the pointer
//! was gone at each lift) and the pen's barrel button was lost (winit reads
//! no button of a pointer). Here a pen's touches become the mouse's events
//! (the tip is the left button; hovering moves the pointer), and its barrel
//! button, read from the pointer's pen info, counts as Alt in the view: the
//! button held and the tip dragged turns the view, with Shift it pans, with
//! Ctrl it zooms (`viewport::handle_input`). A finger stays a touch. A
//! tablet whose driver plays the mouse (Windows Ink off) sends mouse events
//! already, its buttons whatever the driver says.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use winit::event::{ElementState, MouseButton, Touch, TouchPhase, WindowEvent};

/// The barrel button held at the pen's last event.
static BARREL: AtomicBool = AtomicBool::new(false);
/// The pen is the pointer (a mouse moved since puts it back to the mouse).
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The pen's last event, in ms since `START`.
static LAST: AtomicU64 = AtomicU64::new(0);
static START: Mutex<Option<Instant>> = Mutex::new(None);
/// The pointer ids known to be pens (a pointer's type is asked once, while
/// it is in range; its last event, the tip let go, may come when it no
/// longer answers).
static PENS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn now_ms() -> u64 {
    let mut s = START.lock().unwrap();
    let t0 = *s.get_or_insert_with(Instant::now);
    t0.elapsed().as_millis() as u64
}

/// The pen's barrel button is held (and the pen is the pointer now).
pub fn barrel() -> bool {
    ACTIVE.load(Ordering::Relaxed) && BARREL.load(Ordering::Relaxed)
}

/// The pen is the pointer now (its last event came after the mouse's).
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// For the capture harness: the barrel button held or let go, as if the
/// pen were the pointer.
pub fn set_barrel(held: bool) {
    BARREL.store(held, Ordering::Relaxed);
    ACTIVE.store(held, Ordering::Relaxed);
}

/// A mouse event came: the mouse is the pointer again, unless it is the
/// echo of the pen's own (within 100 ms of its last).
pub fn mouse_seen() {
    if ACTIVE.load(Ordering::Relaxed) && now_ms().saturating_sub(LAST.load(Ordering::Relaxed)) > 100 {
        ACTIVE.store(false, Ordering::Relaxed);
        BARREL.store(false, Ordering::Relaxed);
    }
}

/// A touch from winit: when it is a pen's, the mouse's events to give egui
/// in its place (the barrel button noted); None for a finger.
pub fn as_mouse(t: &Touch) -> Option<Vec<WindowEvent>> {
    let known = PENS.lock().unwrap().contains(&t.id);
    let barrel = match pen_info(t.id) {
        Some(b) => {
            if !known {
                let mut v = PENS.lock().unwrap();
                v.push(t.id);
                // (a pen keeps its id; a few at most)
                if v.len() > 16 {
                    v.remove(0);
                }
            }
            b
        }
        None if known => BARREL.load(Ordering::Relaxed),
        None => return None,
    };
    BARREL.store(barrel, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Relaxed);
    LAST.store(now_ms(), Ordering::Relaxed);
    Some(mouse_events(t))
}

/// The mouse's events for a pen's touch: the pointer moved there, then the
/// left button pressed when the tip touches, let go when it leaves.
pub fn mouse_events(t: &Touch) -> Vec<WindowEvent> {
    let mut v = vec![WindowEvent::CursorMoved { device_id: t.device_id, position: t.location }];
    let state = match t.phase {
        TouchPhase::Started => Some(ElementState::Pressed),
        TouchPhase::Ended | TouchPhase::Cancelled => Some(ElementState::Released),
        TouchPhase::Moved => None,
    };
    if let Some(state) = state {
        v.push(WindowEvent::MouseInput { device_id: t.device_id, state, button: MouseButton::Left });
    }
    v
}

/// The barrel button of the pointer `id` when it is a pen (Windows: the
/// pen info of the message being handled), None when it is not a pen.
#[cfg(windows)]
fn pen_info(id: u64) -> Option<bool> {
    use windows_sys::Win32::UI::Input::Pointer::{GetPointerPenInfo, GetPointerType, POINTER_FLAG_SECONDBUTTON, POINTER_PEN_INFO};
    use windows_sys::Win32::UI::WindowsAndMessaging::{PEN_FLAG_BARREL, PT_PEN};
    let id = id as u32;
    let mut kind = 0;
    // SAFETY: plain Win32 calls on a pointer id, the out values on the stack
    if unsafe { GetPointerType(id, &mut kind) } == 0 || kind != PT_PEN {
        return None;
    }
    let mut info: POINTER_PEN_INFO = Default::default();
    if unsafe { GetPointerPenInfo(id, &mut info) } == 0 {
        return Some(BARREL.load(Ordering::Relaxed));
    }
    Some(info.penFlags & PEN_FLAG_BARREL != 0 || info.pointerInfo.pointerFlags & POINTER_FLAG_SECONDBUTTON != 0)
}

#[cfg(not(windows))]
fn pen_info(_id: u64) -> Option<bool> {
    // elsewhere a tablet's pen comes as the mouse already
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;
    use winit::event::DeviceId;

    fn touch(phase: TouchPhase) -> Touch {
        // SAFETY: a dummy device id, only compared
        Touch { device_id: unsafe { DeviceId::dummy() }, phase, location: PhysicalPosition::new(120.0, 80.0), force: None, id: 7 }
    }

    #[test]
    fn a_pen_touch_is_the_mouse() {
        let down = mouse_events(&touch(TouchPhase::Started));
        assert!(matches!(down[0], WindowEvent::CursorMoved { position, .. } if position.x == 120.0 && position.y == 80.0));
        assert!(matches!(down[1], WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. }));
        let hover = mouse_events(&touch(TouchPhase::Moved));
        assert_eq!(hover.len(), 1);
        let up = mouse_events(&touch(TouchPhase::Ended));
        assert!(matches!(up[1], WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. }));
        // not a pen here (no Windows): a finger stays a touch
        #[cfg(not(windows))]
        assert!(as_mouse(&touch(TouchPhase::Started)).is_none());
    }
}
