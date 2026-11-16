//! Small process-level Win32 helpers used by the (safe) app shell: console
//! output for a GUI-subsystem exe, the single-instance mutex, and the UI
//! thread's message loop.

use std::io::Write;
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Console::{
    AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, TranslateMessage, MSG,
};

/// Holds the named mutex for the life of the process (FR-19).
pub struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: the handle was created by `single_instance` and is closed
        // exactly once, here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub enum SingleInstance {
    /// We are the first instance; keep the guard alive (it is only held for
    /// its `Drop`, which closes the mutex, so the field is never read).
    First(#[allow(dead_code)] InstanceGuard),
    /// Another Glide already runs; the caller should exit quietly.
    AlreadyRunning,
    /// The mutex could not be created; run anyway rather than refuse to start.
    Unknown,
}

pub fn single_instance() -> SingleInstance {
    // SAFETY: `CreateMutexW` with a static NUL-terminated name; the
    // returned handle is owned by the guard. `GetLastError` is read
    // immediately after, before any other Win32 call.
    unsafe {
        let Ok(handle) = CreateMutexW(None, false, w!("Local\\Glide.SingleInstance")) else {
            return SingleInstance::Unknown;
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            SingleInstance::AlreadyRunning
        } else {
            SingleInstance::First(InstanceGuard(handle))
        }
    }
}

/// Print text from a GUI-subsystem exe: use redirected stdout if there is
/// one (`glide --print-default-config > file`), else attach to the parent
/// console (cmd/PowerShell) and write to it directly.
pub fn console_print(text: &str, to_stderr: bool) {
    // SAFETY: plain handle query / console attach; no pointers involved.
    let redirected = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }
        .map(|h| !h.0.is_null() && h.0 as isize != -1)
        .unwrap_or(false);
    if redirected {
        if to_stderr {
            let _ = std::io::stderr().write_all(text.as_bytes());
        } else {
            let _ = std::io::stdout().write_all(text.as_bytes());
        }
        return;
    }
    // SAFETY: attaching to the parent's console has no preconditions; it
    // fails harmlessly when there is none (double-click launch).
    let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok();
    if attached {
        if let Ok(mut con) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            let _ = con.write_all(text.as_bytes());
        }
    }
}

/// The UI thread's message loop; returns after `PostQuitMessage`.
pub fn run_message_loop() {
    let mut msg = MSG::default();
    // SAFETY: the standard Win32 message pump on a stack-local `MSG`.
    unsafe {
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 <= 0 {
                break; // 0 = WM_QUIT, -1 = error
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
