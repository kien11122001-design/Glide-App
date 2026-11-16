//! Safe wrapper around `SendInput` for wheel events (1.2).

use super::EXTRA_INFO_TAG;
use crate::engine::Output;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL, MOUSEINPUT,
};

fn mouse_event(dw_flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                // `mouseData` wants the wheel delta in the high-order word as
                // a signed value; the crate's binding takes it directly.
                mouseData: data as u32,
                dwFlags: dw_flags,
                time: 0,
                // Tag so the hook can recognize (and never re-swallow) our
                // own injected events (PRD §7.4 rule 1).
                dwExtraInfo: EXTRA_INFO_TAG,
            },
        },
    }
}

/// Inject one batch covering both axes in a single `SendInput` call when
/// both are non-zero (1.2: "batch multiple events... when possible").
pub fn send_wheel(out: Output) {
    let mut inputs = [INPUT::default(); 2];
    let mut n = 0usize;
    if out.v_units != 0 {
        inputs[n] = mouse_event(MOUSEEVENTF_WHEEL, out.v_units);
        n += 1;
    }
    if out.h_units != 0 {
        inputs[n] = mouse_event(MOUSEEVENTF_HWHEEL, out.h_units);
        n += 1;
    }
    if n == 0 {
        return;
    }
    // SAFETY: `inputs[..n]` is a valid, fully-initialized `INPUT` slice and
    // `size_of::<INPUT>()` matches the struct we just built; `SendInput`
    // only reads it. No pointers or memory ownership persist afterward.
    unsafe {
        SendInput(&inputs[..n], std::mem::size_of::<INPUT>() as i32);
    }
}
