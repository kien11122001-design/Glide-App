# Architecture

```
hook thread ──(lock-free SPSC ring, unpark)──▶ animator thread ──SendInput──▶ apps
   ▲ WH_MOUSE_LL, pass-through rules                 Engine.tick(dt) per display frame
UI thread: tray, hotkey, config poll (1 s), fullscreen poll (250 ms), session/power events
```
* `engine/` – pure physics (no Win32, no clock): exponential smoothing / eased segments / momentum,
  acceleration, reversal, remainder carry. A live config swap migrates in-flight motion between models.
* `config/` – TOML → `Patch` → preset → clamp → `Config` snapshot (atomically swapped in `state::Shared`).
* `animator.rs` – parks while idle; frame loop against a `Backend` trait (virtual clock in tests).
* `platform/win32/` – the **only** module with `unsafe` (`scripts/check-unsafe.sh`): hook, inject, timer,
  window/process lookup (+UIPI + fullscreen), tray, hotkey, autostart, system helpers.
* `app.rs` – safe wiring of the above.

Hook rules (cheap-first): injected → pass; disabled → pass; delta % 120 ≠ 0 → pass; bypass modifier → pass;
Ctrl+wheel → pass; excluded / UIPI-blocked target → pass; fullscreen game → pass; per-app disabled → pass;
otherwise swallow and queue (Shift+wheel becomes a horizontal event with inverted sign).
