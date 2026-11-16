//! Low-level mouse hook (1.1) + the pass-through rules that are cheap enough
//! to run in the hook callback (CLAUDE.md §7.4). Everything expensive
//! (config snapshot, window lookup) is a cache hit in the common case.
//!
//! The callback must return in well under the ~300 ms `LowLevelHooksTimeout`
//! (target: p99 < 100 µs, NFR table) or Windows silently unhooks it, so the
//! swallow path never allocates and the window lookup is TTL-cached (a cache
//! *miss* allocates one short string, at most once per window / 500 ms).

use super::timer::boost_thread_priority;
use super::window::WindowCache;
use super::EXTRA_INFO_TAG;
use crate::config::Modifier;
use crate::engine::{Axis, WheelInput, WHEEL_DELTA};
use crate::state::{QueuedInput, Shared};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HHOOK, LLMHF_INJECTED, LLMHF_LOWER_IL_INJECTED, MSG,
    MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_APP, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEHWHEEL,
    WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
};

/// The one hook instance this process installs; only the hook thread reads it.
static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
/// So `stop()`/`reinstall()` (called from the UI thread) can reach the hook
/// thread's `GetMessageW` loop.
static HOOK_THREAD_ID: AtomicU32 = AtomicU32::new(0);
/// Monotonic clock for wheel timestamps (acceleration cadence, FR-6).
static START: OnceLock<Instant> = OnceLock::new();
/// Rate limit for `reinstall` (ms on the `START` clock; 0 = never).
static LAST_REINSTALL_MS: AtomicU64 = AtomicU64::new(0);

const WM_REINSTALL: u32 = WM_APP + 2;
const MIN_REINSTALL_GAP_MS: u64 = 2_000;

/// Microseconds since the first call. No allocation, no syscall beyond QPC.
fn now_us() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    // SAFETY: `GetAsyncKeyState` takes a plain virtual-key code; no pointers.
    (unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0
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
/// pass-through. Returns `true` to swallow the event.
fn handle_wheel(msg: u32, ll: &MSLLHOOKSTRUCT) -> bool {
    // Rule 1: never re-process injected input. Our own output is always
    // injected (and tagged); events injected by other tools (touchpad
    // drivers, remote sessions, mouse software) are already "smooth" or
    // deliberate, so they pass through untouched too.
    if ll.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0
        || ll.dwExtraInfo == EXTRA_INFO_TAG
    {
        return false;
    }

    let Some(shared) = SHARED.get() else {
        return false;
    };

    // Rule 2: disabled/paused.
    if !shared.enabled() {
        return false;
    }

    // The signed wheel delta lives in the high word of `mouseData`.
    let delta = ((ll.mouseData >> 16) & 0xFFFF) as i16 as i32;

    // Rule 3: precision devices emit deltas that aren't multiples of 120.
    if delta == 0 || delta % WHEEL_DELTA != 0 {
        return false;
    }

    let (_, cfg) = shared.snapshot();

    // Rule 4: bypass modifier held.
    if modifier_held(cfg.bypass_modifier) {
        return false;
    }

    // Rule 6: Ctrl+wheel (zoom) stays native.
    let vertical_msg = msg == WM_MOUSEWHEEL;
    if vertical_msg && cfg.ctrl_passthrough && key_down(VK_CONTROL) {
        return false;
    }

    // Rule 5: per-app exclude/allowlist, UIPI-blocked (elevated) targets.
    let (allowed, profile, uipi_blocked) = WINDOW_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        let target = cache.target_under_cursor();
        (
            target.exe.is_empty() || cfg.app_allowed(&target.exe),
            cfg.profile_for(&target.exe),
            target.uipi_blocked,
        )
    });
    if !allowed || uipi_blocked {
        return false;
    }
    if cfg.auto_disable_in_fullscreen && shared.fullscreen() {
        return false;
    }

    let settings = cfg.settings_for_profile(profile);
    if !settings.enabled {
        return false;
    }

    let (axis, delta) = if vertical_msg {
        // FR-11: Shift+wheel scrolls horizontally. Wheel-down (negative)
        // must move content right (positive `HWHEEL`), hence the sign flip.
        if settings.shift_horizontal && settings.horizontal_enabled && key_down(VK_SHIFT) {
            (Axis::Horizontal, -delta)
        } else {
            (Axis::Vertical, delta)
        }
    } else if settings.horizontal_enabled {
        (Axis::Horizontal, delta)
    } else {
        return false;
    };

    let queued = QueuedInput {
        input: WheelInput {
            delta,
            axis,
            t_us: now_us(),
        },
        profile,
    };
    if shared.queue.push(queued) {
        shared.wake();
    }
    // Full or not, this event has been claimed: never let it double-scroll.
    true
}

fn is_button_down(msg: u32) -> bool {
    msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN || msg == WM_MBUTTONDOWN || msg == WM_XBUTTONDOWN
}

/// The actual `WH_MOUSE_LL` callback. Must be `extern "system"` and return
/// promptly; all real logic lives in `handle_wheel` above.
///
/// # Safety
/// Only ever invoked by Windows as a `WH_MOUSE_LL` hook procedure: for
/// `code >= 0`, `lparam` points to a valid `MSLLHOOKSTRUCT`.
unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        let msg = wparam.0 as u32;
        if msg == WM_MOUSEWHEEL || msg == WM_MOUSEHWHEEL {
            // SAFETY: for `code >= 0`, Windows guarantees `lparam` points to a
            // valid `MSLLHOOKSTRUCT` for the duration of this callback.
            let ll = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            if handle_wheel(msg, ll) {
                return LRESULT(1);
            }
        } else if is_button_down(msg) {
            // 7.1: a click stops tablet-style inertia (never swallowed).
            if let Some(shared) = SHARED.get() {
                if shared.cancel_on_click() {
                    shared.request_cancel();
                }
            }
        }
    }
    // SAFETY: `code`/`wparam`/`lparam` are passed through unmodified, and
    // the hook handle argument is ignored for low-level hooks.
    unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) }
}

fn install() -> Option<HHOOK> {
    // SAFETY: `low_level_mouse_proc` is `extern "system"`, matches the
    // `HOOKPROC` signature, and stays valid for the process lifetime (it's a
    // plain `fn`, not a closure). A null module + thread id 0 is the
    // documented way to install a *global* low-level hook.
    unsafe {
        SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(low_level_mouse_proc),
            HINSTANCE::default(),
            0,
        )
    }
    .ok()
}

/// Runs the hook on the calling thread until [`stop`] is called (or the
/// process is torn down). The hook is always removed before returning.
pub fn run(shared: Arc<Shared>) -> Result<(), String> {
    let _ = START.get_or_init(Instant::now);
    let _ = SHARED.set(shared);
    boost_thread_priority();
    // SAFETY: `GetCurrentThreadId` has no preconditions.
    HOOK_THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::Release);

    let Some(mut hook) = install() else {
        HOOK_THREAD_ID.store(0, Ordering::Release);
        return Err("could not install the mouse hook (SetWindowsHookExW failed)".to_owned());
    };

    let mut msg = MSG::default();
    // SAFETY: `GetMessageW`/`TranslateMessage`/`DispatchMessageW` on a
    // stack-local `MSG` is the standard Win32 message-loop pattern; `hook`
    // is a live handle we own and unhook exactly once per replacement.
    unsafe {
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 <= 0 {
                break; // 0 = WM_QUIT, -1 = error
            }
            if msg.message == WM_REINSTALL {
                // FR-22: Windows may have dropped the hook (timeout, session
                // switch, resume). Install the new one *before* removing the
                // old so there is no unhooked gap.
                if let Some(fresh) = install() {
                    let _ = UnhookWindowsHookEx(hook);
                    hook = fresh;
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Always unhook, on both `WM_QUIT` and message-loop errors.
        let _ = UnhookWindowsHookEx(hook);
    }
    HOOK_THREAD_ID.store(0, Ordering::Release);
    Ok(())
}

fn post_to_hook_thread(msg: u32) {
    let tid = HOOK_THREAD_ID.load(Ordering::Acquire);
    if tid != 0 {
        // SAFETY: posting a message to a thread id we obtained ourselves; if
        // the thread has already exited this is a documented no-op error.
        let _ = unsafe { PostThreadMessageW(tid, msg, WPARAM(0), LPARAM(0)) };
    }
}

/// Ask the hook thread's message loop to exit (FR-22: hook is always
/// removed on shutdown).
pub fn stop() {
    post_to_hook_thread(WM_QUIT);
}

/// Ask the hook thread to re-install the hook (after unlock / resume /
/// remote-session changes). Rate-limited so event storms cannot thrash it.
pub fn reinstall() {
    let now_ms = (now_us() / 1_000).max(1);
    let last = LAST_REINSTALL_MS.load(Ordering::Relaxed);
    if last != 0 && now_ms.saturating_sub(last) < MIN_REINSTALL_GAP_MS {
        return;
    }
    LAST_REINSTALL_MS.store(now_ms, Ordering::Relaxed);
    post_to_hook_thread(WM_REINSTALL);
}
