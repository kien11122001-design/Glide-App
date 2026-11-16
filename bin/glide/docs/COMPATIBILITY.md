# Compatibility notes

> Status: **not yet verified on real hardware** (development environment had no Windows).
> The matrix below lists what to test (plan item 6.6) and the behaviour designed in.

## Behaviour by design
* **Elevated (admin) windows**: Windows (UIPI) drops injected input to elevated targets, so Glide
  detects them and passes the real wheel events through (no "dead scrolling"). To smooth them, run Glide elevated
  (e.g. a scheduled task "run with highest privileges").
* **Games**: default exclude list + automatic pause when a borderless/exclusive fullscreen window is foreground.
  Browsers, video players and presenters are exempt from that pause. Injecting input near anti-cheat carries risk; keep games excluded.
* **Touchpads / precision wheels**: events whose delta is not a multiple of 120 pass through untouched.
* **Other injectors** (mouse drivers, remote sessions, AutoHotkey): injected wheel events pass through.
* **UWP apps** appear as `applicationframehost.exe` to `exclude`/`include_only`.
* **Legacy apps** that ignore sub-120 deltas may scroll in coarse steps; use a per-app override.

## Test matrix (fill in)
Chrome, Edge, Firefox, VS Code, Explorer, Notepad, Terminal, Word, Excel, PDF reader, Slack/Discord,
a legacy Win32 app, a precision touchpad laptop, multi-monitor mixed refresh/DPI.

## Troubleshooting
* Smoothing stopped after unlock/sleep: Glide re-installs the hook automatically; toggle it from the tray if not.
* Antivirus flags the exe: global hooks + input injection look suspicious; the source is open, hashes are published with releases.
