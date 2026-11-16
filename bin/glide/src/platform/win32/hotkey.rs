//! `RegisterHotKey`/`UnregisterHotKey` wrapper. Parsing `"ctrl+alt+s"` into
//! modifiers/vk is `config::Hotkey` (platform-independent, unit-tested);
//! this module only does the live Win32 registration.

use crate::config::Hotkey;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN,
};

/// The one global toggle hotkey Glide registers.
pub const TOGGLE_HOTKEY_ID: i32 = 1;

fn modifiers(hk: &Hotkey) -> HOT_KEY_MODIFIERS {
    let mut m = HOT_KEY_MODIFIERS(0);
    if hk.ctrl {
        m |= MOD_CONTROL;
    }
    if hk.alt {
        m |= MOD_ALT;
    }
    if hk.shift {
        m |= MOD_SHIFT;
    }
    if hk.win {
        m |= MOD_WIN;
    }
    m
}

/// Registers `hk` under [`TOGGLE_HOTKEY_ID`]. `hwnd` must be a window owned
/// by the calling thread (its message loop receives `WM_HOTKEY`).
pub fn register(hwnd: HWND, hk: &Hotkey) -> bool {
    // SAFETY: `hwnd` is caller-owned and live; `hk.vk` came from
    // `config::parse_hotkey`, a plain Win32 virtual-key code.
    unsafe { RegisterHotKey(hwnd, TOGGLE_HOTKEY_ID, modifiers(hk), hk.vk).is_ok() }
}

pub fn unregister(hwnd: HWND) {
    // SAFETY: unregisters the id we registered above; a no-op error if it
    // was never registered (e.g. an invalid configured hotkey).
    unsafe {
        let _ = UnregisterHotKey(hwnd, TOGGLE_HOTKEY_ID);
    }
}
