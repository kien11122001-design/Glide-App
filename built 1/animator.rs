//! Animator: parks while idle, ticks at the display rate while there is motion.
//!
//! Safe and platform-independent. Everything OS-specific (clock, frame wait,
//! `SendInput`) sits behind [`Backend`], so the loop is tested with a virtual
//! clock on any OS.

use crate::config::{Config, FrameRate};
use crate::engine::{Engine, Output};
use crate::state::Shared;
use std::sync::Arc;

pub trait Backend {
    /// Monotonic time in microseconds.
    fn now_us(&mut self) -> u64;
    /// Display refresh rate in Hz (for `frame_rate = "auto"`).
    fn refresh_hz(&mut self) -> u32;
    /// Block until the next frame.
    fn wait_frame(&mut self, hz: u32);
    /// Inject wheel units; called at most once per frame.
    fn send(&mut self, out: Output);
    /// Free frame-timer resources while parked (NFR: nothing held when idle).
    fn release(&mut self);
}

const MIN_HZ: u32 = 30;
const MAX_HZ: u32 = 360;

fn frame_hz<B: Backend>(cfg: &Config, backend: &mut B) -> u32 {
    let hz = match cfg.frame_rate {
        FrameRate::Fixed(n) => n,
        FrameRate::Auto => backend.refresh_hz(),
    };
    hz.clamp(MIN_HZ, MAX_HZ)
}

/// Owns the engine and the config/profile it currently runs with.
struct Session {
    engine: Engine,
    cfg: Arc<Config>,
    generation: u64,
    profile: u8,
}

impl Session {
    fn new(shared: &Shared) -> Self {
        let (generation, cfg) = shared.snapshot();
        Session {
            engine: Engine::new(cfg.base.engine),
            cfg,
            generation,
            profile: 0,
        }
    }

    /// Pick up a live-reloaded config; in-flight motion continues.
    fn refresh(&mut self, shared: &Shared) {
        if shared.generation() != self.generation {
            let (generation, cfg) = shared.snapshot();
            self.generation = generation;
            self.cfg = cfg;
            self.engine
                .set_config(self.cfg.settings_for_profile(self.profile).engine);
        }
    }

    fn drain(&mut self, shared: &Shared) {
        while let Some(q) = shared.queue.pop() {
            if !shared.enabled() {
                continue;
            }
            if q.profile != self.profile {
                self.profile = q.profile;
                self.engine
                    .set_config(self.cfg.settings_for_profile(q.profile).engine);
            }
            self.engine.on_input(q.input);
        }
    }
}

/// Run frames until all motion has settled (or shutdown). Returns when idle.
fn animate_burst<B: Backend>(shared: &Shared, s: &mut Session, backend: &mut B) {
    let mut hz = frame_hz(&s.cfg, backend);
    // Emit the first frame's worth immediately: input -> motion <= 1 frame.
    let mut dt = 1.0 / f64::from(hz);
    let mut last = backend.now_us();
    loop {
        if shared.shutting_down() {
            s.engine.cancel();
            return;
        }
        s.refresh(shared);
        if shared.take_cancel() {
            s.engine.cancel();
        }
        s.drain(shared);

        let out = s.engine.tick(dt);
        if !out.is_zero() {
            backend.send(out);
        }
        if s.engine.is_idle() && shared.queue.is_empty() {
            return;
        }

        hz = frame_hz(&s.cfg, backend);
        backend.wait_frame(hz);
        let now = backend.now_us();
        // Measured dt (not a fixed step); the engine clamps stalls itself.
        dt = now.saturating_sub(last) as f64 / 1_000_000.0;
        last = now;
    }
}

/// Thread entry point. Returns only on shutdown.
pub fn run<B: Backend>(shared: &Shared, backend: &mut B) {
    shared.register_animator();
    let mut session = Session::new(shared);
    loop {
        if shared.shutting_down() {
            return;
        }
        // Apply a pending cancel here: it must not be consumed without effect.
        if shared.take_cancel() {
            session.engine.cancel();
        }
        let has_work = !shared.queue.is_empty() || !session.engine.is_idle();
        if !has_work {
            backend.release();
            // Parked: zero CPU. `wake()` (unpark) is sticky, so an event that
            // arrives just before this call is not lost. Spurious wakeups just
            // loop back here.
            std::thread::park();
            continue;
        }
        animate_burst(shared, &mut session, backend);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Axis, WheelInput};
    use crate::state::QueuedInput;

    /// Virtual clock: `wait_frame` advances time by exactly one frame.
    struct Mock {
        now: u64,
        hz: u32,
        sent: Vec<Output>,
        frames: u32,
        first_send_frame: Option<u32>,
    }

    impl Mock {
        fn new(hz: u32) -> Self {
            Mock {
                now: 0,
                hz,
                sent: Vec::new(),
                frames: 0,
                first_send_frame: None,
            }
        }
        fn total(&self) -> (i64, i64) {
            self.sent.iter().fold((0, 0), |(v, h), o| {
                (v + i64::from(o.v_units), h + i64::from(o.h_units))
            })
        }
    }

    impl Backend for Mock {
        fn now_us(&mut self) -> u64 {
            self.now
        }
        fn refresh_hz(&mut self) -> u32 {
            self.hz
        }
        fn wait_frame(&mut self, hz: u32) {
            self.now += 1_000_000 / u64::from(hz);
            self.frames += 1;
            assert!(self.frames < 100_000, "animator failed to settle");
        }
        fn send(&mut self, out: Output) {
            self.first_send_frame.get_or_insert(self.frames);
            self.sent.push(out);
        }
        fn release(&mut self) {}
    }

    fn push(shared: &Shared, delta: i32, axis: Axis, profile: u8) {
        assert!(shared.queue.push(QueuedInput {
            input: WheelInput {
                delta,
                axis,
                t_us: 0,
            },
            profile,
        }));
    }

    fn cfg_no_accel() -> Config {
        let mut c = Config::default();
        c.base.engine.accel.enabled = false;
        c
    }

    #[test]
    fn a_notch_becomes_a_smooth_stream_summing_to_the_step() {
        for hz in [60, 120, 144, 240] {
            let shared = Shared::new(cfg_no_accel());
            let mut s = Session::new(&shared);
            let mut b = Mock::new(hz);
            push(&shared, 120, Axis::Vertical, 0);
            animate_burst(&shared, &mut s, &mut b);

            let (v, h) = b.total();
            assert!((v - 120).abs() <= 1, "{hz} Hz: {v}");
            assert_eq!(h, 0);
            assert!(b.sent.len() > 5, "must be many small events, not one jump");
            assert!(
                b.sent.iter().all(|o| o.v_units.abs() < 120),
                "each frame is a fraction of a notch"
            );
            assert!(s.engine.is_idle());
        }
    }

    #[test]
    fn first_output_arrives_in_the_first_frame() {
        let shared = Shared::new(cfg_no_accel());
        let mut s = Session::new(&shared);
        let mut b = Mock::new(120);
        push(&shared, 120, Axis::Vertical, 0);
        animate_burst(&shared, &mut s, &mut b);
        assert!(b.sent[0].v_units > 0);
        assert_eq!(
            b.first_send_frame,
            Some(0),
            "first motion is emitted before any frame wait"
        );
    }

    #[test]
    fn settles_and_stops_ticking() {
        let shared = Shared::new(cfg_no_accel());
        let mut s = Session::new(&shared);
        let mut b = Mock::new(60);
        push(&shared, 120, Axis::Vertical, 0);
        animate_burst(&shared, &mut s, &mut b);
        let frames = b.frames;
        assert!(frames < 40, "ended after ~1/3 s, not spinning: {frames}");
        // nothing pending: a burst returns immediately without emitting
        let before = b.sent.len();
        animate_burst(&shared, &mut s, &mut b);
        assert_eq!(b.sent.len(), before);
    }

    #[test]
    fn cancel_and_disable_stop_motion() {
        let shared = Shared::new(cfg_no_accel());
        let mut s = Session::new(&shared);
        let mut b = Mock::new(60);
        push(&shared, 120, Axis::Vertical, 0);
        // one frame, then the user disables
        s.drain(&shared);
        let first = s.engine.tick(1.0 / 60.0);
        assert!(!first.is_zero());
        shared.set_enabled(false);
        animate_burst(&shared, &mut s, &mut b);
        assert!(b.sent.is_empty(), "cancelled: nothing more emitted");
        assert!(s.engine.is_idle());
    }

    #[test]
    fn input_ignored_while_disabled() {
        let shared = Shared::new(cfg_no_accel());
        shared.set_enabled(false);
        let _ = shared.take_cancel();
        let mut s = Session::new(&shared);
        let mut b = Mock::new(60);
        push(&shared, 120, Axis::Vertical, 0);
        animate_burst(&shared, &mut s, &mut b);
        assert!(b.sent.is_empty());
    }

    #[test]
    fn live_reload_applies_mid_motion() {
        let shared = Shared::new(cfg_no_accel());
        let mut s = Session::new(&shared);
        let mut b = Mock::new(60);
        push(&shared, 120, Axis::Vertical, 0);
        s.drain(&shared);
        let first = s.engine.tick(1.0 / 60.0);

        let mut next = cfg_no_accel();
        next.base.engine.animation_time_ms = 100.0;
        shared.store_config(next);
        animate_burst(&shared, &mut s, &mut b);

        let total = i64::from(first.v_units) + b.total().0;
        assert!((total - 120).abs() <= 1, "no jump or loss on reload: {total}");
        assert!(b.frames < 15, "faster settle with the new 100 ms config");
    }

    #[test]
    fn per_app_profile_changes_the_step() {
        let mut cfg = cfg_no_accel();
        let mut big = cfg.base;
        big.engine.step_size = 240.0;
        cfg.overrides.push(("big.exe".to_owned(), big));
        let shared = Shared::new(cfg);
        let mut s = Session::new(&shared);
        let mut b = Mock::new(60);
        push(&shared, 120, Axis::Vertical, 1);
        animate_burst(&shared, &mut s, &mut b);
        assert!((b.total().0 - 240).abs() <= 1);
    }

    #[test]
    fn horizontal_and_vertical_together() {
        let shared = Shared::new(cfg_no_accel());
        let mut s = Session::new(&shared);
        let mut b = Mock::new(144);
        push(&shared, 120, Axis::Vertical, 0);
        push(&shared, -240, Axis::Horizontal, 0);
        animate_burst(&shared, &mut s, &mut b);
        let (v, h) = b.total();
        assert!((v - 120).abs() <= 1 && (h + 240).abs() <= 1, "{v} {h}");
        assert!(b.sent.iter().any(|o| o.v_units != 0 && o.h_units != 0));
    }

    #[test]
    fn fixed_frame_rate_is_respected() {
        let mut cfg = cfg_no_accel();
        cfg.frame_rate = FrameRate::Fixed(30);
        let shared = Shared::new(cfg);
        let mut s = Session::new(&shared);
        let mut b = Mock::new(240);
        push(&shared, 120, Axis::Vertical, 0);
        animate_burst(&shared, &mut s, &mut b);
        assert!(b.frames <= 15, "30 Hz -> few frames, got {}", b.frames);
    }

    #[test]
    fn shutdown_exits_the_thread_loop() {
        let shared = Shared::new(cfg_no_accel());
        shared.request_shutdown();
        let mut b = Mock::new(60);
        run(&shared, &mut b); // must return immediately
        assert!(b.sent.is_empty());
    }
}
