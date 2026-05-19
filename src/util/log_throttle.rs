use std::cell::Cell;
use std::time::{Duration, Instant};

/// Rate-limits log output to at most once per `interval`.
/// Uses `Cell<Instant>` so it works behind `&self` without requiring `&mut self`.
pub struct LogThrottle {
    last: Cell<Instant>,
    interval: Duration,
}

impl LogThrottle {
    pub fn new(interval: Duration) -> Self {
        // Subtract the interval so the very first call always fires.
        Self {
            last: Cell::new(Instant::now() - interval),
            interval,
        }
    }

    pub fn per_second() -> Self {
        Self::new(Duration::from_secs(1))
    }

    /// Returns `true` at most once per interval.
    pub fn should_log(&self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last.get()) >= self.interval {
            self.last.set(now);
            true
        } else {
            false
        }
    }
}
