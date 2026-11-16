//! Per-axis accumulator: turns notches into a stream of small wheel-unit
//! outputs. See `CLAUDE.md` §7.3 for the design this implements.
//!
//! Two accumulation styles share one `target`-style field pair so
//! `is_settled` needs no config:
//! - **Rate-based** (`target_remaining`): exponential smoothing and momentum
//!   both decay a single outstanding distance by a per-frame fraction
//!   (`alpha`). The fraction differs (tau-derived vs. friction-derived) but
//!   the shrink-toward-zero math, and thus the "flush the tail" trick used
//!   to settle in finite time, is identical.
//! - **Curve-based** (`target_total`/`progress`): `linear`/`ease_out_*` spawn
//!   a fixed-duration segment and emit the derivative of the eased curve.
//!
//! Because a config's easing/momentum selection decides which field pair is
//! written, the unused pair simply stays at zero, so summing both is a safe
//! settle test even across a live-reloaded config.

use super::easing::SMOOTHING_K;
use super::{Easing, EngineConfig, WHEEL_DELTA};

/// Below this many px-equivalents remaining, flush the rest immediately
/// rather than let an exponential/friction decay approach zero forever.
const EPS_PX: f64 = 0.3;

#[derive(Clone, Copy, Debug)]
pub struct AxisState {
    /// Outstanding distance for the rate-based (exponential/momentum) model.
    target_remaining: f64,
    /// Segment target/progress for the curve-based model.
    target_total: f64,
    progress: f64,
    elapsed: f64,
    /// Sub-wheel-unit carry so fractional deltas are never lost (FR-10).
    fractional_remainder: f64,
    last_notch_t_us: Option<u64>,
    multiplier: f64,
}

impl AxisState {
    pub fn new() -> Self {
        AxisState {
            target_remaining: 0.0,
            target_total: 0.0,
            progress: 0.0,
            elapsed: 0.0,
            fractional_remainder: 0.0,
            last_notch_t_us: None,
            multiplier: 1.0,
        }
    }

    /// Update the acceleration multiplier from notch cadence (FR-6), then
    /// fold one notch into whichever accumulation model `cfg` selects.
    pub fn push(&mut self, cfg: &EngineConfig, delta: i32, t_us: u64) {
        if delta == 0 {
            return;
        }

        if cfg.accel.enabled {
            match self.last_notch_t_us {
                None => self.multiplier = 1.0,
                Some(last) => {
                    let interval_ms = t_us.saturating_sub(last) as f64 / 1000.0;
                    if interval_ms <= cfg.accel.window_ms {
                        self.multiplier = (self.multiplier + 0.5).min(cfg.accel.max_multiplier);
                    } else if interval_ms > cfg.accel.window_ms * 2.0 {
                        // Cadence went cold: back to unaccelerated.
                        self.multiplier = 1.0;
                    }
                    // Between window and 2*window: hold steady.
                }
            }
        } else {
            self.multiplier = 1.0;
        }
        self.last_notch_t_us = Some(t_us);

        let dir = if cfg.reverse_direction { -1.0 } else { 1.0 };
        let notches = f64::from(delta) / f64::from(WHEEL_DELTA);
        let distance = notches * cfg.step_size * self.multiplier * dir;
        if !distance.is_finite() || distance == 0.0 {
            return;
        }

        if cfg.momentum.enabled || cfg.easing == Easing::ExponentialSmoothing {
            // FR-8: an opposite-sign notch cancels remaining motion first.
            if self.target_remaining != 0.0 && distance.signum() != self.target_remaining.signum() {
                self.target_remaining = 0.0;
            }
            self.target_remaining += distance;
            if !self.target_remaining.is_finite() {
                // Saturate instead of overflowing to NaN/inf on pathological input.
                self.target_remaining = distance.signum() * 1.0e15;
            }
        } else {
            let seg_remaining = self.target_total * (1.0 - self.progress);
            self.target_total =
                if seg_remaining != 0.0 && distance.signum() != seg_remaining.signum() {
                    distance
                } else {
                    seg_remaining + distance
                };
            self.progress = 0.0;
            self.elapsed = 0.0;
        }
    }

    /// Advance by `dt` seconds (already clamped/sanitized by the caller) and
    /// return the whole wheel units to inject this frame.
    pub fn tick(&mut self, cfg: &EngineConfig, dt: f64) -> i32 {
        if dt <= 0.0 {
            return self.emit(cfg, 0.0, false);
        }

        let (moved_px, settling) = if cfg.momentum.enabled {
            let alpha = 1.0 - cfg.momentum.friction.powf(dt * 60.0);
            self.decay_remaining(alpha)
        } else if cfg.easing == Easing::ExponentialSmoothing {
            let tau = (cfg.animation_time_ms / 1000.0) / SMOOTHING_K;
            let alpha = if tau <= 0.0 {
                1.0
            } else {
                1.0 - (-dt / tau).exp()
            };
            self.decay_remaining(alpha)
        } else {
            let duration = (cfg.animation_time_ms / 1000.0).max(1e-6);
            self.elapsed += dt;
            let t = (self.elapsed / duration).min(1.0);
            let new_progress = cfg.easing.apply(t);
            let mv = self.target_total * (new_progress - self.progress);
            self.progress = new_progress;
            let settling = t >= 1.0;
            if settling {
                // Fully flushed; reset so the next notch starts a clean segment.
                self.target_total = 0.0;
                self.progress = 0.0;
                self.elapsed = 0.0;
            }
            (mv, settling)
        };

        self.emit(cfg, moved_px, settling)
    }

    /// Shrink `target_remaining` by `alpha`, flushing the tail once it's
    /// within `EPS_PX` so settling happens in finite time with zero loss.
    /// Returns the px moved this frame and whether motion just settled.
    fn decay_remaining(&mut self, alpha: f64) -> (f64, bool) {
        let mv = self.target_remaining * alpha;
        self.target_remaining -= mv;
        if self.target_remaining.abs() < EPS_PX {
            let leftover = self.target_remaining;
            self.target_remaining = 0.0;
            (mv + leftover, true)
        } else {
            (mv, false)
        }
    }

    /// px-equivalent -> whole wheel units, carrying the fractional remainder.
    /// On the settling frame, the carried remainder is rounded into the
    /// output rather than truncated away, so the running total lands exactly
    /// on the requested distance instead of losing up to one unit forever.
    fn emit(&mut self, cfg: &EngineConfig, moved_px: f64, settling: bool) -> i32 {
        if moved_px == 0.0 && self.fractional_remainder == 0.0 {
            return 0;
        }
        let units =
            moved_px * f64::from(WHEEL_DELTA) / cfg.px_per_notch + self.fractional_remainder;
        let whole = if settling {
            units.round()
        } else {
            units.trunc()
        };
        self.fractional_remainder = if settling { 0.0 } else { units - whole };
        // `as i32` on floats is a saturating cast (defined Rust behavior):
        // extreme accumulated input clamps instead of overflowing/panicking.
        whole as i32
    }

    pub fn is_settled(&self) -> bool {
        self.target_remaining.abs() < EPS_PX && self.target_total.abs() < EPS_PX
    }

    /// Drop all remaining motion (disable / quit / click). Acceleration
    /// cadence tracking is left untouched; only in-flight distance is lost.
    pub fn reset(&mut self) {
        self.target_remaining = 0.0;
        self.target_total = 0.0;
        self.progress = 0.0;
        self.elapsed = 0.0;
        self.fractional_remainder = 0.0;
    }

    pub fn accel_multiplier(&self) -> f64 {
        self.multiplier
    }
}
