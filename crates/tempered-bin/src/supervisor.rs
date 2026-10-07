//! The daemon's lifecycle as a pure state machine: readings and
//! failures in, actions out.  See `docs/daemon.md`, "Supervisor".

use std::time::Duration;

use rustix::time::ClockId;
use tempered_hid::protocol::CentiCelsius;

/// IIO-device failures in a row before giving up.
const MAX_CREATE_FAILURES: u32 = 3;

/// How often a long outage is logged again.
const FAILURE_REMINDER: Duration = Duration::from_secs(3600);

/// A point in time on `CLOCK_BOOTTIME`, which keeps counting while the
/// machine is suspended, so a reading held across a suspend ages by the
/// time it was asleep.  `Instant` uses `CLOCK_MONOTONIC`, which stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BootTime(Duration);

impl BootTime {
    pub(crate) fn now() -> Self {
        let now = rustix::time::clock_gettime(ClockId::Boottime);
        Self(Duration::try_from(now).unwrap_or_default())
    }

    /// The time from `earlier` to this, or zero if `earlier` is later.
    fn since(self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }
}

/// Whether a virtual device exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Absent,
    Present,
}

/// What to do with a good reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Create the virtual device serving this reading, then report the
    /// outcome with [`Supervisor::created`].
    Create(CentiCelsius),
    /// Serve this reading from the existing device.
    Update(CentiCelsius),
}

/// The lifecycle of the virtual device.
#[derive(Debug)]
pub(crate) struct Supervisor {
    /// Whether the virtual device exists.
    state: State,
    /// How long a reading is served after the stick stops answering.
    hold: Duration,
    /// When the stick last answered.
    last_good: Option<BootTime>,
    /// IIO-device failures in a row.
    create_failures: u32,
}

impl Supervisor {
    pub(crate) fn new(hold: Duration) -> Self {
        Self {
            state: State::Absent,
            hold,
            last_good: None,
            create_failures: 0,
        }
    }

    pub(crate) fn is_present(&self) -> bool {
        self.state == State::Present
    }

    pub(crate) fn on_reading(&mut self, temperature: CentiCelsius, now: BootTime) -> Action {
        self.last_good = Some(now);
        match self.state {
            State::Absent => Action::Create(temperature),
            State::Present => Action::Update(temperature),
        }
    }

    /// Records the outcome of an [`Action::Create`]; whether to give up,
    /// the IIO device having failed to appear too many times in a row.
    #[must_use]
    pub(crate) fn created(&mut self, ok: bool) -> bool {
        if ok {
            self.state = State::Present;
            self.create_failures = 0;
            return false;
        }
        self.create_failures += 1;
        self.create_failures >= MAX_CREATE_FAILURES
    }

    /// The stick did not answer; whether the held reading has expired, so
    /// the device must be destroyed now.
    #[must_use]
    pub(crate) fn on_failure(&mut self, now: BootTime) -> bool {
        let expired = self
            .last_good
            .is_none_or(|last| now.since(last) > self.hold);
        let destroy = self.state == State::Present && expired;
        if destroy {
            self.state = State::Absent;
        }
        destroy
    }

    /// The kernel stopped the device behind the daemon's back; it must be
    /// destroyed.
    pub(crate) fn on_device_dead(&mut self) {
        self.state = State::Absent;
    }
}

/// When to log stick failures: on the first of a run, then hourly, and
/// once when readings resume.
#[derive(Debug, Default)]
pub(crate) struct FailureLog {
    /// When the current run of failures was last logged.
    last_logged: Option<BootTime>,
}

impl FailureLog {
    /// Whether this failure should be logged.
    pub(crate) fn failure(&mut self, now: BootTime) -> bool {
        let due = self
            .last_logged
            .is_none_or(|last| now.since(last) >= FAILURE_REMINDER);
        if due {
            self.last_logged = Some(now);
        }
        due
    }

    /// Whether this reading ends a logged run of failures.
    pub(crate) fn success(&mut self) -> bool {
        self.last_logged.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOLD: Duration = Duration::from_secs(60);
    const T: CentiCelsius = CentiCelsius::new(3512);

    fn present(start: BootTime) -> Supervisor {
        let mut supervisor = Supervisor::new(HOLD);
        assert_eq!(supervisor.on_reading(T, start), Action::Create(T));
        assert!(!supervisor.created(true));
        assert!(supervisor.is_present());
        supervisor
    }

    #[test]
    fn creates_then_updates() {
        let start = BootTime(Duration::ZERO);
        let mut supervisor = present(start);
        assert_eq!(supervisor.on_reading(T, start), Action::Update(T));
    }

    #[test]
    fn failures_within_hold_keep_device() {
        let start = BootTime(Duration::ZERO);
        let mut supervisor = present(start);
        assert!(!supervisor.on_failure(BootTime(start.0 + HOLD)));
        assert!(supervisor.is_present());
    }

    #[test]
    fn hold_expiry_destroys_and_recovery_recreates() {
        let start = BootTime(Duration::ZERO);
        let mut supervisor = present(start);
        let later = BootTime(start.0 + HOLD + Duration::from_secs(1));
        assert!(supervisor.on_failure(later));
        assert!(!supervisor.is_present());
        assert!(!supervisor.on_failure(later));
        assert_eq!(supervisor.on_reading(T, later), Action::Create(T));
    }

    #[test]
    fn failures_while_absent_do_nothing() {
        let mut supervisor = Supervisor::new(HOLD);
        assert!(!supervisor.on_failure(BootTime(Duration::ZERO)));
    }

    #[test]
    fn three_create_failures_give_up() {
        let start = BootTime(Duration::ZERO);
        let mut supervisor = Supervisor::new(HOLD);
        for _ in 0..MAX_CREATE_FAILURES - 1 {
            assert_eq!(supervisor.on_reading(T, start), Action::Create(T));
            assert!(!supervisor.created(false));
        }
        assert_eq!(supervisor.on_reading(T, start), Action::Create(T));
        assert!(supervisor.created(false));
    }

    #[test]
    fn success_resets_create_failures() {
        let start = BootTime(Duration::ZERO);
        let mut supervisor = Supervisor::new(HOLD);
        for _ in 0..MAX_CREATE_FAILURES - 1 {
            supervisor.on_reading(T, start);
            assert!(!supervisor.created(false));
        }
        supervisor.on_reading(T, start);
        assert!(!supervisor.created(true));
        supervisor.on_device_dead();
        for _ in 0..MAX_CREATE_FAILURES - 1 {
            supervisor.on_reading(T, start);
            assert!(!supervisor.created(false));
        }
    }

    #[test]
    fn dead_device_is_destroyed() {
        let mut supervisor = present(BootTime(Duration::ZERO));
        supervisor.on_device_dead();
        assert!(!supervisor.is_present());
    }

    #[test]
    fn failure_log_first_hourly_and_recovery() {
        let start = BootTime(Duration::ZERO);
        let mut log = FailureLog::default();
        assert!(!log.success());
        assert!(log.failure(start));
        assert!(!log.failure(BootTime(start.0 + Duration::from_secs(10))));
        assert!(log.failure(BootTime(start.0 + FAILURE_REMINDER)));
        assert!(log.success());
        assert!(!log.success());
        assert!(log.failure(BootTime(start.0 + FAILURE_REMINDER)));
    }
}
