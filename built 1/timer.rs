//! [`crate::animator::Backend`] for real Windows hardware: a high-resolution
//! waitable timer for frame pacing, `QueryPerformanceCounter` for the clock,
//! and the refresh rate of the monitor under the cursor for
//! `frame_rate = "auto"` (cached, so it is not re-queried every frame).

use super::inject;
use crate::animator::Backend;
use crate::engine::Output;
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, POINT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromPoint, DEVMODEW, ENUM_CURRENT_SETTINGS,
    MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CreateWaitableTimerExW, GetCurrentThread, SetThreadPriority, SetWaitableTimer,
    WaitForSingleObject, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, THREAD_PRIORITY_ABOVE_NORMAL,
    TIMER_ALL_ACCESS,
};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

const DEFAULT_HZ: u32 = 60;
/// The cursor may move to another monitor; re-check the refresh rate this
/// often while animating (cheap: a few syscalls, twice a second).
const HZ_TTL: Duration = Duration::from_millis(500);

/// Raise the calling thread slightly above normal so frame pacing and the
/// hook callback stay responsive under load (best-effort).
pub fn boost_thread_priority() {
    // SAFETY: operates on the current thread's pseudo-handle.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
}

fn query_refresh_hz() -> Option<u32> {
    // SAFETY: every out-structure is a correctly sized, zero-initialised
    // local; `szDevice` is a NUL-terminated buffer that `GetMonitorInfoW`
    // fills and that outlives the `EnumDisplaySettingsW` call.
    unsafe {
        let mut point = POINT::default();
        GetCursorPos(&mut point).ok()?;
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>()).as_bool() {
            return None;
        }
        let mut mode = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        if !EnumDisplaySettingsW(PCWSTR(info.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode).as_bool() {
            return None;
        }
        // 0 / 1 mean "hardware default" in `dmDisplayFrequency`.
        (mode.dmDisplayFrequency >= 30).then_some(mode.dmDisplayFrequency)
    }
}

pub struct WindowsBackend {
    qpc_freq: i64,
    timer: Option<HANDLE>,
    /// `timeBeginPeriod(1)` is process-wide and must be paired 1:1 with
    /// `timeEndPeriod(1)`; this tracks whether we currently hold it.
    fine_timer_period: bool,
    hz_cache: Option<(u32, Instant)>,
}

impl WindowsBackend {
    pub fn new() -> Self {
        let mut freq = 0i64;
        // SAFETY: out-pointer to a stack local; always succeeds on Windows
        // XP+ per the documented contract.
        unsafe {
            let _ = QueryPerformanceFrequency(&mut freq);
        }
        WindowsBackend {
            qpc_freq: freq.max(1),
            timer: None,
            fine_timer_period: false,
            hz_cache: None,
        }
    }

    fn ensure_timer(&mut self) {
        if self.timer.is_some() || self.fine_timer_period {
            return; // already set up (or already on the sleep fallback)
        }
        // SAFETY: standard `CreateWaitableTimerExW` call; the returned
        // handle (if any) is owned by `self` and closed in `release`.
        let handle = unsafe {
            CreateWaitableTimerExW(None, None, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0)
        };
        if let Ok(h) = handle {
            self.timer = Some(h);
        } else {
            // Fallback path (Windows older than 10 1803): 1 ms scheduler
            // granularity makes `std::thread::sleep` usable.
            // SAFETY: `timeBeginPeriod` has no preconditions; paired with
            // `timeEndPeriod` in `release`.
            unsafe {
                let _ = timeBeginPeriod(1);
            }
            self.fine_timer_period = true;
        }
    }
}

impl Default for WindowsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for WindowsBackend {
    fn now_us(&mut self) -> u64 {
        let mut ticks = 0i64;
        // SAFETY: out-pointer to a stack local.
        unsafe {
            let _ = QueryPerformanceCounter(&mut ticks);
        }
        // i128 avoids overflow for large uptimes at microsecond precision.
        ((i128::from(ticks) * 1_000_000) / i128::from(self.qpc_freq)).max(0) as u64
    }

    fn refresh_hz(&mut self) -> u32 {
        if let Some((hz, at)) = self.hz_cache {
            if at.elapsed() < HZ_TTL {
                return hz;
            }
        }
        let hz = query_refresh_hz().unwrap_or(DEFAULT_HZ);
        self.hz_cache = Some((hz, Instant::now()));
        hz
    }

    fn wait_frame(&mut self, hz: u32) {
        self.ensure_timer();
        let period_100ns = -((1_000_000_000 / 100) / i64::from(hz.max(1)));
        if let Some(timer) = self.timer {
            // SAFETY: `timer` is a live waitable-timer handle owned by
            // `self`; `period_100ns` is a valid relative (negative) offset.
            unsafe {
                if SetWaitableTimer(timer, &period_100ns, 0, None, None, false).is_ok() {
                    WaitForSingleObject(timer, u32::MAX);
                    return;
                }
            }
        }
        // Fallback: plain sleep (coarser, but never blocks forever).
        std::thread::sleep(Duration::from_micros(1_000_000 / u64::from(hz.max(1))));
    }

    fn send(&mut self, out: Output) {
        inject::send_wheel(out);
    }

    fn release(&mut self) {
        self.hz_cache = None;
        if let Some(timer) = self.timer.take() {
            // SAFETY: `timer` was created by us in `ensure_timer` and is
            // not used anywhere else; closing it once is required by NFR
            // "nothing held when idle".
            unsafe {
                let _ = CloseHandle(timer);
            }
        }
        if self.fine_timer_period {
            // SAFETY: pairs the `timeBeginPeriod(1)` above exactly once.
            unsafe {
                let _ = timeEndPeriod(1);
            }
            self.fine_timer_period = false;
        }
    }
}
