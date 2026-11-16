# CLAUDE.md — Glide: Smooth Scrolling for Windows Mouse Wheels

> Working name: **Glide** (crate: `glide`). This file is the Product Requirements Document (PRD) and the standing project instructions for Claude Code. Progress tracking lives in `IMPLEMENTATION_PLAN.md`.

---

## 1. Overview

**Glide** is a tiny, background Windows utility written in Rust that makes a physical mouse wheel scroll smoothly in every application, similar to how finger scrolling feels on a tablet. It intercepts discrete wheel "notches", swallows them, and replaces them with a stream of small, eased, high-resolution scroll events.

### Objective
Build a **small, efficient, memory-safe** smooth-scrolling app for the mouse with **user-customizable behavior** (scroll step size, animation time, acceleration, momentum, easing, per-app rules, etc.).

### Problem
Standard mouse wheels scroll in coarse jumps (typically 3 lines / ~100 px per notch). This feels jarring compared to touch or precision-touchpad scrolling, and it makes tracking content while reading harder. Existing solutions are either bloated, closed-source, Electron-based, unmaintained, or inconsistent across apps.

### Product Vision
Install once, forget it exists: near-zero CPU when idle, a few MB of RAM, a single-file executable under ~500 KB, and scrolling that feels natural everywhere.

---

## 2. Goals and Non-Goals

### Goals
- G1. Smooth, tablet-like wheel scrolling system-wide (vertical and horizontal).
- G2. Extremely small footprint: tiny binary, low idle memory, ~0% idle CPU.
- G3. Memory safety: all logic in safe Rust; `unsafe` confined to a small, audited FFI module.
- G4. Deep customizability via a human-editable config file, with live reload.
- G5. Non-intrusive: does not break games, elevated apps, touchpads, or other input tools.
- G6. Zero telemetry, no network access, no dependencies on a runtime.

### Non-Goals (v1)
- Not a general mouse remapper or macro tool.
- No GUI settings window (config file + tray menu only; GUI may come later).
- No touchpad smoothing (Precision Touchpads are already smooth; pass through).
- No support for macOS/Linux in v1 (architecture should keep platform code isolated so ports are possible).
- No installer requirement; portable exe is the primary distribution.

---

## 3. Target Users
- Power users and developers who read long documents/code and want smoother scrolling.
- Users migrating from touch/trackpad workflows who dislike stepped wheel scrolling.
- Users of high-refresh-rate displays (120/144/240 Hz) who notice 100 px jumps.

---

## 4. Research Findings (Summary)

| Topic | Finding |
|---|---|
| Interception | `SetWindowsHookExW(WH_MOUSE_LL)` sees all wheel events system-wide before target apps. Returning non-zero swallows the event. |
| Injection | `SendInput` with `MOUSEEVENTF_WHEEL` / `MOUSEEVENTF_HWHEEL` and `mouseData` deltas smaller than `WHEEL_DELTA` (120). Most modern apps (browsers, Office, Explorer, IDEs) honor fractional deltas; some legacy apps round or ignore them. |
| Loop prevention | Injected events carry `LLMHF_INJECTED` (and can carry a custom `dwExtraInfo` tag). The hook must ignore its own events. |
| Hook timeout | Windows silently unhooks a low-level hook if the callback exceeds the `LowLevelHooksTimeout` (~300 ms default). The callback must do minimal work and return immediately. |
| UIPI | A non-elevated process cannot inject input into elevated windows. Options: run as admin (via user choice / scheduled task) or accept limitation and pass through wheel events for elevated targets. |
| Touchpads | Precision Touchpads and some high-resolution wheels emit deltas that are not multiples of 120. Heuristic: pass through events where `abs(delta) % 120 != 0`. |
| Timing | Frame pacing via a high-resolution waitable timer (`CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`) or `DwmFlush()` for vsync alignment. Physics must use measured delta-time, not fixed steps. |
| Language | Rust: no runtime/GC, ~100–300 KB with size-optimized release profile, safe concurrency primitives. Official `windows` crate provides bindings. |
| Safety caveat | Win32 FFI is inherently `unsafe`; the safety benefit comes from minimizing and isolating that surface. |

---

## 5. Functional Requirements

### 5.1 Core Scrolling Engine
- FR-1. Intercept global vertical wheel events and swallow them when smoothing applies.
- FR-2. Intercept global horizontal wheel events (`WM_MOUSEHWHEEL`) and smooth them equivalently (configurable on/off).
- FR-3. Convert each wheel notch into a configurable **target distance** (step size).
- FR-4. Animate toward the target over a configurable **animation time**, emitting many small `SendInput` wheel events per second (frame-rate independent).
- FR-5. Support **accumulation**: rapid consecutive notches extend the target rather than restarting the animation.
- FR-6. Support **acceleration**: faster notch cadence multiplies step distance up to a cap.
- FR-7. Support optional **momentum/inertia**: after input stops, velocity decays via friction (tablet-style flick feel).
- FR-8. **Direction reversal** immediately cancels remaining motion and starts in the new direction.
- FR-9. Support selectable **easing curves**: `linear`, `ease_out_quad`, `ease_out_cubic`, `ease_out_expo`, `exponential_smoothing` (default).
- FR-10. Carry **sub-unit remainders** between frames so fractional deltas are never lost (no drift, no stalls).
- FR-11. Support **Shift+wheel → horizontal scroll** translation (configurable).
- FR-12. Support **Ctrl+wheel** (zoom) pass-through by default, configurable.
- FR-13. Pass through when a **modifier hotkey** is held (e.g., Alt held = raw scrolling; configurable).

### 5.2 Customization (all user-configurable)

| Setting | Description | Default | Range |
|---|---|---|---|
| `enabled` | Master toggle | `true` | bool |
| `step_size` | Distance per notch, in px-equivalent | `120` | 20–1000 |
| `animation_time_ms` | Time to settle after input | `300` | 50–2000 |
| `easing` | Curve name | `exponential_smoothing` | enum |
| `acceleration.enabled` | Speed up on rapid scrolling | `true` | bool |
| `acceleration.max_multiplier` | Cap | `4.0` | 1.0–10.0 |
| `acceleration.window_ms` | Interval within which notches count as “rapid” | `120` | 20–500 |
| `momentum.enabled` | Inertia after input stops | `false` | bool |
| `momentum.friction` | Decay per second factor | `0.90` | 0.5–0.999 |
| `momentum.min_velocity` | Stop threshold | `0.5` | >0 |
| `reverse_direction` | Invert wheel direction (natural scrolling) | `false` | bool |
| `horizontal.enabled` | Smooth horizontal wheel | `true` | bool |
| `shift_horizontal` | Shift+wheel = horizontal | `true` | bool |
| `bypass_modifier` | Key that disables smoothing while held | `"alt"` | none/alt/ctrl/shift/win |
| `frame_rate` | `"auto"` (display refresh) or fixed Hz | `"auto"` | 30–360 |
| `apps.exclude` | Process names to never smooth | `["games..."]` | list |
| `apps.include_only` | Optional allowlist mode | `[]` | list |
| `apps.overrides.<exe>` | Per-app override of any above setting | `{}` | table |
| `autostart` | Launch at login | `false` | bool |
| `hotkey.toggle` | Global enable/disable hotkey | `"ctrl+alt+s"` | string |

- FR-14. Config is a single **TOML** file at `%APPDATA%\Glide\config.toml`, created with commented defaults on first run.
- FR-15. **Live reload** on file change (no restart). Invalid config → keep last-good config, surface error via tray notification/log.
- FR-16. All values are **validated and clamped** to safe ranges.
- FR-17. Provide **built-in presets**: `default`, `snappy`, `silky`, `tablet` (momentum on), selectable via `preset = "..."`.

### 5.3 Application Behavior
- FR-18. Run in the background with a **tray icon** (toggle enable, open config, reload, autostart toggle, quit).
- FR-19. **Single instance** enforcement (named mutex).
- FR-20. **Per-app rules** by process executable name; detect target window under the cursor (`WindowFromPoint` → PID → exe name), with caching.
- FR-21. **Fullscreen/game detection**: auto-disable when a fullscreen exclusive/borderless game window is foreground (configurable).
- FR-22. **Graceful fail-safe**: if the hook is dropped by Windows, detect and reinstall it; on panic/exit always remove the hook.
- FR-23. Optional CLI flags: `--config <path>`, `--disable`, `--print-default-config`, `--version`.

---

## 6. Non-Functional Requirements

| Category | Requirement |
|---|---|
| Binary size | ≤ 500 KB release exe (target ~150–300 KB) |
| Idle memory | ≤ 5 MB working set while idle |
| Active memory | ≤ 10 MB while scrolling |
| Idle CPU | ~0% (animator thread parked; no polling) |
| Active CPU | < 2% of one core during continuous scrolling |
| Hook latency | Hook callback p99 < 100 µs; never blocks |
| Input-to-motion latency | First emitted scroll event ≤ 1 frame (~8–16 ms) after the notch |
| Smoothness | No visible stutter at 60/120/144/240 Hz; frame-time-based math |
| Startup | < 100 ms to active |
| Safety | `#![forbid(unsafe_code)]` everywhere except `platform/win32`; each `unsafe` block has a `// SAFETY:` comment |
| Privacy | No network, no telemetry, no logging of pointer position or app content |
| Compatibility | Windows 10 (1809+) and Windows 11, x64 and ARM64 |
| Reliability | Never leaves system in a stuck-scrolling state; all animation stops on disable/quit |

---

## 7. Architecture

### 7.1 Module Layout
```
glide/
├─ Cargo.toml
├─ CLAUDE.md
├─ IMPLEMENTATION_PLAN.md
├─ build.rs                 # embeds icon/manifest (optional)
├─ assets/                  # icon, manifest
└─ src/
   ├─ main.rs               # entry, single-instance, wiring
   ├─ config/               # schema, defaults, presets, validation, live reload
   ├─ engine/               # PURE safe Rust: physics, easing, accumulator (unit-testable)
   ├─ state.rs              # shared state (atomics / small mutex), command channel
   └─ platform/
      └─ win32/             # ONLY place with `unsafe`
         ├─ hook.rs         # WH_MOUSE_LL hook + message loop
         ├─ inject.rs       # SendInput wrappers
         ├─ timer.rs        # high-res waitable timer / DwmFlush
         ├─ window.rs       # target window/process detection + cache
         ├─ tray.rs         # Shell_NotifyIconW
         ├─ hotkey.rs       # RegisterHotKey
         └─ autostart.rs    # registry Run key
```

### 7.2 Threading Model
1. **Hook thread**: installs `WH_MOUSE_LL`, runs a message loop. Callback does only: ignore injected/own events → apply pass-through rules → push `WheelInput { delta, axis, timestamp }` into a lock-free/bounded channel → wake animator → return `1` (swallow) or `CallNextHookEx` (pass through).
2. **Animator thread**: parked on an event/condvar. When woken, ticks at display rate: reads pending inputs, advances the engine by measured `dt`, emits `SendInput` events, and re-parks when motion ends (velocity/remaining ≈ 0).
3. **UI/main thread**: tray icon, hotkey, config watcher messages, single-instance handling.

Shared state is minimal: an atomic `enabled` flag, an atomically swapped (`Arc`-swap style) immutable `Config` snapshot, and the input channel.

### 7.3 Engine Design (pure, testable)
- **Inputs**: notch events (signed delta, axis, timestamp), config snapshot, `dt`.
- **State per axis**: `target_remaining`, `velocity` (for momentum), `fractional_remainder`, `last_notch_time`, `accel_multiplier`.
- **Notch handling**: `distance = step_size × accel_multiplier × direction`; `target_remaining += distance`; reversal → zero remaining & velocity first.
- **Per-frame update (default, exponential smoothing)**:
  `alpha = 1 − exp(−dt / tau)` where `tau = animation_time / k`;
  `move = target_remaining × alpha`; `target_remaining −= move`; emit `move`.
  Frame-rate independent by construction.
- **Curve-based modes** (`ease_out_*`): each notch spawns/extends an animation segment with start time, duration, and distance; emitted amount per frame = eased position delta.
- **Momentum mode**: input adds impulse to `velocity`; each frame `velocity *= friction^(dt)`; stop below `min_velocity`.
- **Emit**: convert px-equivalent → wheel units (`units = px × 120 / px_per_notch`, `px_per_notch` calibration constant); add remainder; emit `trunc()` integer units; keep fractional part.
- **Termination**: when `|remaining| < epsilon` flush final remainder and park.

### 7.4 Pass-Through Rules (evaluated in hook, cheap-first)
1. Event is injected/tagged with Glide’s `dwExtraInfo` → pass through.
2. Smoothing disabled/paused → pass through.
3. `abs(delta) % 120 != 0` (precision device) → pass through.
4. Bypass modifier held → pass through.
5. Target process excluded / not in allowlist / fullscreen game → pass through (cached lookup).
6. Ctrl held (zoom) and `ctrl_passthrough = true` → pass through.
7. Otherwise → swallow and smooth.

---

## 8. Configuration Example (`config.toml`)
```toml
preset = "default"          # default | snappy | silky | tablet

enabled = true
step_size = 120             # px-equivalent per notch
animation_time_ms = 300
easing = "exponential_smoothing"
reverse_direction = false
frame_rate = "auto"

[acceleration]
enabled = true
max_multiplier = 4.0
window_ms = 120

[momentum]
enabled = false
friction = 0.90
min_velocity = 0.5

[horizontal]
enabled = true
shift_horizontal = true

[input]
bypass_modifier = "alt"
ctrl_passthrough = true

[apps]
exclude = ["game.exe"]
include_only = []

[apps.overrides."code.exe"]
animation_time_ms = 200

[general]
autostart = false
toggle_hotkey = "ctrl+alt+s"
```

---

## 9. Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Windows drops the hook (timeout) | Smoothing silently stops | Keep callback minimal; watchdog re-installs hook; heartbeat check |
| Elevated windows unaffected (UIPI) | Inconsistent behavior | Detect elevated targets and pass through; document “run as admin” option |
| Apps ignore sub-120 deltas | Choppy or no scroll in some apps | Accumulate fractions; per-app override for unit quantization (e.g., emit whole-120 steps at lower rate) |
| Event feedback loop | Runaway scrolling | Tag events via `dwExtraInfo` + `LLMHF_INJECTED` check |
| Games/anti-cheat flag hooks or input injection | Bans/false positives | Auto-disable in fullscreen; default exclusion list; document risk |
| Per-event process lookup too slow in hook | Hook latency/timeouts | HWND→exe cache with TTL; resolve asynchronously; fast-path default |
| Precision touchpad double-smoothing | Laggy touchpad | Delta-modulo heuristic pass-through |
| Wheel-unit ↔ px ambiguity | Step size not exact px | Treat `step_size` as “px-equivalent” with calibratable constant; document |
| Antivirus false positives (global hook + input injection) | Distribution friction | Sign binary; open-source; minimal imports; publish hashes |
| Stuck scrolling on crash | Bad UX | `panic = "abort"` + hook auto-removed on process exit; animator stops when hook thread gone |

---

## 10. Success Metrics
- Binary ≤ 500 KB; idle RSS ≤ 5 MB; idle CPU ≈ 0%.
- Smooth scrolling verified in: Chrome, Edge, Firefox, VS Code, Explorer, Word/Excel, Notepad, PDF readers, Terminal.
- Zero panics and zero `unsafe` outside `platform/win32` (enforced in CI).
- Config live-reload works with invalid-config recovery.
- 24-hour soak test with no memory growth and no hook loss.

---

## 11. Milestones
1. **M0** — Project scaffold, CI, size-optimized profile.
2. **M1** — Hook + injection MVP (fixed smoothing).
3. **M2** — Engine with config-driven step/time/easing, accel, reversal.
4. **M3** — Config file, presets, live reload.
5. **M4** — Tray, hotkey, single instance, autostart.
6. **M5** — Per-app rules, fullscreen detection, touchpad pass-through.
7. **M6** — Momentum mode, hardening, soak tests.
8. **M7** — Packaging, signing, docs, v1.0 release.

---

## 12. Project Instructions for Claude Code

### Build / Run / Test
```bash
cargo build --release          # optimized, size-tuned binary
cargo run                      # debug run (console visible)
cargo test                     # engine unit tests (platform-independent)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```
Target: `x86_64-pc-windows-msvc` (also `aarch64-pc-windows-msvc`). Engine tests must pass on any OS.

### Release profile (must be kept)
```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

### Coding Rules
- Use `#![forbid(unsafe_code)]` at the crate root; allow `unsafe` **only** inside `src/platform/win32/` via `#![allow(unsafe_code)]` on that module.
- Every `unsafe` block requires a `// SAFETY:` comment explaining the invariants.
- Keep the **engine pure**: no Win32 types, no I/O, no clocks (pass `dt`/timestamps in) so it is fully unit-testable.
- The hook callback must never allocate, lock for long, log, or block. Target < 100 µs.
- Dependencies: prefer `std`. Allowed: `windows` (minimal features), `serde` + `toml` (config), `notify`-style watching only if size-justified (else poll mtime). Justify every new crate in the PR/commit message and check size impact.
- No `unwrap()`/`expect()` in non-test code except for provably infallible cases with a comment.
- No network code. No telemetry.
- Follow `rustfmt`; zero clippy warnings.

### Definition of Done (per task)
- Code compiles with no warnings; clippy clean.
- Unit tests added for engine logic; manual test notes for Win32 behavior.
- Binary size and idle memory re-checked if dependencies or features changed.
- The corresponding checkbox in `IMPLEMENTATION_PLAN.md` is ticked.

### Workflow
- Work through `IMPLEMENTATION_PLAN.md` in order; check off `[x]` items as completed.
- Prefer small, reviewable commits per sub-item.
- When behavior is uncertain (app compatibility, timing), write a small experiment or test harness rather than guessing.
