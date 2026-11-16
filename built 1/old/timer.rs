//! [`crate::animator::Backend`] for real Windows hardware: a high-resolution
//! waitable timer for frame pacing, `QueryPerformanceCounter` for the clock,
//! and `EnumDisplaySettingsW` for `frame_rate = "auto"`.

use super::inject;
use crate::animator::Backend;
use crate::engine::Output;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS,
};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CreateWaitableTimerExW, SetWaitableTimer, WaitForSingleObject, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
    TIMER_ALL_ACCESS,
};

const DEFAULT_HZ: u32 = 60;

pub struct WindowsBackend {
    qpc_freq: i64,
    timer: Option<HANDLE>,
    /// `timeBeginPeriod(1)` is process-wide and must be paired 1:1 with
    /// `timeEndPeriod(1)`; this tracks whether we currently hold it.
    fine_timer_period: bool,
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
        }
    }

    fn ensure_timer(&mut self) {
        if self.timer.is_some() {
            return;
        }
        // SAFETY: standard `CreateWaitableTimerExW` call; the returned
        // handle (if any) is owned by `self` and closed in `release`.
        let handle = unsafe {
            CreateWaitableTimerExW(None, None, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION.0, TIMER_ALL_ACCESS.0)
        };
        if let Ok(h) = handle {
            self.timer = Some(h);
        } else {
            // Fallback path (older Windows without the high-res flag):
            // 1 ms scheduler granularity makes `std::thread::sleep` usable.
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
        ((ticks as i128 * 1_000_000) / self.qpc_freq as i128).max(0) as u64
    }

    fn refresh_hz(&mut self) -> u32 {
        let mut mode = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        // SAFETY: `mode` is a correctly-sized, zero-initialized `DEVMODEW`;
        // `EnumDisplaySettingsW` fills it in place for the primary device.
        let ok = unsafe { EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut mode).as_bool() };
        if ok && mode.dmDisplayFrequency >= 30 {
            mode.dmDisplayFrequency
        } else {
            DEFAULT_HZ
        }
    }

    fn wait_frame(&mut self, hz: u32) {
        self.ensure_timer();
        let period_100ns = -((1_000_000_000 / 100) / i64::from(hz.max(1)));
        if let Some(timer) = self.timer {
            // SAFETY: `timer` is a live waitable-timer handle owned by
            // `self`; `due_time` is a valid relative (negative) offset.
            unsafe {
                if SetWaitableTimer(timer, &period_100ns, 0, None, None, false).is_ok() {
                    WaitForSingleObject(timer, u32::MAX);
                    return;
                }
            }
        }
        // Fallback: plain sleep (coarser, but never blocks forever).
        std::thread::sleep(std::time::Duration::from_micros(1_000_000 / u64::from(hz.max(1))));
    }

    fn send(&mut self, out: Output) {
        inject::send_wheel(out);
    }

    fn release(&mut self) {
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
