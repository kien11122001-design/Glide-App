//! Easing curves.
//!
//! Every curve maps normalized time `t ∈ [0, 1]` to normalized progress
//! `[0, 1]`, with `apply(0) == 0` and `apply(1) == 1` exactly. Curves are
//! monotonically non-decreasing (ease-out shapes), so a single notch never
//! overshoots its target.

/// Decay constant of exponential smoothing: the fraction of the distance still
/// remaining after one full `animation_time` is `e^-K` (~0.7 %).
pub const SMOOTHING_K: f64 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseOutQuad,
    EaseOutCubic,
    EaseOutExpo,
    /// Default. Handled incrementally by the axis state (frame-rate independent
    /// by construction); `apply` gives the equivalent closed-form curve.
    ExponentialSmoothing,
}

impl Easing {
    pub const ALL: [Easing; 5] = [
        Easing::Linear,
        Easing::EaseOutQuad,
        Easing::EaseOutCubic,
        Easing::EaseOutExpo,
        Easing::ExponentialSmoothing,
    ];

    /// Progress in `[0, 1]` for normalized time `t` (clamped; non-finite → 1).
    pub fn apply(self, t: f64) -> f64 {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            1.0
        };
        match self {
            Easing::Linear => t,
            Easing::EaseOutQuad => {
                let u = 1.0 - t;
                1.0 - u * u
            }
            Easing::EaseOutCubic => {
                let u = 1.0 - t;
                1.0 - u * u * u
            }
            Easing::EaseOutExpo => {
                if t >= 1.0 {
                    1.0
                } else {
                    1.0 - (-10.0 * t).exp2()
                }
            }
            Easing::ExponentialSmoothing => {
                (1.0 - (-SMOOTHING_K * t).exp()) / (1.0 - (-SMOOTHING_K).exp())
            }
        }
    }

    /// Config-file name of the curve.
    pub fn name(self) -> &'static str {
        match self {
            Easing::Linear => "linear",
            Easing::EaseOutQuad => "ease_out_quad",
            Easing::EaseOutCubic => "ease_out_cubic",
            Easing::EaseOutExpo => "ease_out_expo",
            Easing::ExponentialSmoothing => "exponential_smoothing",
        }
    }

    /// Parse a config-file name (case-insensitive, `-` or `_`).
    pub fn from_name(name: &str) -> Option<Easing> {
        let norm = name.trim().to_ascii_lowercase().replace('-', "_");
        Easing::ALL.into_iter().find(|e| e.name() == norm)
    }
}
