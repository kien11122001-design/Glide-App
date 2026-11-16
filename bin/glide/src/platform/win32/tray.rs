//! Tray icon, its context menu, and the hidden window that hosts it (5.2).
//! Runs entirely on the UI thread; the message loop lives in `system.rs`.

use crate::config::{self, Watcher, PRESET_NAMES};
use crate::state::Shared;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR,
    NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, GetCursorPos,
    KillTimer, LoadIconW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, SetTimer, TrackPopupMenu, CW_USEDEFAULT, HICON, HMENU, IDI_APPLICATION,
    MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, SW_SHOWNORMAL, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_APP, WM_COMMAND, WM_DESTROY, WM_ENDSESSION,
    WM_HOTKEY, WM_LBUTTONUP, WM_POWERBROADCAST, WM_QUERYENDSESSION, WM_RBUTTONUP, WM_SETTINGCHANGE,
    WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

const WM_TRAYICON: u32 = WM_APP + 1;
/// Poll interval for the config-file watcher (FR-15: "within ~1 second").
const CONFIG_POLL_TIMER_ID: usize = 1;
const CONFIG_POLL_MS: u32 = 1000;
/// FR-21: cheap periodic poll so the hook's hot path only ever does an
/// atomic load, never a `GetForegroundWindow`/`GetWindowRect` round trip.
const FULLSCREEN_POLL_TIMER_ID: usize = 2;
const FULLSCREEN_POLL_MS: u32 = 250;

const ID_TOGGLE_ENABLED: usize = 1;
const ID_OPEN_CONFIG: usize = 2;
const ID_RELOAD: usize = 3;
const ID_AUTOSTART: usize = 4;
const ID_QUIT: usize = 5;
const ID_PRESET_BASE: usize = 100; // + index into PRESET_NAMES

/// Icon resource ids embedded by `build.rs`: a light glyph for dark
/// taskbars (1) and a dark glyph for light taskbars (2).
const ICON_FOR_DARK_TASKBAR: usize = 1;
const ICON_FOR_LIGHT_TASKBAR: usize = 2;

/// `WM_WTSSESSION_CHANGE` wParam values (WTS_*).
const WTS_CONSOLE_CONNECT: usize = 1;
const WTS_REMOTE_CONNECT: usize = 3;
const WTS_SESSION_LOGON: usize = 5;
const WTS_SESSION_LOCK: usize = 7;
const WTS_SESSION_UNLOCK: usize = 8;
/// `WM_POWERBROADCAST` wParam values (PBT_*).
const PBT_APMRESUMESUSPEND: usize = 7;
const PBT_APMRESUMEAUTOMATIC: usize = 18;

struct Ctx {
    shared: Arc<Shared>,
    config_path: PathBuf,
    watcher: Mutex<Watcher>,
}

static CTX: OnceLock<Ctx> = OnceLock::new();
/// The tray window, so `notify` can be called from any thread.
static HWND_RAW: AtomicIsize = AtomicIsize::new(0);

fn hwnd_from_raw(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

fn copy_wide(dst: &mut [u16], text: &str) {
    let mut n = 0;
    for (i, unit) in text
        .encode_utf16()
        .take(dst.len().saturating_sub(1))
        .enumerate()
    {
        dst[i] = unit;
        n = i + 1;
    }
    if let Some(end) = dst.get_mut(n) {
        *end = 0;
    }
}

/// Balloon notification (config errors, hook failures). Safe from any thread;
/// silently does nothing before the tray window exists.
pub fn notify(title: &str, text: &str, error: bool) {
    let raw = HWND_RAW.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd_from_raw(raw),
        uID: 1,
        uFlags: NIF_INFO,
        dwInfoFlags: if error { NIIF_ERROR } else { NIIF_WARNING },
        ..Default::default()
    };
    copy_wide(&mut data.szInfoTitle, title);
    copy_wide(&mut data.szInfo, text);
    // SAFETY: `data` is fully initialised and outlives the call.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

/// Register (or clear, for an empty string) the global toggle hotkey.
fn rebind_hotkey(hwnd: HWND, text: &str) -> bool {
    super::hotkey::unregister(hwnd);
    let text = text.trim();
    if text.is_empty() {
        return true;
    }
    match config::parse_hotkey(text) {
        Some(hk) => super::hotkey::register(hwnd, &hk),
        None => false,
    }
}

/// Re-parse and (on success) publish the config file. Invalid files keep
/// the last-good config (FR-15) and raise a tray balloon + stderr line.
fn reload_config(ctx: &Ctx) {
    match config::load(&ctx.config_path) {
        Ok(cfg) => {
            let old_hotkey = ctx.shared.snapshot().1.toggle_hotkey.clone();
            let new_hotkey = cfg.toggle_hotkey.clone();
            ctx.shared.store_config(cfg);
            if old_hotkey != new_hotkey {
                let hwnd = hwnd_from_raw(HWND_RAW.load(Ordering::Relaxed));
                if !rebind_hotkey(hwnd, &new_hotkey) {
                    notify(
                        "Glide",
                        &format!("Could not register hotkey `{new_hotkey}` (already in use?)"),
                        false,
                    );
                }
            }
        }
        Err(e) => {
            eprintln!("glide: config error, keeping last-good settings: {e}");
            notify(
                "Glide: config error",
                &format!("{e}\nKeeping the last good settings."),
                true,
            );
        }
    }
}

fn taskbar_is_light() -> bool {
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `data`/`size` describe a valid DWORD out-buffer that outlives
    // the call; the key/value names are static NUL-terminated literals.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast::<c_void>()),
            Some(&mut size),
        )
    };
    status.is_ok() && data != 0
}

fn load_tray_icon() -> HICON {
    let id = if taskbar_is_light() {
        ICON_FOR_LIGHT_TASKBAR
    } else {
        ICON_FOR_DARK_TASKBAR
    };
    // SAFETY: resource-id icons are loaded from our own module (shared,
    // owned by the system: no `DestroyIcon`); `id` is passed as a
    // MAKEINTRESOURCE value.
    unsafe {
        let module: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        LoadIconW(module, PCWSTR(id as *const u16))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default()
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

fn add_icon(hwnd: HWND) {
    let mut data = NOTIFYICONDATAW {
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAYICON,
        hIcon: load_tray_icon(),
        ..icon_data(hwnd)
    };
    // SAFETY: `w!` yields a static NUL-terminated buffer; `data` is fully
    // initialised and `hwnd` is a live window we own.
    unsafe {
        let tip_text = w!("Glide - smooth scrolling");
        let tip = tip_text.as_wide();
        let n = tip.len().min(data.szTip.len() - 1);
        data.szTip[..n].copy_from_slice(&tip[..n]);
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
    }
}

/// Theme switch (dark/light taskbar): swap the glyph in place.
fn refresh_icon(hwnd: HWND) {
    let data = NOTIFYICONDATAW {
        uFlags: NIF_ICON,
        hIcon: load_tray_icon(),
        ..icon_data(hwnd)
    };
    // SAFETY: `data` is fully initialised and outlives the call.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

fn taskbar_created_message() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    // SAFETY: `RegisterWindowMessageW` with a static, NUL-terminated name.
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

/// # Safety
/// Only ever invoked by Windows as the window procedure of our tray window,
/// with the message parameters it supplies.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAYICON => {
            let event = lparam.0 as u32;
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                show_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            handle_command((wparam.0 & 0xFFFF) as usize);
            LRESULT(0)
        }
        WM_HOTKEY => {
            if let Some(ctx) = CTX.get() {
                ctx.shared.toggle_enabled();
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if let Some(ctx) = CTX.get() {
                if wparam.0 == CONFIG_POLL_TIMER_ID {
                    let changed = ctx.watcher.lock().map(|mut w| w.changed()).unwrap_or(false);
                    if changed {
                        reload_config(ctx);
                    }
                } else if wparam.0 == FULLSCREEN_POLL_TIMER_ID {
                    ctx.shared
                        .set_fullscreen(super::window::foreground_is_fullscreen());
                }
            }
            LRESULT(0)
        }
        // 6.5: the OS may silently drop the hook across lock/unlock, remote
        // session switches and sleep; re-install it and stop any motion.
        WM_WTSSESSION_CHANGE => {
            match wparam.0 {
                WTS_SESSION_UNLOCK | WTS_SESSION_LOGON | WTS_CONSOLE_CONNECT
                | WTS_REMOTE_CONNECT => {
                    super::hook::reinstall();
                }
                WTS_SESSION_LOCK => {
                    if let Some(ctx) = CTX.get() {
                        ctx.shared.request_cancel();
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            if wparam.0 == PBT_APMRESUMEAUTOMATIC || wparam.0 == PBT_APMRESUMESUSPEND {
                super::hook::reinstall();
                if let Some(ctx) = CTX.get() {
                    ctx.shared.request_cancel();
                }
            }
            LRESULT(1) // TRUE: message handled
        }
        WM_SETTINGCHANGE => {
            refresh_icon(hwnd);
            // SAFETY: forwarding to the default handler is always valid.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // Allow the session to end; on WM_ENDSESSION(true) we actually stop.
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                if let Some(ctx) = CTX.get() {
                    ctx.shared.request_shutdown();
                }
                // SAFETY: standard shutdown of this thread's message loop.
                unsafe { PostQuitMessage(0) };
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: standard shutdown of this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        other => {
            if other == taskbar_created_message() {
                add_icon(hwnd); // Explorer restarted: the icon is gone.
                return LRESULT(0);
            }
            // SAFETY: forwarding unhandled messages is the documented
            // default `WndProc` behavior.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

fn handle_command(id: usize) {
    let Some(ctx) = CTX.get() else { return };
    match id {
        ID_TOGGLE_ENABLED => {
            ctx.shared.toggle_enabled();
        }
        ID_OPEN_CONFIG => {
            let _ = config::ensure_default_file(&ctx.config_path);
            open_in_editor(&ctx.config_path);
        }
        ID_RELOAD => reload_config(ctx),
        ID_AUTOSTART => {
            let want = !super::autostart::is_enabled();
            if let Err(e) = super::autostart::set_enabled(want) {
                notify("Glide", &format!("Could not change autostart: {e}"), true);
            }
        }
        ID_QUIT => {
            ctx.shared.request_shutdown();
            // SAFETY: called on the UI thread from its own window procedure;
            // ends the message loop in `system::run_message_loop`.
            unsafe { PostQuitMessage(0) };
        }
        id if (ID_PRESET_BASE..ID_PRESET_BASE + PRESET_NAMES.len()).contains(&id) => {
            let name = PRESET_NAMES[id - ID_PRESET_BASE];
            match std::fs::read_to_string(&ctx.config_path) {
                Ok(text) => {
                    let updated = config::set_preset_in_text(&text, name);
                    if let Err(e) = std::fs::write(&ctx.config_path, updated) {
                        notify("Glide", &format!("Could not save the preset: {e}"), true);
                    }
                }
                Err(e) => notify("Glide", &format!("Could not read the config: {e}"), true),
            }
        }
        _ => {}
    }
}

fn wide_nul(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn open_in_editor(path: &Path) {
    let file = wide_nul(path);
    // SAFETY: both buffers are NUL-terminated UTF-16 kept alive for the
    // duration of the calls; `ShellExecuteW` copies what it needs.
    unsafe {
        let result = ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(file.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
        // <= 32 is a ShellExecute failure (e.g. no `.toml` association):
        // fall back to Notepad with the quoted path as its argument.
        if result.0 as isize <= 32 {
            let mut quoted: Vec<u16> = vec![u16::from(b'"')];
            quoted.extend(path.as_os_str().encode_wide());
            quoted.extend([u16::from(b'"'), 0]);
            let _ = ShellExecuteW(
                None,
                w!("open"),
                w!("notepad.exe"),
                PCWSTR(quoted.as_ptr()),
                None,
                SW_SHOWNORMAL,
            );
        }
    }
}

fn show_menu(hwnd: HWND) {
    let Some(ctx) = CTX.get() else { return };
    // SAFETY: `menu` is a freshly created popup menu, destroyed below before
    // returning (which also destroys the preset submenu it owns); every
    // string passed is either a `w!()` literal or a NUL-terminated buffer
    // that lives until `AppendMenuW` returns.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let enabled = ctx.shared.enabled();
        let _ = AppendMenuW(
            menu,
            if enabled { MF_CHECKED } else { MF_UNCHECKED } | MF_STRING,
            ID_TOGGLE_ENABLED,
            w!("Enabled"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        if let Ok(presets) = CreatePopupMenu() {
            for (i, name) in PRESET_NAMES.iter().enumerate() {
                let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
                let _ = AppendMenuW(
                    presets,
                    MF_STRING,
                    ID_PRESET_BASE + i,
                    PCWSTR(wide.as_ptr()),
                );
            }
            // MF_POPUP: the "id" argument is the submenu handle.
            let _ = AppendMenuW(menu, MF_POPUP | MF_STRING, presets.0 as usize, w!("Preset"));
        }

        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_CONFIG, w!("Open config"));
        let _ = AppendMenuW(menu, MF_STRING, ID_RELOAD, w!("Reload config"));
        let autostart = if super::autostart::is_enabled() {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        } | MF_STRING;
        let _ = AppendMenuW(menu, autostart, ID_AUTOSTART, w!("Start with Windows"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, w!("Quit"));

        let mut pt = Default::default();
        let _ = GetCursorPos(&mut pt);
        // A hidden window can't be foreground on its own; this is the
        // documented workaround so the popup dismisses on an outside click.
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(
            menu,
            TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
    }
}

/// Create the hidden window that hosts the tray icon, registers the global
/// toggle hotkey and session notifications, and starts the poll timers. The
/// caller pumps its messages (`system::run_message_loop`) on this thread.
/// `initial_error` (a config that failed to load at startup) becomes a balloon.
pub fn create(
    shared: Arc<Shared>,
    config_path: PathBuf,
    initial_error: Option<String>,
) -> windows::core::Result<HWND> {
    let watcher = Watcher::new(&config_path);
    let hotkey_text = shared.snapshot().1.toggle_hotkey.clone();
    let _ = CTX.set(Ctx {
        shared,
        config_path,
        watcher: Mutex::new(watcher),
    });
    // SAFETY: registers a window class and creates a window with a valid
    // `wndproc`; all strings are `w!()` static literals.
    let hwnd = unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = w!("GlideTrayWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("Glide"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            HMENU::default(),
            instance,
            None,
        )?;
        HWND_RAW.store(hwnd.0 as isize, Ordering::Relaxed);
        add_icon(hwnd);
        SetTimer(hwnd, CONFIG_POLL_TIMER_ID, CONFIG_POLL_MS, None);
        SetTimer(hwnd, FULLSCREEN_POLL_TIMER_ID, FULLSCREEN_POLL_MS, None);
        // Best-effort: without it we merely miss lock/unlock re-installs.
        let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
        hwnd
    };
    if !rebind_hotkey(hwnd, &hotkey_text) {
        eprintln!("glide: could not register hotkey `{hotkey_text}` (already in use?)");
        notify(
            "Glide",
            &format!("Could not register hotkey `{hotkey_text}` (already in use?)"),
            false,
        );
    }
    if let Some(e) = initial_error {
        eprintln!("glide: config error, using defaults: {e}");
        notify(
            "Glide: config error",
            &format!("{e}\nUsing default settings."),
            true,
        );
    }
    Ok(hwnd)
}

pub fn destroy(hwnd: HWND) {
    super::hotkey::unregister(hwnd);
    HWND_RAW.store(0, Ordering::Relaxed);
    // SAFETY: removes the icon/timers/notification we added in `create`;
    // harmless if any was already gone (Explorer restart, etc).
    unsafe {
        let _ = WTSUnRegisterSessionNotification(hwnd);
        let _ = KillTimer(hwnd, CONFIG_POLL_TIMER_ID);
        let _ = KillTimer(hwnd, FULLSCREEN_POLL_TIMER_ID);
        let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
    }
}
