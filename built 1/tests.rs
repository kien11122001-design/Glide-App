use super::*;

fn plain() -> EngineConfig {
    let d = EngineConfig::default();
    EngineConfig {
        accel: AccelConfig {
            enabled: false,
            ..d.accel
        },
        ..d
    }
}

fn with_easing(easing: Easing) -> EngineConfig {
    EngineConfig {
        easing,
        ..plain()
    }
}

fn momentum() -> EngineConfig {
    let d = plain();
    EngineConfig {
        momentum: MomentumConfig {
            enabled: true,
            ..d.momentum
        },
        ..d
    }
}

fn notch(delta: i32, axis: Axis, t_us: u64) -> WheelInput {
    WheelInput { delta, axis, t_us }
}

struct Run {
    v: i64,
    h: i64,
    secs: f64,
    min_v: i32,
    max_v: i32,
}

/// Tick at a fixed `dt` until the engine parks (10 s safety cap).
fn run_until_idle(e: &mut Engine, dt: f64) -> Run {
    run_until_idle_for(e, dt, 10.0)
}

/// Like [`run_until_idle`] with an explicit safety cap in seconds (a very
/// high momentum `friction` legitimately glides for tens of seconds).
fn run_until_idle_for(e: &mut Engine, dt: f64, cap_secs: f64) -> Run {
    let mut r = Run {
        v: 0,
        h: 0,
        secs: 0.0,
        min_v: 0,
        max_v: 0,
    };
    let max_ticks = (cap_secs / dt) as usize;
    for _ in 0..max_ticks {
        if e.is_idle() {
            break;
        }
        let o = e.tick(dt);
        r.v += i64::from(o.v_units);
        r.h += i64::from(o.h_units);
        r.min_v = r.min_v.min(o.v_units);
        r.max_v = r.max_v.max(o.v_units);
        r.secs += dt;
    }
    r
}

/// Deterministic PRNG so the tests need no external crate.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn unit(&mut self) -> f64 {
        (self.next() % 10_000) as f64 / 10_000.0
    }
}

#[test]
fn easing_endpoints_and_monotonic() {
    for e in Easing::ALL {
        assert!(e.apply(0.0).abs() < 1e-12, "{e:?} start");
        assert!((e.apply(1.0) - 1.0).abs() < 1e-12, "{e:?} end");
        let mut prev = 0.0;
        for i in 0..=1000 {
            let y = e.apply(f64::from(i) / 1000.0);
            assert!(y >= prev - 1e-12, "{e:?} not monotonic at {i}");
            assert!(y <= 1.0 + 1e-12, "{e:?} overshoot at {i}");
            prev = y;
        }
        assert_eq!(e.apply(f64::NAN), 1.0);
    }
}

#[test]
fn easing_names_round_trip() {
    for e in Easing::ALL {
        assert_eq!(Easing::from_name(e.name()), Some(e));
    }
    assert_eq!(Easing::from_name(" Ease-Out-Cubic "), Some(Easing::EaseOutCubic));
    assert_eq!(Easing::from_name("bogus"), None);
}

#[test]
fn notches_accumulate_into_step_size() {
    for easing in Easing::ALL {
        let mut e = Engine::new(with_easing(easing));
        for i in 0..5 {
            e.on_input(notch(120, Axis::Vertical, i * 10_000));
        }
        let r = run_until_idle(&mut e, 1.0 / 60.0);
        assert!((r.v - 600).abs() <= 1, "{easing:?}: total {}", r.v);
        assert_eq!(r.h, 0);
    }
}

#[test]
fn every_easing_settles_near_animation_time() {
    for easing in Easing::ALL {
        let mut e = Engine::new(with_easing(easing));
        e.on_input(notch(120, Axis::Vertical, 0));
        let r = run_until_idle(&mut e, 1.0 / 120.0);
        assert!((r.v - 120).abs() <= 1, "{easing:?}: total {}", r.v);
        assert!(
            r.secs > 0.2 && r.secs < 0.5,
            "{easing:?}: settle time {}",
            r.secs
        );
        assert!(r.min_v >= 0, "{easing:?} produced a negative step");
    }
}

#[test]
fn frame_rate_independent() {
    let mut first: Option<f64> = None;
    for hz in [60.0, 120.0, 144.0, 240.0] {
        let mut e = Engine::new(plain());
        e.on_input(notch(120, Axis::Vertical, 0));
        let r = run_until_idle(&mut e, 1.0 / hz);
        assert!((r.v - 120).abs() <= 1, "{hz} Hz: total {}", r.v);
        let t0 = *first.get_or_insert(r.secs);
        assert!((r.secs - t0).abs() < 0.03, "{hz} Hz: settle {} vs {t0}", r.secs);
    }
}

#[test]
fn position_curve_matches_across_rates() {
    // After exactly 100 ms of simulated time, distance covered must agree.
    let mut at_100ms = Vec::new();
    for ticks_per_100ms in [6_u32, 12, 24] {
        let mut e = Engine::new(plain());
        e.on_input(notch(120, Axis::Vertical, 0));
        let dt = 0.1 / f64::from(ticks_per_100ms);
        let mut sum = 0_i64;
        for _ in 0..ticks_per_100ms {
            sum += i64::from(e.tick(dt).v_units);
        }
        at_100ms.push(sum);
    }
    let lo = *at_100ms.iter().min().unwrap_or(&0);
    let hi = *at_100ms.iter().max().unwrap_or(&0);
    assert!(hi - lo <= 2, "curves diverge: {at_100ms:?}");
}

#[test]
fn no_drift_over_ten_thousand_ticks() {
    let mut e = Engine::new(plain());
    let mut rng = Lcg(0x5EED);
    let (mut total, mut sent, mut t_us) = (0_i64, 0_i64, 0_u64);
    for _ in 0..10_000 {
        let dt = 0.004 + rng.unit() * 0.02;
        t_us += (dt * 1e6) as u64;
        if rng.next() % 40 == 0 {
            e.on_input(notch(120, Axis::Vertical, t_us));
            sent += 1;
        }
        total += i64::from(e.tick(dt).v_units);
    }
    total += run_until_idle(&mut e, 1.0 / 60.0).v;
    assert!(sent > 100, "test must exercise many notches");
    assert!((total - sent * 120).abs() <= 1, "drift: {total} vs {}", sent * 120);
}

#[test]
fn stays_idle_without_input() {
    let mut e = Engine::new(plain());
    assert!(e.is_idle());
    for _ in 0..100 {
        assert!(e.tick(1.0 / 60.0).is_zero());
    }
}

#[test]
fn acceleration_grows_caps_and_resets() {
    let mut e = Engine::new(EngineConfig::default());
    assert!((e.accel_multiplier(Axis::Vertical) - 1.0).abs() < 1e-9);

    e.on_input(notch(120, Axis::Vertical, 0));
    assert!((e.accel_multiplier(Axis::Vertical) - 1.0).abs() < 1e-9);
    e.on_input(notch(120, Axis::Vertical, 50_000));
    assert!((e.accel_multiplier(Axis::Vertical) - 1.5).abs() < 1e-9);

    for i in 2..20 {
        e.on_input(notch(120, Axis::Vertical, i * 50_000));
    }
    assert!((e.accel_multiplier(Axis::Vertical) - 4.0).abs() < 1e-9, "cap");

    // A long pause (> 2 x window) drops back to 1x.
    e.on_input(notch(120, Axis::Vertical, 19 * 50_000 + 500_000));
    assert!((e.accel_multiplier(Axis::Vertical) - 1.0).abs() < 1e-9);
}

#[test]
fn acceleration_scales_distance() {
    let mut slow = Engine::new(EngineConfig::default());
    let mut fast = Engine::new(EngineConfig::default());
    for i in 0..4 {
        slow.on_input(notch(120, Axis::Vertical, i * 1_000_000));
        fast.on_input(notch(120, Axis::Vertical, i * 30_000));
    }
    let s = run_until_idle(&mut slow, 1.0 / 60.0).v;
    let f = run_until_idle(&mut fast, 1.0 / 60.0).v;
    assert_eq!(s, 480, "slow notches are not accelerated");
    // 1.0 + 1.5 + 2.0 + 2.5 = 7.0 steps
    assert!((f - 840).abs() <= 1, "fast total {f}");
}

#[test]
fn reversal_cancels_and_reverses_immediately() {
    for cfg in [plain(), with_easing(Easing::EaseOutCubic), momentum()] {
        let mut e = Engine::new(cfg);
        e.on_input(notch(120, Axis::Vertical, 0));
        for _ in 0..3 {
            e.tick(1.0 / 60.0);
        }
        e.on_input(notch(-120, Axis::Vertical, 60_000));

        let first = e.tick(1.0 / 60.0);
        assert!(first.v_units < 0, "immediate opposite motion: {first:?}");

        let r = run_until_idle(&mut e, 1.0 / 60.0);
        assert!(r.max_v <= 0, "no residual forward motion after reversal");
        assert!(
            (i64::from(first.v_units) + r.v + 120).abs() <= 1,
            "reversal total {}",
            i64::from(first.v_units) + r.v
        );
    }
}

#[test]
fn reverse_direction_flips_sign() {
    let mut e = Engine::new(EngineConfig {
        reverse_direction: true,
        ..plain()
    });
    e.on_input(notch(120, Axis::Vertical, 0));
    let r = run_until_idle(&mut e, 1.0 / 60.0);
    assert!((r.v + 120).abs() <= 1);
    assert!(r.max_v <= 0);
}

#[test]
fn axes_are_independent() {
    let mut e = Engine::new(plain());
    e.on_input(notch(120, Axis::Horizontal, 0));
    let r = run_until_idle(&mut e, 1.0 / 60.0);
    assert_eq!(r.v, 0);
    assert!((r.h - 120).abs() <= 1);
}

#[test]
fn momentum_decays_in_bounded_time_without_overshoot() {
    for hz in [60.0, 144.0, 240.0] {
        let mut e = Engine::new(momentum());
        e.on_input(notch(120, Axis::Vertical, 0));
        let r = run_until_idle(&mut e, 1.0 / hz);
        assert!(e.is_idle());
        assert!(r.secs < 3.0, "{hz} Hz: took {} s", r.secs);
        assert!(r.min_v >= 0, "{hz} Hz: negative overshoot");
        assert!((r.v - 120).abs() <= 1, "{hz} Hz: total {}", r.v);
    }
}

#[test]
fn small_wheel_units_never_lost() {
    // High-resolution wheel: 30 units per event, 4 events = 1 notch.
    let mut e = Engine::new(plain());
    for i in 0..4 {
        e.on_input(notch(30, Axis::Vertical, i * 8_000));
    }
    let r = run_until_idle(&mut e, 1.0 / 60.0);
    assert!((r.v - 120).abs() <= 1, "total {}", r.v);
}

#[test]
fn huge_dt_is_clamped() {
    let mut a = Engine::new(plain());
    let mut b = Engine::new(plain());
    a.on_input(notch(120, Axis::Vertical, 0));
    b.on_input(notch(120, Axis::Vertical, 0));
    assert_eq!(a.tick(10.0), b.tick(MAX_DT_S));
    assert_eq!(a.tick(-5.0), Output::default());
}

#[test]
fn non_finite_dt_is_harmless() {
    let mut e = Engine::new(plain());
    e.on_input(notch(120, Axis::Vertical, 0));
    assert!(e.tick(f64::NAN).is_zero());
    assert!(e.tick(f64::INFINITY).is_zero());
    assert!(e.tick(1.0 / 60.0).v_units > 0);
}

#[test]
fn extreme_input_saturates_instead_of_overflowing() {
    let mut e = Engine::new(plain());
    for i in 0..1000 {
        e.on_input(notch(i32::MAX, Axis::Vertical, i));
        e.on_input(notch(i32::MAX, Axis::Horizontal, i));
    }
    let o = e.tick(MAX_DT_S);
    assert!(o.v_units > 0 && o.h_units > 0);
    e.cancel();
    assert!(e.is_idle());
}

#[test]
fn cancel_drops_motion() {
    let mut e = Engine::new(plain());
    e.on_input(notch(120, Axis::Vertical, 0));
    e.tick(1.0 / 60.0);
    e.cancel();
    assert!(e.is_idle());
    assert!(e.tick(1.0 / 60.0).is_zero());
}

#[test]
fn config_swap_keeps_motion_going() {
    let mut e = Engine::new(plain());
    e.on_input(notch(120, Axis::Vertical, 0));
    let mut total = i64::from(e.tick(1.0 / 60.0).v_units);
    e.set_config(EngineConfig {
        animation_time_ms: 100.0,
        ..plain()
    });
    total += run_until_idle(&mut e, 1.0 / 60.0).v;
    assert!((total - 120).abs() <= 1, "total {total}");
}

#[test]
fn sanitize_clamps_and_repairs() {
    let bad = EngineConfig {
        step_size: f64::NAN,
        animation_time_ms: -5.0,
        px_per_notch: 0.0,
        accel: AccelConfig {
            enabled: true,
            max_multiplier: f64::INFINITY,
            window_ms: 0.0,
        },
        momentum: MomentumConfig {
            enabled: true,
            friction: 5.0,
            min_velocity: -1.0,
        },
        ..EngineConfig::default()
    }
    .sanitized();
    assert_eq!(bad.step_size, 120.0);
    assert_eq!(bad.animation_time_ms, 1.0);
    assert_eq!(bad.px_per_notch, 1.0);
    assert_eq!(bad.accel.max_multiplier, 1.0);
    assert_eq!(bad.accel.window_ms, 1.0);
    assert_eq!(bad.momentum.friction, 0.999);
    assert!(bad.momentum.min_velocity > 0.0);
}

// ---------------------------------------------------------------------------
// momentum.min_velocity (was accepted by the config but ignored by the engine)
// ---------------------------------------------------------------------------

fn momentum_with_min_velocity(v: f64) -> EngineConfig {
    let m = momentum();
    EngineConfig {
        momentum: MomentumConfig {
            min_velocity: v,
            ..m.momentum
        },
        ..m
    }
}

#[test]
fn min_velocity_ends_momentum_sooner_without_losing_distance() {
    let mut fine = Engine::new(momentum_with_min_velocity(0.5));
    let mut coarse = Engine::new(momentum_with_min_velocity(400.0));
    for e in [&mut fine, &mut coarse] {
        e.on_input(notch(120, Axis::Vertical, 0));
    }
    let a = run_until_idle(&mut fine, 1.0 / 60.0);
    let b = run_until_idle(&mut coarse, 1.0 / 60.0);
    assert!(
        b.secs < a.secs,
        "higher threshold stops earlier: {} vs {}",
        b.secs,
        a.secs
    );
    assert!((a.v - 120).abs() <= 1 && (b.v - 120).abs() <= 1, "{} {}", a.v, b.v);
    assert!(a.min_v >= 0 && b.min_v >= 0);
}

#[test]
fn huge_min_velocity_cannot_turn_the_tail_into_a_big_jump() {
    let mut e = Engine::new(momentum_with_min_velocity(1.0e6));
    e.on_input(notch(120, Axis::Vertical, 0));
    let first = e.tick(1.0 / 60.0).v_units;
    let r = run_until_idle(&mut e, 1.0 / 60.0);
    assert!(
        (i64::from(first) + r.v - 120).abs() <= 1,
        "total {}",
        i64::from(first) + r.v
    );
    // The tail flush is capped (MAX_STOP_PX), so no single frame carries
    // more than the first, natural frame does.
    assert!(r.max_v <= first, "max {} first {first}", r.max_v);
}

#[test]
fn very_high_friction_still_settles_and_conserves_distance() {
    let mut cfg = momentum();
    cfg.momentum.friction = 0.999;
    let mut e = Engine::new(cfg);
    e.on_input(notch(120, Axis::Vertical, 0));
    let r = run_until_idle_for(&mut e, 1.0 / 30.0, 600.0);
    assert!(e.is_idle(), "settled after {} s", r.secs);
    assert!((r.v - 120).abs() <= 1, "total {}", r.v);
    assert!(r.min_v >= 0);
}

// ---------------------------------------------------------------------------
// Live reload can switch the accumulation model while motion is in flight.
// ---------------------------------------------------------------------------

#[test]
fn switching_model_mid_motion_neither_strands_nor_loses_distance() {
    let models = [
        with_easing(Easing::Linear),
        with_easing(Easing::EaseOutQuad),
        with_easing(Easing::EaseOutCubic),
        with_easing(Easing::EaseOutExpo),
        with_easing(Easing::ExponentialSmoothing),
        momentum(),
    ];
    for (i, from) in models.iter().enumerate() {
        for (j, to) in models.iter().enumerate() {
            let mut e = Engine::new(*from);
            for k in 0..3 {
                e.on_input(notch(120, Axis::Vertical, k * 20_000));
            }
            let mut total = 0_i64;
            for _ in 0..4 {
                total += i64::from(e.tick(1.0 / 60.0).v_units);
            }
            e.set_config(*to);
            e.on_input(notch(120, Axis::Vertical, 200_000));
            let r = run_until_idle_for(&mut e, 1.0 / 60.0, 60.0);
            total += r.v;
            assert!(e.is_idle(), "{i}->{j}: stranded motion");
            assert!((total - 480).abs() <= 1, "{i}->{j}: total {total}");
        }
    }
}

#[test]
fn reversal_right_after_a_model_switch_cancels_the_old_motion() {
    let mut e = Engine::new(with_easing(Easing::EaseOutCubic));
    e.on_input(notch(120, Axis::Vertical, 0));
    e.tick(1.0 / 60.0);
    e.set_config(plain());
    e.on_input(notch(-120, Axis::Vertical, 50_000));
    let first = e.tick(1.0 / 60.0).v_units;
    assert!(first < 0, "immediate opposite motion");
    let r = run_until_idle(&mut e, 1.0 / 60.0);
    assert!(r.max_v <= 0, "no forward residue after the switch");
    assert!((i64::from(first) + r.v + 120).abs() <= 1);
}

// ---------------------------------------------------------------------------
// 2.9 hardening: randomized sequences over every mode
// ---------------------------------------------------------------------------

/// `max_friction` bounds momentum glide time so the fixed 10 s test cap stays
/// meaningful (very high friction has its own dedicated test above).
fn random_config(rng: &mut Lcg, max_friction: f64) -> EngineConfig {
    let easing = Easing::ALL[(rng.next() % 5) as usize];
    let use_momentum = rng.next() % 4 == 0;
    let mut cfg = EngineConfig {
        step_size: 20.0 + rng.unit() * 980.0,
        animation_time_ms: 50.0 + rng.unit() * 1950.0,
        easing,
        accel: AccelConfig {
            enabled: rng.next() % 2 == 0,
            max_multiplier: 1.0 + rng.unit() * 9.0,
            window_ms: 20.0 + rng.unit() * 480.0,
        },
        momentum: MomentumConfig {
            enabled: use_momentum,
            friction: 0.5 + rng.unit() * 0.499,
            min_velocity: 0.001 + rng.unit() * 50.0,
        },
        reverse_direction: rng.next() % 2 == 0,
        px_per_notch: 60.0 + rng.unit() * 180.0,
    };
    cfg.momentum.friction = cfg.momentum.friction.min(max_friction);
    cfg
}

#[test]
fn fuzz_random_traffic_never_panics_and_always_settles() {
    let mut rng = Lcg(0xC0FF_EE11);
    for case in 0..300 {
        let mut e = Engine::new(random_config(&mut rng, 0.95));
        let mut t_us = 0_u64;
        for _ in 0..200 {
            // Interleave inputs (both axes, both signs, sometimes non-multiples
            // of 120 or zero), config swaps and ticks with awkward `dt`s.
            match rng.next() % 6 {
                0 | 1 => {
                    let delta = match rng.next() % 5 {
                        0 => 0,
                        1 => 1 + (rng.next() % 119) as i32,
                        2 => -120 * (1 + (rng.next() % 4) as i32),
                        _ => 120 * (1 + (rng.next() % 4) as i32),
                    };
                    let axis = if rng.next() % 3 == 0 {
                        Axis::Horizontal
                    } else {
                        Axis::Vertical
                    };
                    t_us += (rng.unit() * 200_000.0) as u64;
                    e.on_input(notch(delta, axis, t_us));
                }
                2 => e.set_config(random_config(&mut rng, 0.95)),
                3 => {
                    let dt = match rng.next() % 4 {
                        0 => f64::NAN,
                        1 => -1.0,
                        2 => 50.0,
                        _ => 0.0,
                    };
                    let _ = e.tick(dt);
                }
                _ => {
                    let _ = e.tick(0.001 + rng.unit() * 0.03);
                }
            }
        }
        let r = run_until_idle(&mut e, 1.0 / 120.0);
        assert!(e.is_idle(), "case {case}: did not settle");
        assert!(r.secs < 10.0, "case {case}: took {} s", r.secs);
    }
}

#[test]
fn fuzz_distance_is_conserved_for_same_direction_notches() {
    // Without reversal or acceleration, N notches must travel exactly
    // N * step_size px-equivalent, whatever the mode/timing/frame rate.
    let mut rng = Lcg(0xD15_7A2C);
    for case in 0..200 {
        let mut cfg = random_config(&mut rng, 0.999);
        cfg.accel.enabled = false;
        cfg.reverse_direction = false;
        let cfg = cfg.sanitized();
        let mut e = Engine::new(cfg);

        let notches = 1 + (rng.next() % 8) as i64;
        let mut total = 0_i64;
        let mut t_us = 0_u64;
        let mut sent = 0_i64;
        while sent < notches {
            if rng.next() % 3 == 0 {
                t_us += 10_000;
                e.on_input(notch(120, Axis::Vertical, t_us));
                sent += 1;
            }
            total += i64::from(e.tick(0.002 + rng.unit() * 0.02).v_units);
        }
        // Friction up to 0.999 may glide for tens of seconds: generous cap.
        total += run_until_idle_for(&mut e, 1.0 / 90.0, 600.0).v;

        let expected = notches as f64 * cfg.step_size * 120.0 / cfg.px_per_notch;
        assert!(
            (total as f64 - expected).abs() <= 1.5,
            "case {case} ({:?}, momentum {}): {total} vs {expected}",
            cfg.easing,
            cfg.momentum.enabled
        );
    }
}
