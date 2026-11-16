#![allow(unsafe_code)]
//! All Win32 FFI lives here, and only here (crate root has
//! `#![forbid(unsafe_code)]`). Every `unsafe` block carries a `// SAFETY:`
//! comment explaining the invariant it relies on.
//!
//! NOTE: this module cannot be exercised by `cargo test` off Windows, so it
//! is unverified by CI running on a non-Windows host. The pure `engine`,
//! `config`, `state`, `animator` and `cli` modules carry the automated test
//! suite (Gate 2/Gate 4); this module is verified by the manual Win32
//! compatibility pass in Phase 6 (`cargo build --release --target
//! x86_64-pc-windows-msvc` + the app matrix in `IMPLEMENTATION_PLAN.md`).

pub mod autostart;
pub mod hook;
pub mod hotkey;
pub mod inject;
pub mod timer;
pub mod tray;
pub mod window;

pub use timer::WindowsBackend;

/// Magic tag stamped on every event Glide injects via `SendInput`, so the
/// hook can recognize (and never re-swallow) its own output even though
/// `LLMHF_INJECTED` alone doesn't distinguish Glide from other injectors.
pub const EXTRA_INFO_TAG: usize = 0x474C4944; // "GLID"
