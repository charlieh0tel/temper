//! A fixed-interval schedule that does not drift: slot `n` is at
//! `start + n * interval`, and a missed slot is skipped.

use std::thread;
use std::time::Duration;
use std::time::Instant;

/// Slots every `interval` from when it was made.
#[derive(Debug)]
pub(crate) struct Schedule {
    start: Instant,
    interval: Duration,
}

impl Schedule {
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            start: Instant::now(),
            interval,
        }
    }

    /// Sleeps until the next slot.
    pub(crate) fn sleep(&self) {
        let elapsed = self.start.elapsed();
        let next = self.interval * slot_after(elapsed, self.interval);
        thread::sleep(next.saturating_sub(elapsed));
    }
}

/// The first slot strictly after `elapsed`.
fn slot_after(elapsed: Duration, interval: Duration) -> u32 {
    let slots = elapsed.as_nanos() / interval.as_nanos() + 1;
    u32::try_from(slots).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots() {
        let interval = Duration::from_secs(10);
        assert_eq!(slot_after(Duration::ZERO, interval), 1);
        assert_eq!(slot_after(Duration::from_millis(9_999), interval), 1);
        assert_eq!(slot_after(Duration::from_secs(10), interval), 2);
        assert_eq!(slot_after(Duration::from_secs(35), interval), 4);
    }
}
