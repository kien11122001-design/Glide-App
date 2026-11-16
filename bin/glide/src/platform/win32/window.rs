//! Cursor -> root window -> PID -> exe name / elevation, with a tiny cache so
//! the hook's fast path rarely pays for a real lookup (6.2, 6.3). Also the
//! foreground-fullscreen heuristic for FR-21.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetCursorPos, GetDesktopWindow, GetForegroundWindow,
    GetShellWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, WindowFromPoint,
    GA_ROOT, GWL_STYLE, WS_CAPTION,
};

/// exe lookups are relatively expensive (process open + a few syscalls); the
/// cursor usually sits over the same window for many consecutive wheel
/// events, so remember the last answer for a short TTL.
const CACHE_TTL: Duration = Duration::from_millis(500);

/// Windows where borderless "fullscreen" is a normal, non-game state: video
/// players, browsers (F11 / video fullscreen) and presenters keep smoothing.
const FULLSCREEN_EXEMPT: [&str; 14] = [
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
    "vlc.exe",
    "mpc-hc.exe",
    "mpc-hc64.exe",
    "mpv.exe",
    "potplayermini64.exe",
    "wmplayer.exe",
    "powerpnt.exe",
    "sumatrapdf.exe",
];

/// What the hook needs to know about the window under the cursor.
pub struct Target {
    /// Lowercase exe name (`""` if unknown).
    pub exe: String,
    /// The target is elevated (or protected) while Glide is not: Windows
    /// (UIPI) would drop our injected wheel events, so we must pass the real
    /// ones through instead of swallowing them ("dead scroll").
    pub uipi_blocked: bool,
}

impl Target {
    fn unknown() -> Self {
        Target {
            exe: String::new(),
            uipi_blocked: false,
        }
    }
}

pub struct WindowCache {
    hwnd: isize,
    target: Target,
    checked_at: Instant,
}

impl WindowCache {
    pub fn new() -> Self {
        WindowCache {
            hwnd: 0,
            target: Target::unknown(),
            checked_at: Instant::now() - CACHE_TTL - Duration::from_secs(1),
        }
    }

    pub fn target_under_cursor(&mut self) -> &Target {
        let Some(hwnd) = root_window_under_cursor() else {
            self.hwnd = 0;
            self.target = Target::unknown();
            return &self.target;
        };
        let now = Instant::now();
        let key = hwnd.0 as isize;
        if key == self.hwnd && now.duration_since(self.checked_at) < CACHE_TTL {
            return &self.target;
        }
        self.hwnd = key;
        self.checked_at = now;
        self.target = inspect(hwnd);
        &self.target
    }
}

impl Default for WindowCache {
    fn default() -> Self {
        Self::new()
    }
}

fn root_window_under_cursor() -> Option<HWND> {
    // SAFETY: `GetCursorPos`/`WindowFromPoint`/`GetAncestor` take no
    // invariants beyond a valid out-pointer, which `point` provides.
    unsafe {
        let mut point = Default::default();
        if GetCursorPos(&mut point).is_err() {
            return None;
        }
        let leaf = WindowFromPoint(point);
        if leaf.0.is_null() {
            return None;
        }
        let root = GetAncestor(leaf, GA_ROOT);
        Some(if root.0.is_null() { leaf } else { root })
    }
}

/// Is the token behind `process` elevated? `None` if it can't be read.
fn token_elevated(process: HANDLE) -> Option<bool> {
    // SAFETY: `token` is an out-handle we close before returning; the
    // `TOKEN_ELEVATION` buffer and its size match what
    // `GetTokenInformation(TokenElevation)` writes.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast::<c_void>()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        let _ = CloseHandle(token);
        result.ok()?;
        Some(elevation.TokenIsElevated != 0)
    }
}

fn self_elevated() -> bool {
    static SELF: OnceLock<bool> = OnceLock::new();
    *SELF.get_or_init(|| {
        // SAFETY: `GetCurrentProcess` returns a pseudo-handle (no cleanup).
        token_elevated(unsafe { GetCurrentProcess() }).unwrap_or(false)
    })
}

fn image_name(process: HANDLE) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf`/`len` describe a valid writable UTF-16 buffer that
    // outlives the call.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .is_ok();
    if !ok || len == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.rsplit(['\\', '/'])
        .next()
        .map(|s| s.to_ascii_lowercase())
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: `hwnd` is a window handle we just obtained; `pid` is a valid
    // out-pointer. A stale handle simply yields pid 0.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn inspect(hwnd: HWND) -> Target {
    let pid = window_pid(hwnd);
    if pid == 0 {
        return Target::unknown();
    }
    let own_elevated = self_elevated();
    // SAFETY: the process handles opened here are owned locally and closed
    // before return.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            // Protected / other-session process: we cannot even query it, so
            // injected input would not reach it either.
            return Target {
                exe: String::new(),
                uipi_blocked: !own_elevated,
            };
        };
        let exe = image_name(process).unwrap_or_default();
        let target_elevated = match token_elevated(process) {
            Some(e) => e,
            // Token unreadable with a limited handle: an elevated process
            // refuses a fuller handle, a normal one grants it.
            None => match OpenProcess(PROCESS_QUERY_INFORMATION, false, pid) {
                Ok(full) => {
                    let _ = CloseHandle(full);
                    false
                }
                Err(_) => true,
            },
        };
        let _ = CloseHandle(process);
        Target {
            exe,
            uipi_blocked: target_elevated && !own_elevated,
        }
    }
}

fn exe_of_window(hwnd: HWND) -> Option<String> {
    let pid = window_pid(hwnd);
    if pid == 0 {
        return None;
    }
    // SAFETY: the process handle is owned locally and closed before return.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let name = image_name(process);
        let _ = CloseHandle(process);
        name
    }
}

/// True when the foreground window looks like an exclusive/borderless
/// *game*: no title bar, covering its whole monitor, not the shell/desktop
/// and not a browser/video-player/presenter (whose fullscreen keeps scrolling).
pub fn foreground_is_fullscreen() -> bool {
    // SAFETY: all calls take plain handles/out-pointers we own; no lifetime
    // requirements beyond this function's scope.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() || hwnd == GetShellWindow() || hwnd == GetDesktopWindow() {
            return false;
        }

        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut class);
        let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
        if class == "Progman" || class == "WorkerW" {
            return false; // the desktop itself
        }

        // A title bar means a normal (even maximized) window.
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        if style & WS_CAPTION.0 == WS_CAPTION.0 {
            return false;
        }

        let mut rect = Default::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let m = info.rcMonitor;
        let covers = rect.left <= m.left
            && rect.top <= m.top
            && rect.right >= m.right
            && rect.bottom >= m.bottom;
        if !covers {
            return false;
        }
        match exe_of_window(hwnd) {
            Some(exe) => !FULLSCREEN_EXEMPT.contains(&exe.as_str()),
            None => true,
        }
    }
}
