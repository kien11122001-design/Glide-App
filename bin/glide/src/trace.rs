//! Developer aid (`--trace <file>`, 7.2): records every scroll event the
//! animator emits as CSV so a curve can be plotted and tuned. Never enabled by
//! default; nothing is logged unless the flag is given. Wraps any
//! [`Backend`], so it is testable with a virtual clock on any OS.
//!
//! Format: `t_ms,v_units,h_units`, one row per injected frame, with `t_ms`
//! measured from the first event of the run (so it stays small and stable).
//! No pointer position or application content is ever recorded (PRD privacy).

use crate::animator::Backend;
use crate::engine::Output;
use std::io::Write;

pub const HEADER: &str = "t_ms,v_units,h_units";

pub struct Tracing<B, W: Write> {
    inner: B,
    out: W,
    origin_us: Option<u64>,
    wrote_header: bool,
}

impl<B: Backend, W: Write> Tracing<B, W> {
    pub fn new(inner: B, out: W) -> Self {
        Tracing {
            inner,
            out,
            origin_us: None,
            wrote_header: false,
        }
    }

    /// Trace output is best-effort: a full disk must never disturb scrolling.
    fn line(&mut self, text: &str) {
        let _ = writeln!(self.out, "{text}");
    }
}

impl<B: Backend, W: Write> Backend for Tracing<B, W> {
    fn now_us(&mut self) -> u64 {
        self.inner.now_us()
    }

    fn refresh_hz(&mut self) -> u32 {
        self.inner.refresh_hz()
    }

    fn wait_frame(&mut self, hz: u32) {
        self.inner.wait_frame(hz);
    }

    fn send(&mut self, out: Output) {
        if !self.wrote_header {
            self.wrote_header = true;
            self.line(HEADER);
        }
        let now = self.inner.now_us();
        let origin = *self.origin_us.get_or_insert(now);
        let t_ms = now.saturating_sub(origin) as f64 / 1000.0;
        self.line(&format!("{t_ms:.3},{},{}", out.v_units, out.h_units));
        self.inner.send(out);
    }

    fn release(&mut self) {
        // Parking between bursts: make the file complete and start a new
        // time origin for the next burst.
        let _ = self.out.flush();
        self.origin_us = None;
        self.inner.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        now: u64,
        sent: Vec<Output>,
        released: u32,
    }

    impl Backend for Fake {
        fn now_us(&mut self) -> u64 {
            self.now
        }
        fn refresh_hz(&mut self) -> u32 {
            60
        }
        fn wait_frame(&mut self, hz: u32) {
            self.now += 1_000_000 / u64::from(hz);
        }
        fn send(&mut self, out: Output) {
            self.sent.push(out);
        }
        fn release(&mut self) {
            self.released += 1;
        }
    }

    fn csv(t: &Tracing<Fake, Vec<u8>>) -> Vec<String> {
        String::from_utf8(t.out.clone())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn writes_header_then_one_row_per_send_and_forwards() {
        let mut t = Tracing::new(Fake::default(), Vec::new());
        t.now_us();
        t.send(Output {
            v_units: 10,
            h_units: 0,
        });
        t.wait_frame(100); // +10 ms
        t.send(Output {
            v_units: -3,
            h_units: 7,
        });
        assert_eq!(
            csv(&t),
            vec![HEADER, "0.000,10,0", "10.000,-3,7"],
            "time is relative to the first event"
        );
        assert_eq!(t.inner.sent.len(), 2, "events still reach the real backend");
    }

    #[test]
    fn release_flushes_restarts_the_origin_and_forwards() {
        let mut t = Tracing::new(Fake::default(), Vec::new());
        t.send(Output {
            v_units: 1,
            h_units: 0,
        });
        t.release();
        assert_eq!(t.inner.released, 1);
        t.inner.now = 5_000_000; // a later burst
        t.send(Output {
            v_units: 2,
            h_units: 0,
        });
        let rows = csv(&t);
        assert_eq!(rows.len(), 3, "header written only once: {rows:?}");
        assert_eq!(rows[2], "0.000,2,0", "new burst starts again at 0 ms");
    }

    #[test]
    fn write_errors_are_ignored() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("disk full"))
            }
        }
        let mut t = Tracing::new(Fake::default(), Broken);
        t.send(Output {
            v_units: 1,
            h_units: 1,
        });
        t.release();
        assert_eq!(t.inner.sent.len(), 1, "scrolling is unaffected");
    }
}
