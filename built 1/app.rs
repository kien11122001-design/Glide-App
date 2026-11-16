//! Application shell (Phase 5): wires CLI, single instance, config, the
//! animator + hook threads, the tray and the UI message loop together.
//! Entirely safe code; every Win32 call lives in `platform::win32`.

use crate::animator::{self, Backend};
use crate::config::{self, Config};
use crate::platform::win32::{autostart, hook, system, tray, WindowsBackend};
use crate::state::Shared;
use crate::{cli, trace::Tracing};
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

/// Animator thread body. The backend is created here because its timer
/// handle must stay on this thread.
fn run_animator(shared: &Shared, trace_path: Option<PathBuf>) {
    crate::platform::win32::timer::boost_thread_priority();
    let mut backend = WindowsBackend::new();
    match trace_path.and_then(|p| std::fs::File::create(p).ok()) {
        Some(file) => {
            let mut traced = Tracing::new(backend, BufWriter::new(file));
            animator::run(shared, &mut traced);
            traced.release(); // flushes the CSV
        }
        None => {
            animator::run(shared, &mut backend);
            backend.release();
        }
    }
}

pub fn run() -> i32 {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            system::console_print(&format!("glide: {e}\n\n{}", cli::HELP), true);
            return 2;
        }
    };
    if let Some(text) = cli::info_text(&args) {
        system::console_print(&text, false);
        return 0;
    }

    // FR-19: a second launch exits quietly; keep the guard alive until return.
    let _instance = match system::single_instance() {
        system::SingleInstance::AlreadyRunning => return 0,
        other => other,
    };

    // FR-14 / FR-15: first-run file, then load; a bad file means defaults + balloon.
    let config_path = args.config.clone().unwrap_or_else(config::default_path);
    if let Err(e) = config::ensure_default_file(&config_path) {
        eprintln!("glide: cannot create {}: {e}", config_path.display());
    }
    let (cfg, load_error) = match config::load(&config_path) {
        Ok(cfg) => (cfg, None),
        Err(e) => (Config::default(), Some(e.to_string())),
    };
    // `autostart = true` in the file registers the Run entry; turning it off
    // is done from the tray (the registry is the source of truth).
    if cfg.autostart && !autostart::is_enabled() {
        let _ = autostart::set_enabled(true);
    }

    let shared = Shared::new(cfg);
    if args.disable {
        shared.set_enabled(false);
    }

    let animator_thread = {
        let shared = Arc::clone(&shared);
        let trace_path = args.trace.clone();
        thread::Builder::new()
            .name("glide-animator".into())
            .spawn(move || run_animator(&shared, trace_path))
            .ok()
    };

    // The tray exists before the hook thread starts so a hook failure can
    // be shown as a balloon.
    let hwnd = match tray::create(Arc::clone(&shared), config_path, load_error) {
        Ok(hwnd) => hwnd,
        Err(e) => {
            eprintln!("glide: cannot create the tray window: {e}");
            shared.request_shutdown();
            if let Some(t) = animator_thread {
                let _ = t.join();
            }
            return 1;
        }
    };

    let hook_thread = {
        let shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("glide-hook".into())
            .spawn(move || {
                if let Err(e) = hook::run(shared) {
                    eprintln!("glide: {e}");
                    tray::notify("Glide", "Could not install the mouse hook; smoothing is inactive.", true);
                }
            })
            .ok()
    };

    system::run_message_loop();

    // Shutdown: stop all motion first (never leave the wheel "stuck"), then
    // remove the hook, the tray icon, and join the worker threads.
    shared.request_shutdown();
    hook::stop();
    tray::destroy(hwnd);
    if let Some(t) = hook_thread {
        let _ = t.join();
    }
    if let Some(t) = animator_thread {
        let _ = t.join();
    }
    0
}
