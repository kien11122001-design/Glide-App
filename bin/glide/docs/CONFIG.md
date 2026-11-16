# Configuration reference

File: `%APPDATA%\Glide\config.toml` (override with `--config <path>`). Saved changes apply
within ~1 s. An invalid file keeps the last good settings and shows a tray notification.
Out-of-range numbers are clamped; unknown keys are reported as errors (typo protection).
Explicit values always beat `preset`.

| Key | Default | Range / values |
|---|---|---|
| `preset` | `"default"` | `default`, `snappy`, `silky`, `tablet` |
| `enabled` | `true` | bool |
| `step_size` | `120` | 20–1000 (px-equivalent per notch) |
| `animation_time_ms` | `300` | 50–2000 |
| `easing` | `"exponential_smoothing"` | `linear`, `ease_out_quad`, `ease_out_cubic`, `ease_out_expo`, `exponential_smoothing` |
| `reverse_direction` | `false` | bool |
| `frame_rate` | `"auto"` | `"auto"` or 30–360 |
| `[acceleration] enabled / max_multiplier / window_ms` | `true / 4.0 / 120` | bool / 1–10 / 20–500 |
| `[momentum] enabled / friction / min_velocity` | `false / 0.90 / 0.5` | bool / 0.5–0.999 (kept per 1/60 s) / px·s⁻¹ |
| `[horizontal] enabled / shift_horizontal` | `true / true` | bool |
| `[input] bypass_modifier` | `"alt"` | `none`, `alt`, `ctrl`, `shift`, `win` |
| `[input] ctrl_passthrough` | `true` | Ctrl+wheel (zoom) untouched |
| `[apps] exclude / include_only` | game list / `[]` | lowercase exe names |
| `[apps] auto_disable_in_fullscreen` | `true` | bool |
| `[apps.overrides."x.exe"]` | – | any per-app setting above |
| `[general] autostart` | `false` | `true` registers the Run key at startup; turn it off from the tray |
| `[general] toggle_hotkey` | `"ctrl+alt+s"` | modifiers + one key, `""` disables |

Notes: `friction` close to 0.999 glides for tens of seconds (by design). `min_velocity` is the
stop threshold; the final flush is capped so it never becomes a visible jump.
