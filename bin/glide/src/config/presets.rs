//! Built-in presets (FR-17). A preset is a full [`Settings`]; explicit values in
//! the user's file are applied on top of it.

use super::Settings;

pub const NAMES: [&str; 4] = ["default", "snappy", "silky", "tablet"];

pub fn settings(name: &str) -> Option<Settings> {
    let mut s = Settings::default();
    let e = &mut s.engine;
    match name.trim().to_ascii_lowercase().as_str() {
        "default" => {}
        "snappy" => {
            e.step_size = 100.0;
            e.animation_time_ms = 150.0;
            e.accel.max_multiplier = 3.0;
            e.accel.window_ms = 100.0;
        }
        "silky" => {
            e.step_size = 110.0;
            e.animation_time_ms = 550.0;
            e.accel.max_multiplier = 5.0;
        }
        "tablet" => {
            e.step_size = 140.0;
            e.momentum.enabled = true;
            e.momentum.friction = 0.92;
        }
        _ => return None,
    }
    Some(s)
}
