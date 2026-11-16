# IMPLEMENTATION_PLAN.md — Glide

Companion to `CLAUDE.md` (PRD). Tick `[x]` as items are completed. Work top to bottom; each phase ends with a **Gate** that must pass before moving on.

Legend: `FR-n` = functional requirement in the PRD. `NFR` = non-functional requirement.

Status marks — `[x]` done and verified · `[~]` code written but **not yet compiled/run on Windows** (needs `cargo build`/manual test) · `[-]` partly done · `[ ]` not started.
"Verified" for `[x]`: the Rust unit-test suite passed (53 tests) in an earlier session on Linux; the later engine changes (model migration on config swap, `min_velocity`, fuzz tests) were verified against a line-by-line Python port of `engine/axis.rs` because the latest environment had no Rust toolchain or network. Deviation: root uses `#![deny(unsafe_code)]` (not `forbid`, which cannot be re-allowed in a child module, E0453).
Not ticked by design: everything that needs a real Windows machine (Gates, 1.5, 3.3, 6.6, 7.3–7.5, 8.1, 8.3 release).

---

## Phase 0 — Project Scaffold & Tooling (M0)

- [x] **0.1 Repository setup**
  - [x] `cargo new glide` (binary crate), set edition 2021/2024
  - [x] Add `.gitignore`, `LICENSE`, `README.md` stub
  - [x] Place `CLAUDE.md` and `IMPLEMENTATION_PLAN.md` at repo root
- [x] **0.2 Cargo configuration**
  - [x] Add size-optimized `[profile.release]` (`opt-level="z"`, `lto`, `codegen-units=1`, `panic="abort"`, `strip`)
  - [x] Add `windows` crate with minimal features only (`Win32_UI_WindowsAndMessaging`, `Win32_UI_Input_KeyboardAndMouse`, `Win32_Foundation`, `Win32_System_Threading`)
  - [x] Set `#![forbid(unsafe_code)]` at crate root; `#![allow(unsafe_code)]` only in `platform/win32`
  - [x] Set `windows_subsystem = "windows"` for release (no console window)
- [x] **0.3 Module skeleton**
  - [x] Create `config/`, `engine/`, `platform/win32/`, `state.rs` as empty modules with docs
  - [x] Define a `Platform` trait boundary so engine never imports Win32 types
- [~] **0.4 CI & quality gates**
  - [~] GitHub Actions: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `cargo build --release`
  - [x] CI step: grep-fail if `unsafe` appears outside `src/platform/win32/`
  - [~] CI step: report release binary size and fail above 500 KB
  - [~] Add `cargo-deny`/`cargo-audit` check for dependencies
- [~] **0.5 Embedded resources**
  - [~] App manifest (DPI awareness, `asInvoker`, Windows 10/11 compat)
  - [~] Tray icon `.ico` and `build.rs` (via `winres`/`embed-resource` only if size-justified)

**Gate 0:** empty app builds in CI, size report visible, unsafe-guard check passes.

---

## Phase 1 — Hook + Injection MVP (M1)

- [~] **1.1 Low-level mouse hook (`platform/win32/hook.rs`)**
  - [~] Install `WH_MOUSE_LL` via `SetWindowsHookExW`
  - [~] Run dedicated hook thread with `GetMessageW` loop
  - [~] Parse `MSLLHOOKSTRUCT`; identify `WM_MOUSEWHEEL` and `WM_MOUSEHWHEEL`
  - [~] Extract signed wheel delta from `mouseData` high word
  - [~] Ignore events with `LLMHF_INJECTED` flag
  - [~] Ignore events tagged with Glide's `dwExtraInfo` magic value
  - [~] Return `1` to swallow, `CallNextHookEx` to pass through
  - [~] Guarantee `UnhookWindowsHookEx` on shutdown
  - [x] `// SAFETY:` comments on every unsafe block
- [~] **1.2 Input injection (`platform/win32/inject.rs`)**
  - [~] Safe wrapper `send_wheel(axis, units: i32)` around `SendInput`
  - [~] Support `MOUSEEVENTF_WHEEL` (vertical) and `MOUSEEVENTF_HWHEEL` (horizontal)
  - [~] Tag injected events with Glide `dwExtraInfo`
  - [~] Batch multiple events in a single `SendInput` call when possible
- [x] **1.3 Thread plumbing (`state.rs`)**
  - [x] Bounded lock-free (or minimal-lock) channel: hook → animator
  - [x] Wake mechanism (event/condvar) so animator parks when idle
  - [x] Atomic `enabled` flag
- [x] **1.4 Naive animator**
  - [x] Hard-coded step size and fixed duration; simple exponential decay
  - [x] Emit sub-120 deltas with remainder carry
- [ ] **1.5 Manual verification**
  - [ ] Confirm no feedback loop (no runaway scrolling)
  - [ ] Test in Notepad, Chrome, Edge, Explorer, VS Code
  - [ ] Confirm exit removes hook and scrolling returns to normal

**Gate 1:** wheel scrolling is visibly smoothed in at least 4 apps; no runaway events; clean exit.

---

## Phase 2 — Engine (Pure Rust, Fully Tested) (M2)

- [x] **2.1 Engine data model (`engine/`)**
  - [x] `Axis` enum (Vertical, Horizontal)
  - [x] `WheelInput { delta: i32, axis, t: Instant-like u64 }` (time passed in, not read)
  - [x] `EngineConfig` snapshot struct (independent of file format)
  - [x] `Engine::on_input(input)` and `Engine::tick(dt) -> Output { v_units, h_units }`
  - [x] `Engine::is_idle()` for animator parking
- [x] **2.2 Step size & accumulation (FR-3, FR-5)**
  - [x] Convert notches → px-equivalent distance via `step_size`
  - [x] Accumulate rapid notches into `target_remaining`
  - [x] Unit tests: N notches → N × step_size total distance
- [x] **2.3 Animation time & smoothing (FR-4, FR-9)**
  - [x] Implement exponential smoothing with `tau` derived from `animation_time_ms`
  - [x] Implement `linear`, `ease_out_quad`, `ease_out_cubic`, `ease_out_expo` segment-based modes
  - [x] Property test: total emitted distance == requested distance for any `dt` sequence
  - [x] Test frame-rate independence (60/120/144/240 Hz produce same final distance/timing)
- [x] **2.4 Fractional remainder handling (FR-10)**
  - [x] px-equivalent → wheel-unit conversion with calibration constant
  - [x] Carry sub-unit remainder per axis; flush at end of animation
  - [x] Test: no drift over 10,000 ticks
- [x] **2.5 Acceleration (FR-6)**
  - [x] Track inter-notch interval; compute multiplier up to `max_multiplier`
  - [x] Decay multiplier when cadence slows
  - [x] Tests for boundaries (slow, fast, capped)
- [x] **2.6 Direction reversal (FR-8)**
  - [x] On opposite-sign input, clear `target_remaining` and velocity before applying
  - [x] Test: reversal produces immediate opposite motion, no residual overshoot
- [x] **2.7 Direction inversion**
  - [x] `reverse_direction` flips sign at input stage
- [x] **2.8 Momentum mode (FR-7)** *(can be deferred to Phase 6 if schedule tight)*
  - [x] Velocity-based model with per-second friction, `min_velocity` cutoff
  - [x] Blend with accumulation model behind `momentum.enabled`
  - [x] Tests: decay to zero in bounded time; no negative overshoot
- [~] **2.9 Engine hardening**
  - [x] Clamp `dt` (e.g., ≤ 100 ms) to survive stalls/sleep
  - [x] Saturating arithmetic; overflow-safe conversions
  - [~] Fuzz/property tests with random inputs

**Gate 2:** `cargo test` green on any OS; engine has no Win32 dependencies; behavior verified against a simulated timeline.

---

## Phase 3 — Real-Time Loop Integration (M2 cont.)

- [-] **3.1 High-resolution timing (`platform/win32/timer.rs`)**
  - [~] Create waitable timer with `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`
  - [~] Fallback to `timeBeginPeriod(1)` + sleep if unsupported
  - [ ] Optional: `DwmFlush()` vsync-aligned ticking
  - [~] Detect display refresh rate (`EnumDisplaySettingsW`) for `frame_rate = "auto"`
- [x] **3.2 Animator thread**
  - [x] Park until first input; run tick loop; re-park when `engine.is_idle()`
  - [x] Measure real `dt` via `QueryPerformanceCounter`/`Instant`
  - [x] Drain input channel each tick, feed engine, emit output via `inject`
  - [x] Ensure no more than one `SendInput` batch per tick
  - [x] Stop cleanly on shutdown/disable (cancel remaining distance)
- [ ] **3.3 Verify idle behavior**
  - [ ] Confirm 0% CPU when not scrolling (Task Manager / `typeperf`)
  - [ ] Confirm timer released while parked

**Gate 3:** smooth at 60 Hz and high-refresh displays; idle CPU ≈ 0%.

---

## Phase 4 — Configuration System (M3)

- [x] **4.1 Schema & defaults (`config/`)**
  - [x] `serde` structs mirroring PRD §5.2/§8
  - [x] Defaults matching PRD table
  - [x] Validation + clamping to allowed ranges (FR-16)
- [x] **4.2 Presets (FR-17)**
  - [x] Define `default`, `snappy`, `silky`, `tablet` presets
  - [x] `preset = "..."` merges under explicit user values
- [x] **4.3 File handling (FR-14)**
  - [x] Resolve `%APPDATA%\Glide\config.toml`
  - [x] Create directory + commented default config on first run
  - [x] `--config <path>` override
  - [x] `--print-default-config` CLI flag
- [~] **4.4 Live reload (FR-15)**
  - [x] Watch config mtime (poll ~1 s) or `ReadDirectoryChangesW` if size-justified
  - [x] Parse → validate → atomically swap config snapshot
  - [~] On error: keep last-good config, notify via tray balloon + log line
  - [x] Ensure in-flight animation continues smoothly across reload
- [x] **4.5 Config → EngineConfig mapping**
  - [x] Per-app override resolution (base config + `apps.overrides.<exe>`)
- [x] **4.6 Tests**
  - [x] Parse valid/invalid/partial configs
  - [x] Clamping behavior
  - [x] Preset merge precedence
  - [x] Round-trip default config

**Gate 4:** editing `config.toml` changes step size/animation time live; bad config never crashes.

---

## Phase 5 — App Shell: Tray, Hotkey, Lifecycle (M4)

- [~] **5.1 Single instance (FR-19)**
  - [~] Named mutex; second launch exits (optionally signals first to open config)
- [~] **5.2 Tray icon (`platform/win32/tray.rs`, FR-18)**
  - [~] Hidden message window + `Shell_NotifyIconW`
  - [~] Menu: Enable/Disable, Preset submenu, Open config, Reload, Start with Windows, Quit
  - [~] Handle `TaskbarCreated` (Explorer restart) to re-add the icon
  - [~] Balloon/toast for config errors
  - [~] Dark/light-mode friendly icon
- [x] **5.3 Global hotkey (`hotkey.rs`)**
  - [x] Parse `toggle_hotkey` string; `RegisterHotKey`
  - [x] Toggle `enabled` and cancel active animation on disable
- [~] **5.4 Autostart (`autostart.rs`)**
  - [~] Add/remove `HKCU\...\Run` value
  - [~] Reflect actual registry state in tray checkmark
- [~] **5.5 Shutdown handling**
  - [~] Handle `WM_QUERYENDSESSION`/`WM_ENDSESSION` and Quit menu
  - [~] Join threads, unhook, remove tray icon

**Gate 5:** app is fully controllable from tray and hotkey; survives Explorer restart; clean shutdown.

---

## Phase 6 — Compatibility & Intelligence (M5)

- [~] **6.1 Pass-through rules (PRD §7.4)**
  - [~] Precision-device heuristic: pass through when `abs(delta) % 120 != 0`
  - [~] Bypass modifier (Alt default) via `GetAsyncKeyState`
  - [~] Ctrl+wheel pass-through (zoom) option
  - [~] Shift+wheel → horizontal translation (FR-11)
- [-] **6.2 Target window detection (`window.rs`, FR-20)**
  - [~] `WindowFromPoint` at cursor → root window → PID → exe name (`QueryFullProcessImageNameW`)
  - [~] HWND→exe cache with TTL; avoid heavy calls in the hook fast path
  - [ ] Benchmark hook callback latency (target p99 < 100 µs)
  - [~] Exclude list, include-only (allowlist) mode
  - [x] Per-app override application
- [~] **6.3 Elevated window handling**
  - [~] Detect elevated target (token elevation check) when Glide is not elevated
  - [~] Pass through instead of swallowing (avoid dead scrolling)
  - [x] Document optional "run as admin" / scheduled-task approach
- [~] **6.4 Fullscreen/game detection (FR-21)**
  - [~] Detect foreground window covering monitor / D3D fullscreen
  - [x] Default exclusion list of common game/anti-cheat processes
  - [x] Config toggle `auto_disable_in_fullscreen`
- [-] **6.5 Hook resilience (FR-22)**
  - [ ] Heartbeat/watchdog detecting silently dropped hook
  - [~] Reinstall hook automatically; rate-limit reinstall attempts
  - [~] Handle session lock/unlock and remote-desktop transitions
- [ ] **6.6 Compatibility matrix testing**
  - [ ] Chrome, Edge, Firefox, VS Code, Explorer, Notepad, Terminal
  - [ ] Word, Excel, PDF reader, Slack/Discord (Electron)
  - [ ] Legacy Win32 app (verify fraction handling; add quantization override if needed)
  - [ ] Precision touchpad on laptop (no double smoothing)
  - [ ] Multi-monitor with mixed refresh rates and DPI scaling

**Gate 6:** documented compatibility matrix; no dead-scroll cases; hook survives lock/unlock and sleep/resume.

---

## Phase 7 — Momentum, Polish & Hardening (M6)

- [~] **7.1 Momentum finalization** (if not completed in 2.8)
  - [~] Tune friction and thresholds; expose in `tablet` preset
  - [~] Ensure cancel-on-reverse and cancel-on-click (stop inertia when mouse button pressed)
- [-] **7.2 Feel tuning**
  - [~] Build an internal debug mode (`--trace`) writing a scroll curve CSV (dev only, no default logging)
  - [ ] Tune defaults for `default`, `snappy`, `silky` presets against real usage
  - [ ] A/B against stock scrolling and reference apps
- [ ] **7.3 Performance & memory audit**
  - [ ] Measure idle/active working set (target ≤ 5 MB / ≤ 10 MB)
  - [ ] Measure CPU during continuous scroll (< 2% of one core)
  - [ ] Profile allocations; ensure hook path allocates nothing
  - [ ] Re-check binary size; try `opt-level="s"` vs `"z"`, `panic=abort`, feature pruning
- [-] **7.4 Safety audit**
  - [~] Review every `unsafe` block and `// SAFETY:` comment
  - [ ] Run Miri on engine tests; run under sanitizers where feasible
  - [ ] `cargo audit` / `cargo deny` clean
  - [~] Fuzz config parser and engine input sequences
- [ ] **7.5 Soak & stress tests**
  - [ ] 24-hour run: no memory growth, no hook loss
  - [ ] Rapid wheel spam and reversal stress
  - [ ] Sleep/resume, display hot-plug, RDP, fast user switching
  - [ ] Verify no stuck-scroll state after crash/kill

**Gate 7:** all NFR targets met and recorded in a `BENCHMARKS.md`.

---

## Phase 8 — Packaging, Docs & Release (M7)

- [ ] **8.1 Distribution**
  - [ ] Reproducible release build for x64 and ARM64
  - [ ] Code-sign the executable (mitigate AV false positives)
  - [ ] Publish SHA-256 hashes
  - [ ] Optional: `winget` manifest, portable zip, MSI/MSIX
- [~] **8.2 Documentation**
  - [~] `README.md`: install, usage, screenshots of tray menu
  - [~] `docs/CONFIG.md`: every setting with range/default/examples
  - [~] `docs/COMPATIBILITY.md`: known-good/known-problematic apps, UIPI note, games note
  - [~] `docs/ARCHITECTURE.md`: threading model and unsafe boundary
  - [~] Troubleshooting section (hook dropped, elevated apps, antivirus)
- [-] **8.3 Release process**
  - [~] Semantic versioning, `CHANGELOG.md`
  - [ ] GitHub Release with artifacts
  - [ ] Tag `v1.0.0`

**Gate 8:** clean install-and-run on a fresh Windows 10 and Windows 11 machine.

---

## Backlog / Future Ideas (Post-v1)

- [ ] Small GUI settings window with live preview
- [ ] Per-monitor settings
- [ ] Middle-click auto-scroll smoothing
- [ ] Keyboard (PageUp/PageDown/arrow) smoothing
- [ ] Scroll curve visual editor
- [ ] Profile switching by active app or time of day
- [ ] macOS (`CGEventTap`) and Linux (`libinput`/`uinput`) ports via the platform trait
- [ ] Localization

---

## Progress Summary

| Phase | Description | Status |
|---|---|---|
| 0 | Scaffold & tooling | [~] |
| 1 | Hook + injection MVP | [~] |
| 2 | Engine (pure Rust) | [~] |
| 3 | Real-time loop integration | [~] |
| 4 | Configuration system | [~] |
| 5 | Tray, hotkey, lifecycle | [~] |
| 6 | Compatibility & intelligence | [~] |
| 7 | Momentum, polish, hardening | [-] |
| 8 | Packaging, docs, release | [-] |
