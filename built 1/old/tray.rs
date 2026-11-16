//! Tray icon, its context menu, and the hidden window that hosts it (5.2).
//! Runs entirely on the UI thread inside [`super::hotkey::run_message_loop`].

use crate::config::{self, Watcher, PRESET_NAMES};
use crate::state::Shared;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    GetCursorPos, KillTimer, LoadIconW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, SetTimer, TrackPopupMenu, CW_USEDEFAULT, HMENU, IDI_APPLICATION, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, MF_UNCHECKED, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON,
    WINDOW_EX_STYLE, WM_APP, WM_COMMAND, WM_DESTROY, WM_ENDSESSION, WM_HOTKEY, WM_LBUTTONUP,
    WM_QUERYENDSESSION, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

const WM_TRAYICON: u32 = WM_APP + 1;
/// Poll interval for the config-file watcher (FR-15: live reload "within
/// ~1 second").
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

struct Ctx {
    shared: Arc<Shared>,
    config_path: PathBuf,
    watcher: Mutex<Watcher>,
}

static CTX: OnceLock<Ctx> = OnceLock::new();
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn quit_requested() -> bool {
    QUIT_REQUESTED.load(Ordering::Relaxed)
}

/// Re-parse and (on success) publish the config file. Invalid files keep
/// the last-good config (FR-15); logged to stderr since Glide has no
/// window to show error UI in (a tray balloon is a documented follow-up).
fn reload_config(ctx: &Ctx) {
    match config::load(&ctx.config_path) {
        Ok(cfg) => ctx.shared.store_config(cfg),
        Err(e) => eprintln!("glide: config error, keeping last-good settings: {e}"),
    }
}

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
                    ctx.shared.set_fullscreen(super::window::foreground_is_fullscreen());
                }
            }
            LRESULT(0)
        }
        // Allow the session to end; on WM_ENDSESSION(true) we actually stop.
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                if let Some(ctx) = CTX.get() {
                    ctx.shared.request_shutdown();
                }
                QUIT_REQUESTED.store(true, Ordering::Relaxed);
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
                add_icon(hwnd);
                return LRESULT(0);
            }
            // SAFETY: forwarding unhandled messages is the documented
            // default `WndProc` behavior.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

fn taskbar_created_message() -> u32 {
    // SAFETY: `RegisterWindowMessageW` with a static, NUL-terminated name.
    unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }
}

fn handle_command(id: usize) {
    let Some(ctx) = CTX.get() else { return };
    match id {
        ID_TOGGLE_ENABLED => {
            ctx.shared.toggle_enabled();
        }
        ID_OPEN_CONFIG => {
            let _ = config::ensure_default_file(&ctx.config_path);
            open_in_default_editor(&ctx.config_path);
        }
        ID_RELOAD => reload_config(ctx),
        ID_AUTOSTART => {
            let want = !super::autostart::is_enabled();
            let _ = super::autostart::set_enabled(want);
        }
        ID_QUIT => {
            QUIT_REQUESTED.store(true, Ordering::Relaxed);
            ctx.shared.request_shutdown();
        }
        id if (ID_PRESET_BASE..ID_PRESET_BASE + PRESET_NAMES.len()).contains(&id) => {
            let name = PRESET_NAMES[id - ID_PRESET_BASE];
            if let Ok(text) = std::fs::read_to_string(&ctx.config_path) {
                let updated = config::set_preset_in_text(&text, name);
                let _ = std::fs::write(&ctx.config_path, updated);
            }
        }
        _ => {}
    }
}

fn open_in_default_editor(path: &std::path::Path) {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer kept alive for the
    // duration of the call; `ShellExecuteW` copies what it needs.
    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            None,
            None,
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

use std::os::windows::ffi::OsStrExt;

fn show_menu(hwnd: HWND) {
    let Some(ctx) = CTX.get() else { return };
    // SAFETY: `menu` is a freshly created popup menu, destroyed below before
    // returning; every string passed is a `w!()` static NUL-terminated literal.
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

        let Ok(presets) = CreatePopupMenu() else { return };
        for (i, name) in PRESET_NAMES.iter().enumerate() {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let _ = AppendMenuW(presets, MF_STRING, ID_PRESET_BASE + i, PCWSTR(wide.as_ptr()));
        }
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            presets.0 as usize,
            w!("Preset"),
        );

        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_CONFIG, w!("Open config"));
        let _ = AppendMenuW(menu, MF_STRING, ID_RELOAD, w!("Reload config"));
        let autostart_flags = if super::autostart::is_enabled() { MF_CHECKED } else { MF_UNCHECKED } | MF_STRING;
        let _ = AppendMenuW(menu, autostart_flags, ID_AUTOSTART, w!("Start with Windows"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, w!("Quit"));

        let mut pt = Default::default();
        let _ = GetCursorPos(&mut pt);
        // A hidden window can't be foreground on its own; this dance is the
        // documented workaround so the popup dismisses on outside click.
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
        let _ = DestroyMenu(presets);
        let _ = DestroyMenu(menu);
    }
}

fn add_icon(hwnd: HWND) {
    // SAFETY: `NOTIFYICONDATAW` is fully initialized below; `hwnd` is a
    // live window we own.
    unsafe {
        let icon = LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAYICON,
            hIcon: icon,
            ..Default::default()
        };
        let tip = w!("Glide - smooth scrolling").as_wide();
        let n = tip.len().min(data.szTip.len() - 1);
        data.szTip[..n].copy_from_slice(&tip[..n]);
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
        let _ = DestroyIcon(icon);
    }
}

/// Create the hidden window that hosts the tray icon, registers the global
/// toggle hotkey, and starts the config-poll timer. The returned `HWND`
/// must have its messages pumped on the calling thread (see `main.rs`).
pub fn create(shared: Arc<Shared>, config_path: PathBuf) -> windows::core::Result<HWND> {
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
        let instance = GetModuleHandleW(None)?;
        let class_name = w!("GlideTrayWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
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
        add_icon(hwnd);
        SetTimer(hwnd, CONFIG_POLL_TIMER_ID, CONFIG_POLL_MS, None);
        SetTimer(hwnd, FULLSCREEN_POLL_TIMER_ID, FULLSCREEN_POLL_MS, None);
        hwnd
    };
    if !hotkey_text.trim().is_empty() {
        if let Some(hk) = config::parse_hotkey(&hotkey_text) {
            if !super::hotkey::register(hwnd, &hk) {
                eprintln!("glide: could not register hotkey `{hotkey_text}` (already in use?)");
            }
        }
    }
    Ok(hwnd)
}

pub fn destroy(hwnd: HWND) {
    super::hotkey::unregister(hwnd);
    // SAFETY: removes the icon/timer we added in `create`/`add_icon`;
    // harmless if either was already gone (Explorer restart, etc).
    unsafe {
        let _ = KillTimer(hwnd, CONFIG_POLL_TIMER_ID);
        let _ = KillTimer(hwnd, FULLSCREEN_POLL_TIMER_ID);
        let data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}
