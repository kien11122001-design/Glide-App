//! Glide - tiny, memory-safe smooth scrolling for Windows mouse wheels.
//!
//! Layout (see `CLAUDE.md` section 7): the physics `engine`, `config`,
//! `state`, `animator` and `cli` modules are safe and platform-independent
//! (unit-tested on any OS); everything that touches Win32 lives under
//! `platform::win32`, the only module allowed to use `unsafe`.
//!
//! NOTE on the lint level: the PRD asks for `#![forbid(unsafe_code)]` at the
//! root, but `forbid` cannot be re-allowed in a child module (rustc E0453), so
//! the root uses `deny` and only `platform::win32` opts back in with
//! `#![allow(unsafe_code)]`. `scripts/check-unsafe.sh` (run in CI) enforces
//! that the keyword appears nowhere else.
#![deny(unsafe_code)]
// Release builds are a GUI-subsystem exe (no console window). Debug builds keep
// the console so `cargo run` output is visible.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// Off Windows only the pure modules exist as a test target; keep that build
// warning-free without sprinkling `cfg` over every item.
#![cfg_attr(not(windows), allow(dead_code))]

mod animator;
#[cfg(windows)]
mod app;
mod cli;
mod config;
mod engine;
#[cfg(windows)]
mod platform;
mod state;
mod trace;

#[cfg(windows)]
fn main() {
    std::process::exit(app::run());
}

/// Non-Windows hosts: the informational flags still work (handy in CI), the
/// scroller itself does not.
#[cfg(not(windows))]
fn main() {
    let code = match cli::parse(std::env::args().skip(1)) {
        Ok(args) => match cli::info_text(&args) {
            Some(text) => {
                print!("{text}");
                0
            }
            None => {
                eprintln!("glide: smooth scrolling is only available on Windows");
                1
            }
        },
        Err(e) => {
            eprintln!("glide: {e}\n\n{}", cli::HELP);
            2
        }
    };
    std::process::exit(code);
}
