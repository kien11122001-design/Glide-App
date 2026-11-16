use super::*;

#[test]
fn default_template_matches_builtin_defaults() {
    // Round-trip: the commented file we write on first run means exactly the
    // built-in defaults.
    assert_eq!(parse(DEFAULT_CONFIG_TOML), Ok(Config::default()));
}

#[test]
fn empty_and_partial_files_use_defaults() {
    assert_eq!(parse(""), Ok(Config::default()));
    let c = parse("step_size = 200\n[momentum]\nenabled = true\n").unwrap();
    assert_eq!(c.base.engine.step_size, 200.0);
    assert!(c.base.engine.momentum.enabled);
    assert_eq!(c.base.engine.animation_time_ms, 300.0);
}

#[test]
fn utf8_bom_is_ignored() {
    assert!(parse("\u{feff}step_size = 150\n").is_ok());
}

#[test]
fn invalid_files_are_errors_not_panics() {
    for bad in [
        "step_size = ",
        "step_size = \"lots\"",
        "easing = \"bouncy\"",
        "preset = \"nope\"",
        "[input]\nbypass_modifier = \"hyper\"",
        "frame_rate = \"fast\"",
        "step_sise = 100",
        "[general]\ntoggle_hotkey = \"s\"",
        "[apps.overrides.\"a.exe\"]\neasing = \"x\"",
    ] {
        assert!(parse(bad).is_err(), "should reject: {bad}");
    }
}

#[test]
fn values_are_clamped_to_prd_ranges() {
    let c = parse(
        "step_size = 99999\nanimation_time_ms = 1\nframe_rate = 1000\n\
         [acceleration]\nmax_multiplier = 50.0\nwindow_ms = 1\n\
         [momentum]\nfriction = 5.0\nmin_velocity = -3.0\n",
    )
    .unwrap();
    let e = c.base.engine;
    assert_eq!(e.step_size, 1000.0);
    assert_eq!(e.animation_time_ms, 50.0);
    assert_eq!(e.accel.max_multiplier, 10.0);
    assert_eq!(e.accel.window_ms, 20.0);
    assert_eq!(e.momentum.friction, 0.999);
    assert!(e.momentum.min_velocity > 0.0);
    assert_eq!(c.frame_rate, FrameRate::Fixed(360));
    assert_eq!(
        parse("frame_rate = 5").unwrap().frame_rate,
        FrameRate::Fixed(30)
    );
}

#[test]
fn non_finite_values_fall_back() {
    let c = parse("step_size = nan\nanimation_time_ms = inf\n").unwrap();
    assert_eq!(c.base.engine.step_size, 120.0);
    assert_eq!(c.base.engine.animation_time_ms, 300.0);
}

#[test]
fn every_preset_parses_and_differs() {
    let default = parse("").unwrap();
    for name in PRESET_NAMES {
        let c = parse(&format!("preset = \"{name}\"")).unwrap();
        if name != "default" {
            assert_ne!(c.base, default.base, "{name}");
        }
    }
    assert!(
        parse("preset = \"tablet\"")
            .unwrap()
            .base
            .engine
            .momentum
            .enabled
    );
}

#[test]
fn explicit_values_beat_preset() {
    let c = parse("preset = \"snappy\"\nstep_size = 300\n").unwrap();
    assert_eq!(c.base.engine.step_size, 300.0);
    // untouched preset values survive
    assert_eq!(c.base.engine.animation_time_ms, 150.0);
}

#[test]
fn per_app_overrides_resolve_on_top_of_base() {
    let c = parse(
        "animation_time_ms = 400\n\
         [apps.overrides.\"Code.EXE\"]\nanimation_time_ms = 200\n\
         [apps.overrides.\"chrome.exe\"]\nenabled = false\n",
    )
    .unwrap();
    assert_eq!(c.overrides.len(), 2);

    let code = c.profile_for("code.exe");
    assert_ne!(code, 0);
    let s = c.settings_for_profile(code);
    assert_eq!(s.engine.animation_time_ms, 200.0);
    assert_eq!(s.engine.step_size, 120.0, "inherits the base");

    let chrome = c.settings_for_profile(c.profile_for("chrome.exe"));
    assert!(!chrome.enabled);

    assert_eq!(c.profile_for("notepad.exe"), 0);
    assert_eq!(c.settings_for_profile(0).engine.animation_time_ms, 400.0);
    assert_eq!(
        c.settings_for_profile(250),
        &c.base,
        "unknown id is the base"
    );
}

#[test]
fn exclude_and_allowlist() {
    let c = parse("[apps]\nexclude = [\"Game.exe\"]\n").unwrap();
    assert!(!c.app_allowed("game.exe"));
    assert!(c.app_allowed("code.exe"));

    let c = parse("[apps]\ninclude_only = [\"code.exe\"]\n").unwrap();
    assert!(c.app_allowed("code.exe"));
    assert!(!c.app_allowed("notepad.exe"));
}

#[test]
fn input_and_general_options() {
    let c = parse(
        "[input]\nbypass_modifier = \"Shift\"\nctrl_passthrough = false\n\
         [general]\nautostart = true\ntoggle_hotkey = \"ctrl+shift+F9\"\n",
    )
    .unwrap();
    assert_eq!(c.bypass_modifier, Modifier::Shift);
    assert!(!c.ctrl_passthrough);
    assert!(c.autostart);
    assert_eq!(c.toggle_hotkey, "ctrl+shift+F9");
    assert!(
        parse("[general]\ntoggle_hotkey = \"\"\n").is_ok(),
        "empty disables"
    );
}

#[test]
fn hotkey_parser() {
    let h = parse_hotkey("Ctrl+Alt+S").unwrap();
    assert!(h.ctrl && h.alt && !h.shift && !h.win);
    assert_eq!(h.vk, 0x53);
    assert_eq!(parse_hotkey("win+f12").unwrap().vk, 0x7B);
    assert_eq!(parse_hotkey("ctrl+1").unwrap().vk, 0x31);
    assert_eq!(parse_hotkey("alt+space").unwrap().vk, 0x20);
    assert!(parse_hotkey("s").is_none(), "needs a modifier");
    assert!(parse_hotkey("ctrl+alt").is_none(), "needs a key");
    assert!(parse_hotkey("ctrl+a+b").is_none(), "one key only");
    assert!(parse_hotkey("ctrl+f25").is_none());
    assert!(parse_hotkey("ctrl+bogus").is_none());
}

#[test]
fn set_preset_edits_only_that_line() {
    let t = "# hi\npreset = \"default\"   # note\nstep_size = 90\n";
    let out = set_preset_in_text(t, "silky");
    assert!(out.contains("preset = \"silky\"\n"));
    assert!(out.contains("# hi\n") && out.contains("step_size = 90\n"));
    assert_eq!(out.matches("preset").count(), 1);
    assert_eq!(parse(&out).unwrap().base.engine.step_size, 90.0);

    let added = set_preset_in_text("step_size = 90\n", "snappy");
    assert!(added.starts_with("preset = \"snappy\"\n"));
    assert!(parse(&added).is_ok());
}

#[test]
fn watcher_detects_changes() {
    let dir = std::env::temp_dir().join(format!("glide-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");

    ensure_default_file(&path).unwrap();
    let mut w = Watcher::new(&path);
    assert!(!w.changed());
    assert_eq!(load(&path), Ok(Config::default()));

    std::fs::remove_file(&path).unwrap();
    assert!(w.changed(), "deletion is a change");
    assert!(!w.changed());
    assert!(load(&path).is_err());

    std::fs::remove_dir_all(&dir).ok();
}
