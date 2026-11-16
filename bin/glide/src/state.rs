//! State shared between the hook thread, the animator thread and the UI thread.
//!
//! Entirely safe Rust. The hook -> animator path never allocates and never
//! blocks: a bounded single-producer/single-consumer ring of atomics plus
//! `Thread::unpark`.

use crate::config::Config;
use crate::engine::{Axis, WheelInput};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::Thread;

/// A wheel event plus the per-app profile it should be animated with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueuedInput {
    pub input: WheelInput,
    /// 0 = base settings, n = per-app override n-1 (see `Config::profile_for`).
    pub profile: u8,
}

const CAPACITY: usize = 256;

/// Bounded SPSC queue. Each slot is two atomics (packed event + timestamp);
/// `head`/`tail` publish with Release/Acquire so the consumer never observes a
/// half-written slot. Exactly one producer thread and one consumer thread.
pub struct InputQueue {
    slots: Box<[[AtomicU64; 2]]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

fn pack(q: QueuedInput) -> u64 {
    let axis = u64::from(q.input.axis == Axis::Horizontal);
    // `as u32` reinterprets the signed delta bit-for-bit.
    u64::from(q.input.delta as u32) | (axis << 32) | (u64::from(q.profile) << 40)
}

fn unpack(word: u64, t_us: u64) -> QueuedInput {
    QueuedInput {
        input: WheelInput {
            delta: (word & 0xFFFF_FFFF) as u32 as i32,
            axis: if (word >> 32) & 1 == 1 {
                Axis::Horizontal
            } else {
                Axis::Vertical
            },
            t_us,
        },
        profile: ((word >> 40) & 0xFF) as u8,
    }
}

impl InputQueue {
    pub fn new() -> Self {
        InputQueue {
            slots: (0..CAPACITY)
                .map(|_| [AtomicU64::new(0), AtomicU64::new(0)])
                .collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Producer side. Returns `false` (event not stored) when full.
    pub fn push(&self, q: QueuedInput) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= CAPACITY {
            return false;
        }
        let slot = &self.slots[head % CAPACITY];
        slot[0].store(pack(q), Ordering::Relaxed);
        slot[1].store(q.input.t_us, Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// Consumer side.
    pub fn pop(&self) -> Option<QueuedInput> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        let slot = &self.slots[tail % CAPACITY];
        let q = unpack(
            slot[0].load(Ordering::Relaxed),
            slot[1].load(Ordering::Relaxed),
        );
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(q)
    }

    pub fn is_empty(&self) -> bool {
        self.tail.load(Ordering::Acquire) == self.head.load(Ordering::Acquire)
    }
}

impl Default for InputQueue {
    fn default() -> Self {
        Self::new()
    }
}

fn uses_momentum(config: &Config) -> bool {
    config.base.engine.momentum.enabled
        || config
            .overrides
            .iter()
            .any(|(_, s)| s.engine.momentum.enabled)
}

pub struct Shared {
    pub queue: InputQueue,
    enabled: AtomicBool,
    shutdown: AtomicBool,
    cancel: AtomicBool,
    fullscreen: AtomicBool,
    /// True when any profile uses momentum: the hook then cancels inertia on a
    /// mouse-button press (7.1) without having to inspect the config.
    momentum_any: AtomicBool,
    generation: AtomicU64,
    config: Mutex<Arc<Config>>,
    animator: OnceLock<Thread>,
}

impl Shared {
    pub fn new(config: Config) -> Arc<Shared> {
        Arc::new(Shared {
            queue: InputQueue::new(),
            enabled: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            fullscreen: AtomicBool::new(false),
            momentum_any: AtomicBool::new(uses_momentum(&config)),
            generation: AtomicU64::new(1),
            config: Mutex::new(Arc::new(config)),
            animator: OnceLock::new(),
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Disabling also cancels any motion in flight (never leave it scrolling).
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        if !on {
            self.request_cancel();
        }
    }

    pub fn toggle_enabled(&self) -> bool {
        let now = !self.enabled();
        self.set_enabled(now);
        now
    }

    pub fn shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.wake();
    }

    /// Ask the animator to drop all remaining motion (disable, click, quit).
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.wake();
    }

    pub fn take_cancel(&self) -> bool {
        self.cancel.swap(false, Ordering::Relaxed)
    }

    /// Whether a mouse click should cancel in-flight inertia.
    pub fn cancel_on_click(&self) -> bool {
        self.momentum_any.load(Ordering::Relaxed)
    }

    pub fn fullscreen(&self) -> bool {
        self.fullscreen.load(Ordering::Relaxed)
    }

    pub fn set_fullscreen(&self, v: bool) {
        self.fullscreen.store(v, Ordering::Relaxed);
    }

    /// Cheap change counter: readers re-fetch the snapshot only when it moves.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn snapshot(&self) -> (u64, Arc<Config>) {
        let guard = self.config.lock().unwrap_or_else(PoisonError::into_inner);
        (self.generation(), Arc::clone(&guard))
    }

    /// Atomically publish a new config (live reload).
    pub fn store_config(&self, config: Config) {
        let mut guard = self.config.lock().unwrap_or_else(PoisonError::into_inner);
        self.momentum_any
            .store(uses_momentum(&config), Ordering::Relaxed);
        *guard = Arc::new(config);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Called once by the animator thread so others can wake it.
    pub fn register_animator(&self) {
        // A second registration is ignored; there is only one animator.
        let _ = self.animator.set(std::thread::current());
    }

    /// Lock-free wake of the parked animator (safe to call from the hook).
    pub fn wake(&self) {
        if let Some(t) = self.animator.get() {
            t.unpark();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(delta: i32, axis: Axis, profile: u8, t: u64) -> QueuedInput {
        QueuedInput {
            input: WheelInput {
                delta,
                axis,
                t_us: t,
            },
            profile,
        }
    }

    #[test]
    fn queue_round_trips_every_field() {
        let queue = InputQueue::new();
        let items = [
            q(120, Axis::Vertical, 0, 1),
            q(-120, Axis::Horizontal, 7, u64::MAX),
            q(i32::MIN, Axis::Vertical, 255, 42),
            q(i32::MAX, Axis::Horizontal, 1, 0),
        ];
        for it in items {
            assert!(queue.push(it));
        }
        for it in items {
            assert_eq!(queue.pop(), Some(it));
        }
        assert_eq!(queue.pop(), None);
        assert!(queue.is_empty());
    }

    #[test]
    fn queue_is_bounded_and_reusable() {
        let queue = InputQueue::new();
        for i in 0..CAPACITY {
            assert!(queue.push(q(120, Axis::Vertical, 0, i as u64)));
        }
        assert!(
            !queue.push(q(120, Axis::Vertical, 0, 0)),
            "full -> rejected"
        );
        assert_eq!(queue.pop().map(|x| x.input.t_us), Some(0));
        assert!(queue.push(q(120, Axis::Vertical, 0, 999)), "space freed");
        // wrap around several times
        for round in 0..5_u64 {
            while queue.pop().is_some() {}
            for i in 0..CAPACITY as u64 {
                assert!(queue.push(q(1, Axis::Vertical, 0, round * 1000 + i)));
            }
        }
    }

    #[test]
    fn queue_across_threads_preserves_order() {
        let queue = Arc::new(InputQueue::new());
        let producer = {
            let queue = Arc::clone(&queue);
            std::thread::spawn(move || {
                for i in 0..20_000_u64 {
                    while !queue.push(q(120, Axis::Vertical, 0, i)) {
                        std::thread::yield_now();
                    }
                }
            })
        };
        let mut next = 0_u64;
        while next < 20_000 {
            if let Some(item) = queue.pop() {
                assert_eq!(item.input.t_us, next);
                next += 1;
            } else {
                std::thread::yield_now();
            }
        }
        producer.join().unwrap();
    }

    #[test]
    fn config_snapshot_and_generation() {
        let shared = Shared::new(Config::default());
        let (g0, c0) = shared.snapshot();
        let mut next = Config::default();
        next.base.engine.step_size = 77.0;
        shared.store_config(next);
        let (g1, c1) = shared.snapshot();
        assert!(g1 > g0);
        assert_eq!(c0.base.engine.step_size, 120.0, "old snapshot unchanged");
        assert_eq!(c1.base.engine.step_size, 77.0);
    }

    #[test]
    fn cancel_on_click_tracks_momentum_in_any_profile() {
        let shared = Shared::new(Config::default());
        assert!(!shared.cancel_on_click());

        let mut with_momentum = Config::default();
        with_momentum.base.engine.momentum.enabled = true;
        shared.store_config(with_momentum);
        assert!(shared.cancel_on_click());

        let mut per_app = Config::default();
        let mut s = per_app.base;
        s.engine.momentum.enabled = true;
        per_app.overrides.push(("x.exe".to_owned(), s));
        shared.store_config(per_app);
        assert!(shared.cancel_on_click(), "override-only momentum counts");

        shared.store_config(Config::default());
        assert!(!shared.cancel_on_click());
    }

    #[test]
    fn disable_requests_cancel() {
        let shared = Shared::new(Config::default());
        assert!(shared.enabled());
        assert!(!shared.take_cancel());
        assert!(!shared.toggle_enabled());
        assert!(shared.take_cancel());
        assert!(!shared.take_cancel(), "consumed once");
        assert!(shared.toggle_enabled());
    }
}
