//! Low-level mouse hook (1.1) + the pass-through rules that are cheap enough
//! to run in the hook callback (CLAUDE.md §7.4). Everything expensive
//! (config snapshot, window lookup) is a cache hit in the common case.
//!
//! The callback must return in well under the ~300 ms `LowLevelHooksTimeout`
//! (target: p99 < 100 µs, NFR table) or Windows silently unhooks it, so it
//! never allocates on the swallow path and the window lookup is TTL-cached.

use super::window::WindowCache;
use super::EXTRA_INFO_TAG;
use crate::config::Modifier;
use crate::engine::{Axis, WheelInput};
use crate::state::{QueuedInput, Shared};
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HHOOK, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT,
    WH_MOUSE_LL, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT,
};

/// The one hook instance this process installs; only the hook thread reads it.
static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
/// So `stop()` (called from the UI thread) can wake the hook thread's
/// `GetMessageW` loop with `WM_QUIT`.
static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static ENABLED_HINT: AtomicI32 = AtomicI32::new(1);

fn key_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    // SAFETY: `GetAsyncKeyState` takes a plain virtual-key code; no pointers.
    (unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000) != 0
}

fn modifier_held(m: Modifier) -> bool {
    match m {
        Modifier::None => false,
        Modifier::Alt => key_down(VK_MENU),
        Modifier::Ctrl => key_down(VK_CONTROL),
        Modifier::Shift => key_down(VK_SHIFT),
        Modifier::Win => key_down(VK_LWIN) || key_down(VK_RWIN),
    }
}

thread_local! {
    static WINDOW_CACHE: std::cell::RefCell<WindowCache> = std::cell::RefCell::new(WindowCache::new());
}

/// Runs entirely inside the OS callback: decide swallow-and-queue vs.
/// pass-through. Returns `Some(true)` to swallow, `Some(false)`/`None` to
/// pass through via `CallNextHookEx`.
fn handle_wheel(msg: u32, ll: &MSLLHOOKSTRUCT) -> bool {
    // Rule 1: never re-process our own injected output.
    if (ll.flags & LLMHF_INJECTED.0) != 0 && ll.dwExtraInfo.0 == EXTRA_INFO_TAG {
        return false;
    }

    let Some(shared) = SHARED.get() else {
        return false;
    };

    // Rule 2: disabled/paused.
    if !shared.enabled() {
        return false;
    }

    let delta = ((ll.mouseData >> 16) & 0xFFFF) as i16 as i32;

    // Rule 3: precision devices emit deltas that aren't multiples of 120.
    if delta % crate::engine::WHEEL_DELTA != 0 {
        return false;
    }

    let (_, cfg) = shared.snapshot();

    // Rule 4: bypass modifier held.
    if modifier_held(cfg.bypass_modifier) {
        return false;
    }

    let ctrl_held = key_down(VK_CONTROL);
    let shift_held = key_down(VK_SHIFT);

    let axis = if msg == WM_MOUSEHWHEEL {
        if !cfg.base.horizontal_enabled {
            return false;
        }
        Axis::Horizontal
    } else {
        // Rule 6: Ctrl+wheel (zoom) pass-through, unless we're about to
        // reinterpret it as Shift+wheel horizontal scroll.
        if ctrl_held && cfg.ctrl_passthrough {
            return false;
        }
        if shift_held && cfg.base.shift_horizontal && cfg.base.horizontal_enabled {
            Axis::Horizontal
        } else {
            Axis::Vertical
        }
    };

    // Rule 5: per-app exclude/allowlist + fullscreen-game auto-disable.
    let exe = WINDOW_CACHE.with(|c| c.borrow_mut().exe_under_cursor().to_owned());
    if !exe.is_empty() && !cfg.app_allowed(&exe) {
        return false;
    }
    if cfg.auto_disable_in_fullscreen && shared.fullscreen() {
        return false;
    }
    let profile = cfg.profile_for(&exe);
    if !cfg.settings_for_profile(profile).enabled {
        return false;
    }

    let queued = QueuedInput {
        input: WheelInput {
            delta,
            axis,
            t_us: 0, // filled in by the animator's own clock on drain
        },
        profile,
    };
    if shared.queue.push(queued) {
        shared.wake();
    }
    // Full or not, this event has been claimed: never let it double-scroll.
    true
}

/// The actual `WH_MOUSE_LL` callback. Must be `extern "system"` and return
/// promptly; all real logic lives in `handle_wheel` above.
unsafe extern "system" fn low_level_mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && (wparam.0 as u32 == WM_MOUSEWHEEL || wparam.0 as u32 == WM_MOUSEHWHEEL) {
        // SAFETY: for `code >= 0`, Windows guarantees `lparam` points to a
        // valid `MSLLHOOKSTRUCT` for the duration of this callback.
        let ll = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if handle_wheel(wparam.0 as u32, ll) {
            return LRESULT(1);
        }
    }
    // SAFETY: `code`/`wparam`/`lparam` are passed through unmodified, and
    // there is no other active hook handle for this thread.
    unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) }
}

/// Runs the hook on the calling thread until [`stop`] is called (or the
/// process is torn down). Never returns from a panic without unhooking.
pub fn run(shared: Arc<Shared>) {
    let _ = SHARED.set(shared);
    // SAFETY: `GetCurrentThreadId` has no preconditions.
    HOOK_THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::Release);

    // SAFETY: `low_level_mouse_proc` is `extern "system"`, matches the
    // `HOOKPROC` signature, and stays valid for the process lifetime
    // (it's a plain `fn`, not a closure). `HINSTANCE::default()` + thread id
    // 0 is the documented way to install a *global* low-level hook.
    let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), HINSTANCE::default(), 0) };
    let Ok(hook) = hook else {
        return; // Caller sees no smoothing; logged by the app shell.
    };

    let mut msg = MSG::default();
    // SAFETY: `GetMessageW`/`TranslateMessage`/`DispatchMessageW` on a
    // stack-local `MSG` is the standard Win32 message-loop pattern.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Always unhook, on both `WM_QUIT` and any early exit.
        let _ = UnhookWindowsHookEx(hook);
    }
    ENABLED_HINT.store(0, Ordering::Relaxed);
}

/// Ask the hook thread's message loop to exit (FR-22: hook is always
/// removed on shutdown).
pub fn stop() {
    let tid = HOOK_THREAD_ID.load(Ordering::Acquire);
    if tid != 0 {
        // SAFETY: posting `WM_QUIT` to a thread id we obtained ourselves;
        // if the thread has already exited this is a documented no-op error.
        let _ = unsafe { PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}
