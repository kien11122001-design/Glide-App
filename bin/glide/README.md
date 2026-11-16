# Glide

Tiny, memory-safe smooth scrolling for Windows mouse wheels. Glide swallows the coarse
wheel "notches" and replaces them with a stream of small, eased, high-resolution wheel
events, system-wide, like finger scrolling on a touchpad.

* Rust, no runtime, one small `.exe`, ~0 % CPU when idle, no network, no telemetry.
* All `unsafe` is confined to `src/platform/win32/` (CI-enforced).
* Configuration is a plain TOML file with live reload.

## Install / run
Portable: download `glide.exe`, run it. A tray icon appears (left/right-click for the menu:
Enabled, Preset, Open config, Reload config, Start with Windows, Quit).
Global hotkey **Ctrl+Alt+S** toggles smoothing. Hold **Alt** to scroll normally.

## Build
```
cargo build --release          # Windows: target/release/glide.exe
cargo test                     # engine/config/state/animator tests run on any OS
```

## Configuration
`%APPDATA%\Glide\config.toml` is created on first run. See [docs/CONFIG.md](docs/CONFIG.md).
`glide --print-default-config` prints the commented defaults.

## Documentation
[CONFIG](docs/CONFIG.md) · [COMPATIBILITY](docs/COMPATIBILITY.md) · [ARCHITECTURE](docs/ARCHITECTURE.md)

## Status
See `IMPLEMENTATION_PLAN.md` for verified vs. unverified items.
