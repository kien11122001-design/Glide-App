//! Cursor -> root window -> PID -> exe name, with a tiny cache so the hook's
//! fast path rarely pays for a real lookup (6.2). Also a foreground
//! fullscreen heuristic for FR-21.

use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, MAX_PATH};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
    WindowFromPoint, GA_ROOT,
};

/// exe lookups are relatively expensive (process open + a syscall); the
/// cursor usually sits over the same window for many consecutive wheel
/// events, so remember the last answer for a short TTL.
const CACHE_TTL: Duration = Duration::from_millis(500);

pub struct WindowCache {
    hwnd: isize,
    exe: String,
    checked_at: Instant,
}

impl WindowCache {
    pub fn new() -> Self {
        WindowCache {
            hwnd: 0,
            exe: String::new(),
            checked_at: Instant::now() - CACHE_TTL - Duration::from_secs(1),
        }
    }

    /// Lowercase exe name of the window under the cursor (`""` if unknown).
    pub fn exe_under_cursor(&mut self) -> &str {
        // SAFETY: `GetCursorPos`/`WindowFromPoint` take no invariants beyond
        // a valid out-pointer, which `point` provides.
        let hwnd = unsafe {
            let mut point = Default::default();
            if GetCursorPos(&mut point).is_err() {
                return "";
            }
            let leaf = WindowFromPoint(point);
            if leaf.0 == 0 {
                return "";
            }
            let root = GetAncestor(leaf, GA_ROOT);
            if root.0 == 0 {
                leaf
            } else {
                root
            }
        };

        let now = Instant::now();
        if hwnd.0 == self.hwnd && now.duration_since(self.checked_at) < CACHE_TTL {
            return &self.exe;
        }
        self.hwnd = hwnd.0;
        self.checked_at = now;
        self.exe = exe_name_for_window(hwnd).unwrap_or_default();
        &self.exe
    }
}

impl Default for WindowCache {
    fn default() -> Self {
        Self::new()
    }
}

fn exe_name_for_window(hwnd: HWND) -> Option<String> {
    // SAFETY: `hwnd` came from `WindowFromPoint`/`GetAncestor` and is either
    // 0 (checked by the caller) or a live window handle; the process handle
    // and buffer we open/fill are owned locally and released before return.
    unsafe {
        let mut pid = 0u32;
        if GetWindowThreadProcessId(hwnd, Some(&mut pid)) == 0 || pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; MAX_PATH as usize];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(process);
        if !ok || len == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit(['\\', '/'])
            .next()
            .map(|s| s.to_ascii_lowercase())
    }
}

/// True when the foreground window looks like exclusive/borderless
/// fullscreen: no title bar and its rect exactly covers a monitor.
pub fn foreground_is_fullscreen() -> bool {
    // SAFETY: all calls take plain handles/out-pointers we own; no lifetime
    // requirements beyond this function's scope.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
            return false;
        }
        let mut rect = Default::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
        let cx = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_CXSCREEN,
        );
        let cy = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_CYSCREEN,
        );
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        // Primary-monitor heuristic (documented limitation: multi-monitor
        // fullscreen on a secondary display isn't caught by this check).
        rect.left <= 0 && rect.top <= 0 && w >= cx && h >= cy
    }
}
