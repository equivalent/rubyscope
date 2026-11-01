//! Makes the mouse wheel scroll faster everywhere in the app: the side bar,
//! the code, the lists. It changes the wheel events before egui sees them.

use eframe::egui::{Event, RawInput};

/// Every wheel step scrolls this many times further than before.
pub const WHEEL_BOOST: f32 = 1.5;
/// With Ctrl (Cmd on macOS) held, the wheel scrolls this many times faster.
pub const CTRL_BOOST: f32 = 3.0;

/// Scales the wheel events in `raw`. Ctrl and the wheel scroll 3 times
/// faster instead of zooming, so Ctrl is taken off the wheel event. Shift
/// and the wheel are left alone: they scroll one page per step.
pub fn speed_up(raw: &mut RawInput) {
    for event in &mut raw.events {
        if let Event::MouseWheel { delta, modifiers, .. } = event {
            if modifiers.shift {
                continue;
            }
            let mut factor = WHEEL_BOOST;
            if modifiers.command || crate::keys::cmd_held(*modifiers) {
                factor *= CTRL_BOOST;
                modifiers.ctrl = false;
                modifiers.command = false;
                modifiers.mac_cmd = false;
            }
            *delta *= factor;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase, Vec2};

    fn wheel(modifiers: Modifiers) -> RawInput {
        RawInput {
            events: vec![Event::MouseWheel {
                unit: MouseWheelUnit::Line,
                delta: Vec2::new(0.0, -1.0),
                phase: TouchPhase::Move,
                source: MouseWheelSource::Unknown,
                modifiers,
            }],
            ..Default::default()
        }
    }

    fn after(modifiers: Modifiers) -> (Vec2, Modifiers) {
        let mut raw = wheel(modifiers);
        speed_up(&mut raw);
        match raw.events[0] {
            Event::MouseWheel { delta, modifiers, .. } => (delta, modifiers),
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_wheel_scrolls_further() {
        let (delta, _) = after(Modifiers::NONE);
        assert_eq!(delta.y, -WHEEL_BOOST);
    }

    #[test]
    fn ctrl_and_the_wheel_scroll_three_times_faster_and_do_not_zoom() {
        let (delta, modifiers) = after(Modifiers::COMMAND);
        assert_eq!(delta.y, -WHEEL_BOOST * CTRL_BOOST);
        assert!(!crate::keys::cmd_held(modifiers));
        assert!(!modifiers.command);
    }

    #[test]
    fn shift_and_the_wheel_stay_one_page_per_step() {
        let (delta, modifiers) = after(Modifiers::SHIFT);
        assert_eq!(delta.y, -1.0);
        assert!(modifiers.shift);
    }
}
