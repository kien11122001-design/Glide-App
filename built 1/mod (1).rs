//! Configuration: TOML schema, presets, validation and per-app overrides.
//!
//! Pipeline: text -> [`Patch`]/`Extra` (all fields optional) -> start from the
//! preset's [`Settings`] -> apply explicit user values on top -> clamp to the
//! PRD ranges. Explicit values therefore always beat the preset.

mod hotkey;
mod presets;
mod watch;

pub use hotkey::{parse_hotkey, Hotkey};
pub use presets::NAMES as PRESET_NAMES;
pub use watch::Watcher;

use crate::engine::{Easing, EngineConfig};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// Commented default config written on first run (`--print-default-config`).
pub const DEFAULT_CONFIG_TOML: &str = include_str!("default_config.toml");

/// Built-in exclusions: common fullscreen games (input injection near
/// anti-cheat is a needless risk).
pub const DEFAULT_EXCLUDE: [&str; 8] = [
    "cs2.exe",
    "csgo.exe",
    "dota2.exe",
    "eldenring.exe",
    "gta5.exe",
    "r5apex.exe",
    "valorant-win64-shipping.exe",
    "fortniteclient-win64-shipping.exe",
];

/// Profiles are addressed by a `u8` (0 = base, n = override n-1).
const MAX_OVERRIDES: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

fn err<T>(msg: impl Into<String>) -> Result<T, ConfigError> {
    Err(ConfigError(msg.into()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    None,
    Alt,
    Ctrl,
    Shift,
    Win,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameRate {
    Auto,
    Fixed(u32),
}

/// Everything that can differ per application.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub engine: EngineConfig,
    pub horizontal_enabled: bool,
    pub shift_horizontal: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            engine: EngineConfig::default(),
            horizontal_enabled: true,
            shift_horizontal: true,
        }
    }
}

/// Fully resolved, validated configuration snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub base: Settings,
    pub frame_rate: FrameRate,
    pub bypass_modifier: Modifier,
    pub ctrl_passthrough: bool,
    pub exclude: Vec<String>,
    pub include_only: Vec<String>,
    pub auto_disable_in_fullscreen: bool,
    /// (lowercase exe name, settings), sorted by name.
    pub overrides: Vec<(String, Settings)>,
    pub autostart: bool,
    pub toggle_hotkey: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            base: Settings::default(),
            frame_rate: FrameRate::Auto,
            bypass_modifier: Modifier::Alt,
            ctrl_passthrough: true,
            exclude: DEFAULT_EXCLUDE.iter().map(|s| (*s).to_owned()).collect(),
            include_only: Vec::new(),
            auto_disable_in_fullscreen: true,
            overrides: Vec::new(),
            autostart: false,
            toggle_hotkey: "ctrl+alt+s".to_owned(),
        }
    }
}

impl Config {
    /// Profile id for `exe` (0 = base settings). `exe` must be lowercase.
    pub fn profile_for(&self, exe: &str) -> u8 {
        self.overrides
            .iter()
            .position(|(name, _)| name == exe)
            .and_then(|i| u8::try_from(i + 1).ok())
            .unwrap_or(0)
    }

    pub fn settings_for_profile(&self, profile: u8) -> &Settings {
        match usize::from(profile).checked_sub(1) {
            Some(i) => self.overrides.get(i).map_or(&self.base, |(_, s)| s),
            None => &self.base,
        }
    }

    /// Allowlist / exclude decision. `exe` must be lowercase.
    pub fn app_allowed(&self, exe: &str) -> bool {
        if self.include_only.is_empty() {
            !self.exclude.iter().any(|e| e == exe)
        } else {
            self.include_only.iter().any(|e| e == exe)
        }
    }
}

// ---------------------------------------------------------------------------
// Raw (file) schema: every field optional so presets can sit underneath.
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
struct AccelPatch {
    enabled: Option<bool>,
    max_multiplier: Option<f64>,
    window_ms: Option<f64>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
struct MomentumPatch {
    enabled: Option<bool>,
    friction: Option<f64>,
    min_velocity: Option<f64>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
struct HorizontalPatch {
    enabled: Option<bool>,
    shift_horizontal: Option<bool>,
}

/// Settings that may appear at top level *and* inside `[apps.overrides.<exe>]`.
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
struct Patch {
    enabled: Option<bool>,
    step_size: Option<f64>,
    animation_time_ms: Option<f64>,
    easing: Option<String>,
    reverse_direction: Option<bool>,
    acceleration: AccelPatch,
    momentum: MomentumPatch,
    horizontal: HorizontalPatch,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
enum RawFrameRate {
    Fixed(i64),
    Text(String),
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
struct InputRaw {
    bypass_modifier: Option<String>,
    ctrl_passthrough: Option<bool>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
struct AppsRaw {
    exclude: Option<Vec<String>>,
    include_only: Option<Vec<String>>,
    auto_disable_in_fullscreen: Option<bool>,
    overrides: HashMap<String, Patch>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
struct GeneralRaw {
    autostart: Option<bool>,
    toggle_hotkey: Option<String>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
struct Extra {
    preset: Option<String>,
    frame_rate: Option<RawFrameRate>,
    input: InputRaw,
    apps: AppsRaw,
    general: GeneralRaw,
}

const KNOWN_KEYS: [&str; 13] = [
    "preset",
    "enabled",
    "step_size",
    "animation_time_ms",
    "easing",
    "reverse_direction",
    "frame_rate",
    "acceleration",
    "momentum",
    "horizontal",
    "input",
    "apps",
    "general",
];

fn clamp_f(x: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        fallback
    }
}

/// FR-16: clamp to the PRD ranges (the engine additionally sanitizes).
fn clamp_settings(s: &mut Settings) {
    let e = &mut s.engine;
    e.step_size = clamp_f(e.step_size, 20.0, 1000.0, 120.0);
    e.animation_time_ms = clamp_f(e.animation_time_ms, 50.0, 2000.0, 300.0);
    e.accel.max_multiplier = clamp_f(e.accel.max_multiplier, 1.0, 10.0, 4.0);
    e.accel.window_ms = clamp_f(e.accel.window_ms, 20.0, 500.0, 120.0);
    e.momentum.friction = clamp_f(e.momentum.friction, 0.5, 0.999, 0.90);
    e.momentum.min_velocity = clamp_f(e.momentum.min_velocity, 1e-3, 1e6, 0.5);
}

fn apply(s: &mut Settings, p: &Patch) -> Result<(), ConfigError> {
    if let Some(v) = p.enabled {
        s.enabled = v;
    }
    if let Some(v) = p.step_size {
        s.engine.step_size = v;
    }
    if let Some(v) = p.animation_time_ms {
        s.engine.animation_time_ms = v;
    }
    if let Some(name) = &p.easing {
        match Easing::from_name(name) {
            Some(e) => s.engine.easing = e,
            None => return err(format!("unknown easing `{name}`")),
        }
    }
    if let Some(v) = p.reverse_direction {
        s.engine.reverse_direction = v;
    }
    if let Some(v) = p.acceleration.enabled {
        s.engine.accel.enabled = v;
    }
    if let Some(v) = p.acceleration.max_multiplier {
        s.engine.accel.max_multiplier = v;
    }
    if let Some(v) = p.acceleration.window_ms {
        s.engine.accel.window_ms = v;
    }
    if let Some(v) = p.momentum.enabled {
        s.engine.momentum.enabled = v;
    }
    if let Some(v) = p.momentum.friction {
        s.engine.momentum.friction = v;
    }
    if let Some(v) = p.momentum.min_velocity {
        s.engine.momentum.min_velocity = v;
    }
    if let Some(v) = p.horizontal.enabled {
        s.horizontal_enabled = v;
    }
    if let Some(v) = p.horizontal.shift_horizontal {
        s.shift_horizontal = v;
    }
    Ok(())
}

fn parse_modifier(name: &str) -> Result<Modifier, ConfigError> {
    match name.trim().to_ascii_lowercase().as_str() {
        "none" | "" => Ok(Modifier::None),
        "alt" => Ok(Modifier::Alt),
        "ctrl" | "control" => Ok(Modifier::Ctrl),
        "shift" => Ok(Modifier::Shift),
        "win" | "super" | "meta" => Ok(Modifier::Win),
        other => err(format!(
            "bypass_modifier `{other}`: expected none/alt/ctrl/shift/win"
        )),
    }
}

fn parse_frame_rate(raw: &RawFrameRate) -> Result<FrameRate, ConfigError> {
    match raw {
        RawFrameRate::Fixed(n) => {
            let hz = (*n).clamp(30, 360);
            Ok(FrameRate::Fixed(u32::try_from(hz).unwrap_or(60)))
        }
        RawFrameRate::Text(t) if t.trim().eq_ignore_ascii_case("auto") => Ok(FrameRate::Auto),
        RawFrameRate::Text(t) => err(format!("frame_rate `{t}`: expected \"auto\" or 30-360")),
    }
}

fn lower_list(list: &[String]) -> Vec<String> {
    list.iter().map(|s| s.trim().to_ascii_lowercase()).collect()
}

/// Parse and validate a config file's text.
pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let text = text.trim_start_matches('\u{feff}');

    let table: toml::Table = text
        .parse()
        .map_err(|e| ConfigError(format!("TOML syntax error: {e}")))?;
    // Surface typos instead of silently ignoring them.
    if let Some(key) = table.keys().find(|k| !KNOWN_KEYS.contains(&k.as_str())) {
        return err(format!("unknown key `{key}`"));
    }

    let patch: Patch =
        toml::from_str(text).map_err(|e| ConfigError(format!("invalid value: {e}")))?;
    let extra: Extra =
        toml::from_str(text).map_err(|e| ConfigError(format!("invalid value: {e}")))?;

    let preset = extra.preset.as_deref().unwrap_or("default");
    let Some(mut base) = presets::settings(preset) else {
        return err(format!(
            "unknown preset `{preset}` (expected {})",
            PRESET_NAMES.join(" | ")
        ));
    };
    apply(&mut base, &patch)?;
    clamp_settings(&mut base);

    if extra.apps.overrides.len() > MAX_OVERRIDES {
        return err(format!("too many app overrides (max {MAX_OVERRIDES})"));
    }
    let mut overrides = Vec::with_capacity(extra.apps.overrides.len());
    for (exe, p) in &extra.apps.overrides {
        let mut s = base;
        apply(&mut s, p).map_err(|e| ConfigError(format!("[apps.overrides.\"{exe}\"]: {e}")))?;
        clamp_settings(&mut s);
        overrides.push((exe.trim().to_ascii_lowercase(), s));
    }
    overrides.sort_by(|a, b| a.0.cmp(&b.0));

    let defaults = Config::default();
    let hotkey = extra
        .general
        .toggle_hotkey
        .unwrap_or_else(|| defaults.toggle_hotkey.clone());
    if !hotkey.trim().is_empty() && parse_hotkey(&hotkey).is_none() {
        return err(format!("toggle_hotkey `{hotkey}` is not a valid hotkey"));
    }

    Ok(Config {
        base,
        frame_rate: match &extra.frame_rate {
            Some(raw) => parse_frame_rate(raw)?,
            None => FrameRate::Auto,
        },
        bypass_modifier: match &extra.input.bypass_modifier {
            Some(m) => parse_modifier(m)?,
            None => defaults.bypass_modifier,
        },
        ctrl_passthrough: extra
            .input
            .ctrl_passthrough
            .unwrap_or(defaults.ctrl_passthrough),
        exclude: extra
            .apps
            .exclude
            .as_deref()
            .map_or(defaults.exclude, lower_list),
        include_only: extra
            .apps
            .include_only
            .as_deref()
            .map_or(defaults.include_only, lower_list),
        auto_disable_in_fullscreen: extra
            .apps
            .auto_disable_in_fullscreen
            .unwrap_or(defaults.auto_disable_in_fullscreen),
        overrides,
        autostart: extra.general.autostart.unwrap_or(defaults.autostart),
        toggle_hotkey: hotkey,
    })
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// `%APPDATA%\Glide\config.toml` (falls back to `./config.toml`).
pub fn default_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("Glide"))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("config.toml")
}

/// FR-14: create the directory and a commented default file on first run.
pub fn ensure_default_file(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, DEFAULT_CONFIG_TOML)
}

pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ConfigError(format!("cannot read {}: {e}", path.display())))?;
    parse(&text)
}

/// Set `preset = "<name>"` in config text, keeping everything else (tray menu).
pub fn set_preset_in_text(text: &str, name: &str) -> String {
    let line = format!("preset = \"{name}\"");
    let mut replaced = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let t = l.trim_start();
            let is_preset = !replaced
                && t.starts_with("preset")
                && t["preset".len()..].trim_start().starts_with('=');
            if is_preset {
                replaced = true;
                line.clone()
            } else {
                l.to_owned()
            }
        })
        .collect();
    if !replaced {
        out.insert(0, line);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests;
