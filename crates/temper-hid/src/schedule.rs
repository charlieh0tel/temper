//! Polling a stick at a fixed rate: a schedule that does not drift.
//! Slot `n` is at `start + n * interval`, and a missed slot is skipped,
//! so a slow query does not push later ones back.

use std::thread;
use std::time::Duration;
use std::time::Instant;

/// The shortest useful interval between queries: the stick is slow to
/// answer, and a query that fails can take far longer.
pub const MIN_INTERVAL: Duration = Duration::from_secs(1);

/// Slots every `interval` from when it was made.
#[derive(Clone, Copy, Debug)]
pub struct Schedule {
    start: Instant,
    interval: Duration,
}

impl Schedule {
    /// A schedule starting now.  A zero `interval` has no slots to wait
    /// for: [`Schedule::sleep`] returns at once.
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            start: Instant::now(),
            interval,
        }
    }

    /// The time between slots.
    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Sleeps until the next slot.
    pub fn sleep(&self) {
        let elapsed = self.start.elapsed();
        thread::sleep(next_slot(elapsed, self.interval).saturating_sub(elapsed));
    }
}

/// The time of the first slot strictly after `elapsed`, or `elapsed`
/// itself for a zero interval.  Saturates at `Duration::MAX`.
fn next_slot(elapsed: Duration, interval: Duration) -> Duration {
    let interval_nanos = interval.as_nanos();
    if interval_nanos == 0 {
        return elapsed;
    }
    let slot = elapsed.as_nanos() / interval_nanos + 1;
    slot.checked_mul(interval_nanos)
        .and_then(|nanos| u64::try_from(nanos).ok())
        .map_or(Duration::MAX, Duration::from_nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots() {
        let interval = Duration::from_secs(10);
        let next = |elapsed| next_slot(elapsed, interval);
        assert_eq!(next(Duration::ZERO), interval);
        assert_eq!(next(Duration::from_millis(9_999)), interval);
        assert_eq!(next(Duration::from_secs(10)), 2 * interval);
        assert_eq!(next(Duration::from_secs(35)), 4 * interval);
    }

    #[test]
    fn zero_interval_does_not_wait() {
        let elapsed = Duration::from_secs(3);
        assert_eq!(next_slot(elapsed, Duration::ZERO), elapsed);
        Schedule::new(Duration::ZERO).sleep();
    }

    #[test]
    fn far_slots_saturate() {
        assert_eq!(
            next_slot(Duration::MAX, Duration::from_secs(1)),
            Duration::MAX
        );
    }
}
