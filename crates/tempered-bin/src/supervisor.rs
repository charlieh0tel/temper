//! The daemon's lifecycle as a pure state machine: readings and
//! failures in, actions out.  See `docs/daemon.md`, "Supervisor".

use std::time::Duration;
use std::time::Instant;

use tempered_hid::protocol::CentiCelsius;

/// IIO-device failures in a row before giving up.
const MAX_CREATE_FAILURES: u32 = 3;

/// How often a long outage is logged again.
const FAILURE_REMINDER: Duration = Duration::from_secs(3600);

/// Whether a virtual device exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Absent,
    Present,
}

/// What the daemon must do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Create the virtual device serving this reading, then report the
    /// outcome with [`Supervisor::created`].
    Create(CentiCelsius),
    /// Serve this reading from the existing device.
    Update(CentiCelsius),
    /// Remove the link and destroy the device.
    Destroy,
    /// Give up: the IIO device never appeared.
    GiveUp,
}

/// The lifecycle of the virtual device.
#[derive(Debug)]
pub(crate) struct Supervisor {
    state: State,
    /// How long a reading is served after the stick stops answering.
    hold: Duration,
    /// When the stick last answered.
    last_good: Option<Instant>,
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

    pub(crate) fn on_reading(&mut self, temperature: CentiCelsius, now: Instant) -> Action {
        self.last_good = Some(now);
        match self.state {
            State::Absent => Action::Create(temperature),
            State::Present => Action::Update(temperature),
        }
    }

    /// The outcome of an [`Action::Create`].
    pub(crate) fn created(&mut self, ok: bool) -> Option<Action> {
        if ok {
            self.state = State::Present;
            self.create_failures = 0;
            return None;
        }
        self.create_failures += 1;
        (self.create_failures >= MAX_CREATE_FAILURES).then_some(Action::GiveUp)
    }

    /// The stick did not answer.
    pub(crate) fn on_failure(&mut self, now: Instant) -> Option<Action> {
        let expired = self
            .last_good
            .is_none_or(|last| now.saturating_duration_since(last) > self.hold);
        (self.state == State::Present && expired).then(|| self.destroyed())
    }

    /// The kernel stopped the device behind the daemon's back.
    pub(crate) fn on_device_dead(&mut self) -> Action {
        self.destroyed()
    }

    fn destroyed(&mut self) -> Action {
        self.state = State::Absent;
        Action::Destroy
    }
}

/// When to log stick failures: on the first of a run, then hourly, and
/// once when readings resume.
#[derive(Debug, Default)]
pub(crate) struct FailureLog {
    /// When the current run of failures was last logged.
    last_logged: Option<Instant>,
}

impl FailureLog {
    /// Whether this failure should be logged.
    pub(crate) fn failure(&mut self, now: Instant) -> bool {
        let due = self
            .last_logged
            .is_none_or(|last| now.saturating_duration_since(last) >= FAILURE_REMINDER);
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

    fn present(start: Instant) -> Supervisor {
        let mut supervisor = Supervisor::new(HOLD);
        assert_eq!(supervisor.on_reading(T, start), Action::Create(T));
        assert_eq!(supervisor.created(true), None);
        assert!(supervisor.is_present());
        supervisor
    }

    #[test]
    fn creates_then_updates() {
        let start = Instant::now();
        let mut supervisor = present(start);
        assert_eq!(supervisor.on_reading(T, start), Action::Update(T));
    }

    #[test]
    fn failures_within_hold_keep_device() {
        let start = Instant::now();
        let mut supervisor = present(start);
        assert_eq!(supervisor.on_failure(start + HOLD), None);
        assert!(supervisor.is_present());
    }

    #[test]
    fn hold_expiry_destroys_and_recovery_recreates() {
        let start = Instant::now();
        let mut supervisor = present(start);
        let later = start + HOLD + Duration::from_secs(1);
        assert_eq!(supervisor.on_failure(later), Some(Action::Destroy));
        assert!(!supervisor.is_present());
        assert_eq!(supervisor.on_failure(later), None);
        assert_eq!(supervisor.on_reading(T, later), Action::Create(T));
    }

    #[test]
    fn failures_while_absent_do_nothing() {
        let mut supervisor = Supervisor::new(HOLD);
        assert_eq!(supervisor.on_failure(Instant::now()), None);
    }

    #[test]
    fn three_create_failures_give_up() {
        let start = Instant::now();
        let mut supervisor = Supervisor::new(HOLD);
        for _ in 0..MAX_CREATE_FAILURES - 1 {
            assert_eq!(supervisor.on_reading(T, start), Action::Create(T));
            assert_eq!(supervisor.created(false), None);
        }
        assert_eq!(supervisor.on_reading(T, start), Action::Create(T));
        assert_eq!(supervisor.created(false), Some(Action::GiveUp));
    }

    #[test]
    fn success_resets_create_failures() {
        let start = Instant::now();
        let mut supervisor = Supervisor::new(HOLD);
        for _ in 0..MAX_CREATE_FAILURES - 1 {
            supervisor.on_reading(T, start);
            supervisor.created(false);
        }
        supervisor.on_reading(T, start);
        supervisor.created(true);
        supervisor.on_device_dead();
        for _ in 0..MAX_CREATE_FAILURES - 1 {
            supervisor.on_reading(T, start);
            assert_eq!(supervisor.created(false), None);
        }
    }

    #[test]
    fn dead_device_is_destroyed() {
        let mut supervisor = present(Instant::now());
        assert_eq!(supervisor.on_device_dead(), Action::Destroy);
        assert!(!supervisor.is_present());
    }

    #[test]
    fn failure_log_first_hourly_and_recovery() {
        let start = Instant::now();
        let mut log = FailureLog::default();
        assert!(!log.success());
        assert!(log.failure(start));
        assert!(!log.failure(start + Duration::from_secs(10)));
        assert!(log.failure(start + FAILURE_REMINDER));
        assert!(log.success());
        assert!(!log.success());
        assert!(log.failure(start + FAILURE_REMINDER));
    }
}
