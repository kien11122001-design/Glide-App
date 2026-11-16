# Changelog

## [Unreleased]
### Added
- Pure smoothing engine (5 easing curves, acceleration, momentum, reversal, sub-unit remainder carry).
- TOML config with presets, per-app overrides, validation and live reload.
- Win32 layer: `WH_MOUSE_LL` hook, `SendInput` injection, high-resolution frame timer,
  tray icon (dark/light aware), global toggle hotkey, autostart, single instance,
  elevated-target and fullscreen-game pass-through, hook re-install after unlock/resume.
- `--trace <csv>` developer aid, `--print-default-config`, `--config`, `--disable`.
### Fixed
- Config swap mid-motion could strand motion in the unused accumulation model (never settled).
- `momentum.min_velocity` was parsed but ignored by the engine.
- Hook stamped every wheel event with t = 0 (acceleration cadence never worked).
- Shift+wheel produced horizontal scrolling in the wrong direction.
