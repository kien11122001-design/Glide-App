//! Pure smooth-scrolling physics.
//!
//! No Win32 types, no I/O, no clocks: time is always passed in. This makes
//! the whole module unit-testable on any OS.
//!
//! Units: distances are "px-equivalent". One wheel notch (120 wheel units)
//! maps to `px_per_notch` px-equivalents, so `units = px * 120 / px_per_notch`.
//! NOTE: incomplete file, supceptible to change

mod axis;
mod easing;
#[cfg(test)]
mod tests;

use axis::AxisState;
pub use easing::Easing;

/// Standard wheel delta of one notch.
pub const WHEEL_DELTA: i32 = 120;
/// Largest `dt` (seconds) accepted by [`Engine::tick`]; survives stalls/sleep.
pub const MAX_DT_S: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

/// One raw wheel event. `t_us` is a monotonic timestamp in microseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WheelInput {
    pub delta: i32,
    pub axis: Axis,
    pub t_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccelConfig {
    pub enabled: bool,
    pub max_multiplier: f64,
    pub window_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MomentumConfig {
    pub enabled: bool,
    /// Fraction of velocity kept every 1/60 s (frame-rate independent).
    pub friction: f64,
    /// Velocity (px/s) below which motion stops.
    pub min_velocity: f64,
}

/// Engine settings snapshot, independent of the config file format.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineConfig {
    pub step_size: f64,
    pub animation_time_ms: f64,
    pub easing: Easing,
    pub accel: AccelConfig,
    pub momentum: MomentumConfig,
    pub reverse_direction: bool,
    pub px_per_notch: f64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            step_size: 120.0,
            animation_time_ms: 300.0,
            easing: Easing::ExponentialSmoothing,
            accel: AccelConfig {
                enabled: true,
                max_multiplier: 4.0,
                window_ms: 120.0,
            },
            momentum: MomentumConfig {
                enabled: false,
                friction: 0.90,
                min_velocity: 0.5,
            },
            reverse_direction: false,
            px_per_notch: 120.0,
        }
    }
}

fn clamp_f(x: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        fallback
    }
}

impl EngineConfig {
    /// Clamp every field to a range the physics can safely handle.
    pub fn sanitized(mut self) -> Self {
        self.step_size = clamp_f(self.step_size, 1.0, 10_000.0, 120.0);
        self.animation_time_ms = clamp_f(self.animation_time_ms, 1.0, 10_000.0, 300.0);
        self.accel.max_multiplier = clamp_f(self.accel.max_multiplier, 1.0, 100.0, 1.0);
        self.accel.window_ms = clamp_f(self.accel.window_ms, 1.0, 5_000.0, 120.0);
        self.momentum.friction = clamp_f(self.momentum.friction, 0.5, 0.999, 0.90);
        self.momentum.min_velocity = clamp_f(self.momentum.min_velocity, 1e-3, 1e6, 0.5);
        self.px_per_notch = clamp_f(self.px_per_notch, 1.0, 10_000.0, 120.0);
        self
    }
}

/// Whole wheel units to inject this frame, per axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub v_units: i32,
    pub h_units: i32,
}

impl Output {
    pub fn is_zero(&self) -> bool {
        self.v_units == 0 && self.h_units == 0
    }
}

pub struct Engine {
    cfg: EngineConfig,
    v: AxisState,
    h: AxisState,
}

impl Engine {
    pub fn new(cfg: EngineConfig) -> Self {
        Engine {
            cfg: cfg.sanitized(),
            v: AxisState::new(),
            h: AxisState::new(),
        }
    }

    /// Swap settings; in-flight motion continues with the new parameters.
    pub fn set_config(&mut self, cfg: EngineConfig) {
        self.cfg = cfg.sanitized();
    }

    pub fn on_input(&mut self, input: WheelInput) {
        let cfg = self.cfg;
        match input.axis {
            Axis::Vertical => self.v.push(&cfg, input.delta, input.t_us),
            Axis::Horizontal => self.h.push(&cfg, input.delta, input.t_us),
        }
    }

    /// Advance by `dt_s` seconds (clamped to [0, `MAX_DT_S`]).
    pub fn tick(&mut self, dt_s: f64) -> Output {
        let dt = if dt_s.is_finite() {
            dt_s.clamp(0.0, MAX_DT_S)
        } else {
            0.0
        };
        let cfg = self.cfg;
        Output {
            v_units: self.v.tick(&cfg, dt),
            h_units: self.h.tick(&cfg, dt),
        }
    }

    pub fn is_idle(&self) -> bool {
        self.v.is_settled() && self.h.is_settled()
    }

    /// Drop all remaining motion (disable / quit / click).
    pub fn cancel(&mut self) {
        self.v.reset();
        self.h.reset();
    }

    /// Current acceleration multiplier of an axis (diagnostics/tests).
    pub fn accel_multiplier(&self, axis: Axis) -> f64 {
        match axis {
            Axis::Vertical => self.v.accel_multiplier(),
            Axis::Horizontal => self.h.accel_multiplier(),
        }
    }
}
