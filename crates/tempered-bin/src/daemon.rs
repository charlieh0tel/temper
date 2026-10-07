//! `tempered daemon`: presents the stick as an IIO device through
//! `/dev/uhid`.  See `docs/daemon.md`.

use std::env;
use std::fs::File;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::TryRecvError;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use rustix::io::Errno;
use signal_hook::consts::SIGINT;
use signal_hook::consts::SIGTERM;
use signal_hook::iterator::Signals;
use tempered_hid::hidraw::Hidraw;
use tempered_hid::protocol;
use tempered_hid::protocol::CentiCelsius;
use tempered_hid::protocol::Stick;

use crate::iio;
use crate::label::Label;
use crate::label::LabelLock;
use crate::listen;
use crate::listen::ListenError;
use crate::logger::Logger;
use crate::notify;
use crate::notify::Notifier;
use crate::schedule::Schedule;
use crate::sensor::Sensor;
use crate::supervisor::Action;
use crate::supervisor::FailureLog;
use crate::supervisor::Supervisor;
use crate::uhid;
use crate::uhid::Bus;
use crate::uhid::Create2;
use crate::uhid::FromKernel;
use crate::uhid::ToKernel;

/// The virtual device's IDs: the stick's own, on `BUS_VIRTUAL`.
const VENDOR: u32 = 0x3553;
const PRODUCT: u32 = 0xa001;

/// `HID_PHYS` of the virtual device.
const PHYS: &str = "tempered";

/// How long the kernel may take to create the IIO device.
const IIO_TIMEOUT: Duration = Duration::from_secs(10);

/// How often to look for the IIO device while waiting.
const IIO_POLL: Duration = Duration::from_millis(100);

/// Main-loop tick when systemd's watchdog is off.
const DEFAULT_TICK: Duration = Duration::from_secs(1);

/// How long past the interval the poll thread may go quiet before the
/// watchdog pings stop.  A failing query can take over 5 s.
const PROGRESS_GRACE: Duration = Duration::from_secs(10);

/// Exit statuses; see `docs/daemon.md`, "Exit status".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Stopped by a signal, or the stick was unplugged.
    Clean,
    /// A runtime failure; systemd restarts the daemon.
    Failure,
    /// A configuration error; not restarted.
    Config,
    /// The IIO device never appeared; not restarted.
    NoIio,
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        Self::from(match exit {
            Exit::Clean => 0,
            Exit::Failure => 1,
            Exit::Config => 2,
            Exit::NoIio => 3,
        })
    }
}

/// The daemon's settings, from the command line or environment.
#[derive(Debug)]
pub(crate) struct Config {
    pub(crate) device: Option<PathBuf>,
    pub(crate) label: Label,
    pub(crate) interval: Duration,
    pub(crate) hold: Duration,
    pub(crate) run_dir: PathBuf,
}

/// What the worker threads tell the main thread.
#[derive(Debug)]
enum Message {
    /// A stick query's result.
    Reading(Result<CentiCelsius, protocol::Error>),
    /// `SIGTERM` or `SIGINT`.
    Shutdown,
    /// A `DESTROY` the destroy thread wrote.
    Destroyed(io::Result<()>),
    /// A worker thread cannot go on.
    ThreadDied { thread: &'static str, error: String },
}

/// Locks `mutex`; a panic aborts the process, so poisoning cannot
/// leave the data half updated.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn var(key: &str) -> Option<String> {
    env::var(key).ok()
}

/// Runs the daemon until a signal, an unplug, or a fatal error.
pub(crate) fn run(config: Config) -> Exit {
    let logger = Arc::new(Logger::from_env(var));
    match start(config, &logger) {
        Ok(mut daemon) => daemon.run(),
        Err((exit, error)) => {
            logger.error(format_args!("{error:#}"));
            exit
        }
    }
}

/// Everything the main thread owns.
struct Daemon {
    logger: Arc<Logger>,
    notifier: Notifier,
    /// How long the main loop waits between watchdog pings.
    tick: Duration,
    watchdog: bool,
    label: Label,
    lock: LabelLock,
    uhid: Arc<File>,
    sensor: Arc<Mutex<Option<Sensor>>>,
    /// When the poll thread last finished a query.
    progress: Arc<Mutex<Instant>>,
    interval: Duration,
    messages: Receiver<Message>,
    destroy_requests: Sender<()>,
    supervisor: Supervisor,
    failures: FailureLog,
    /// A link that could not be written yet; retried with each reading.
    pending_link: Option<PathBuf>,
    /// Devices created so far, to make each `uniq` unique.
    creations: u32,
    /// Set once the daemon must stop, with its exit status.
    stopping: Option<Exit>,
}

type StartError = (Exit, anyhow::Error);

fn failure(error: impl Into<anyhow::Error>) -> StartError {
    (Exit::Failure, error.into())
}

/// Startup, through `READY=1`; see `docs/daemon.md`, "Startup".
fn start(config: Config, logger: &Arc<Logger>) -> Result<Daemon, StartError> {
    let notifier = Notifier::from_env(var).map_err(failure)?;
    let tick = notify::watchdog_tick(var);
    let mut stick = match &config.device {
        Some(path) => Stick::open(path).map_err(failure)?,
        None => Stick::find().map_err(failure)?,
    };
    let firmware = stick.firmware().map_err(failure)?;
    let uhid = Arc::new(listen::uhid(var).map_err(|error| match error {
        ListenError::Io(_) => failure(error),
        _ => (Exit::Config, error.into()),
    })?);
    let lock = acquire(&config.run_dir, &config.label, logger, &notifier)?;
    logger.info(format_args!(
        "{firmware} at {}, label {}",
        stick.transport().path().display(),
        config.label
    ));

    let (sender, messages) = mpsc::channel();
    let sensor = Arc::new(Mutex::new(None));
    let progress = Arc::new(Mutex::new(Instant::now()));
    spawn_signals(sender.clone()).map_err(failure)?;
    spawn_uhid(
        Arc::clone(&uhid),
        Arc::clone(&sensor),
        Arc::clone(logger),
        sender.clone(),
    );
    spawn_poll(
        stick,
        config.interval,
        Arc::clone(&progress),
        sender.clone(),
    );
    let destroy_requests = spawn_destroy(Arc::clone(&uhid), sender);

    let _ = notifier.ready();
    let _ = notifier.status("waiting for a reading");
    Ok(Daemon {
        logger: Arc::clone(logger),
        notifier,
        tick: tick.unwrap_or(DEFAULT_TICK),
        watchdog: tick.is_some(),
        label: config.label,
        lock,
        uhid,
        sensor,
        progress,
        interval: config.interval,
        messages,
        destroy_requests,
        supervisor: Supervisor::new(config.hold),
        failures: FailureLog::default(),
        pending_link: None,
        creations: 0,
        stopping: None,
    })
}

/// Takes the label's lock, waiting while another instance holds it.
fn acquire(
    dir: &Path,
    label: &Label,
    logger: &Logger,
    notifier: &Notifier,
) -> Result<LabelLock, StartError> {
    let mut logged = false;
    loop {
        if let Some(lock) = LabelLock::try_acquire(dir, label).map_err(|error| {
            failure(anyhow::Error::from(error).context(dir.display().to_string()))
        })? {
            return Ok(lock);
        }
        if !logged {
            logger.warning(format_args!("label {label} is in use; waiting for it"));
            logged = true;
        }
        let _ = notifier.extend_timeout(DEFAULT_TICK * 2);
        thread::sleep(DEFAULT_TICK);
    }
}

fn spawn_signals(sender: Sender<Message>) -> io::Result<()> {
    let mut signals = Signals::new([SIGTERM, SIGINT])?;
    thread::spawn(move || {
        for _ in signals.forever() {
            if sender.send(Message::Shutdown).is_err() {
                return;
            }
        }
    });
    Ok(())
}

/// Answers the kernel's requests from the sensor; see `docs/daemon.md`,
/// "Threads and state".
fn spawn_uhid(
    uhid: Arc<File>,
    sensor: Arc<Mutex<Option<Sensor>>>,
    logger: Arc<Logger>,
    sender: Sender<Message>,
) {
    thread::spawn(move || {
        loop {
            let event = match uhid::read_event(&uhid) {
                Ok(event) => event,
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                    logger.debug(format_args!("uhid: {error}"));
                    continue;
                }
                Err(error) => {
                    let _ = sender.send(Message::ThreadDied {
                        thread: "uhid",
                        error: error.to_string(),
                    });
                    return;
                }
            };
            // Built under the lock, written after it is released.
            let reply = lock(&sensor)
                .as_mut()
                .and_then(|sensor| sensor.handle(&event));
            match reply {
                Some(reply) => {
                    if let Err(error) = uhid::write_event(&uhid, reply.event()) {
                        logger.debug(format_args!("uhid reply: {error}"));
                    }
                }
                None => match event {
                    FromKernel::GetReport { .. } | FromKernel::SetReport { .. } => {
                        logger.debug(format_args!("uhid: request with no device: {event:?}"));
                    }
                    _ => logger.debug(format_args!("uhid: {event:?}")),
                },
            }
        }
    });
}

/// Queries the stick on schedule and reports each result.
fn spawn_poll(
    mut stick: Stick<Hidraw>,
    interval: Duration,
    progress: Arc<Mutex<Instant>>,
    sender: Sender<Message>,
) {
    thread::spawn(move || {
        let schedule = Schedule::new(interval);
        loop {
            let result = stick.temperature();
            *lock(&progress) = Instant::now();
            let gone = matches!(result, Err(protocol::Error::Gone));
            if sender.send(Message::Reading(result)).is_err() || gone {
                return;
            }
            schedule.sleep();
        }
    });
}

/// Writes `DESTROY` on request, so a long destroy does not stop the
/// main thread pinging the watchdog.
fn spawn_destroy(uhid: Arc<File>, sender: Sender<Message>) -> Sender<()> {
    let (requests, receiver) = mpsc::channel::<()>();
    thread::spawn(move || {
        for () in receiver {
            let result = uhid::write_event(&uhid, ToKernel::Destroy);
            if sender.send(Message::Destroyed(result)).is_err() {
                return;
            }
        }
    });
    requests
}

impl Daemon {
    fn run(&mut self) -> Exit {
        loop {
            if let Some(exit) = self.stopping {
                return self.stop(exit);
            }
            self.ping();
            match self.messages.recv_timeout(self.tick) {
                Ok(message) => self.handle(message),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.stopping = Some(Exit::Failure),
            }
        }
    }

    /// Handles a message outside any wait.
    fn handle(&mut self, message: Message) {
        match message {
            Message::Reading(Ok(temperature)) => self.on_reading(temperature),
            Message::Reading(Err(protocol::Error::Gone)) => {
                self.logger.info(format_args!("stick removed"));
                self.stopping = Some(Exit::Clean);
            }
            Message::Reading(Err(error)) => self.on_failure(error),
            other => self.handle_in_wait(other),
        }
    }

    /// Handles a message during a wait, where readings are dropped.
    fn handle_in_wait(&mut self, message: Message) {
        match message {
            Message::Shutdown => self.stopping = self.stopping.or(Some(Exit::Clean)),
            Message::ThreadDied { thread, error } => {
                self.logger
                    .error(format_args!("{thread} thread stopped: {error}"));
                self.stopping = Some(Exit::Failure);
            }
            Message::Reading(Err(protocol::Error::Gone)) => {
                self.logger.info(format_args!("stick removed"));
                self.stopping = self.stopping.or(Some(Exit::Clean));
            }
            Message::Reading(_) | Message::Destroyed(_) => {}
        }
    }

    /// Pings the watchdog while the poll thread makes progress.
    fn ping(&self) {
        let quiet = lock(&self.progress).elapsed();
        if self.watchdog && quiet <= self.interval + PROGRESS_GRACE {
            let _ = self.notifier.watchdog();
        }
    }

    fn on_reading(&mut self, temperature: CentiCelsius) {
        if self.failures.success() {
            self.logger.info(format_args!("readings resumed"));
        }
        match self.supervisor.on_reading(temperature, Instant::now()) {
            Action::Create(temperature) => {
                let created = self.create(temperature);
                if self.supervisor.created(created) == Some(Action::GiveUp) {
                    self.logger.error(format_args!(
                        "the IIO device never appeared; are hid_sensor_hub and \
                         hid_sensor_temperature available?"
                    ));
                    self.stopping = Some(Exit::NoIio);
                }
            }
            Action::Update(temperature) => self.update(temperature),
            Action::Destroy | Action::GiveUp => {}
        }
    }

    fn on_failure(&mut self, error: protocol::Error) {
        let now = Instant::now();
        if self.failures.failure(now) {
            self.logger.warning(format_args!(
                "stick not answering: {:#}",
                anyhow::Error::from(error)
            ));
        }
        if self.supervisor.on_failure(now) == Some(Action::Destroy) {
            self.logger.warning(format_args!(
                "held reading expired; removing the IIO device"
            ));
            self.destroy();
        }
    }

    /// Creates the device and links its IIO device; on failure leaves
    /// none behind.
    fn create(&mut self, temperature: CentiCelsius) -> bool {
        self.creations += 1;
        let uniq = format!("{PHYS}-{}-{}", process::id(), self.creations);
        let create = Create2 {
            name: self.label.as_str().to_owned(),
            phys: PHYS.to_owned(),
            uniq: uniq.clone(),
            bus: Bus::VIRTUAL,
            vendor: VENDOR,
            product: PRODUCT,
            version: 0,
            country: 0,
            descriptor: crate::sensor::DESCRIPTOR.to_vec(),
        };
        // The sensor exists before CREATE2, so the probe's requests are
        // answered.
        *lock(&self.sensor) = Some(Sensor::new(temperature));
        if let Err(error) = uhid::write_event(&self.uhid, ToKernel::Create2(&create)) {
            self.logger
                .error(format_args!("creating the uhid device: {error}"));
            *lock(&self.sensor) = None;
            return false;
        }
        let Some(path) = self.wait_for_iio(&uniq) else {
            if self.stopping.is_none() {
                self.logger.warning(format_args!(
                    "no IIO device within {IIO_TIMEOUT:?}; removing the uhid device"
                ));
            }
            self.destroy();
            return false;
        };
        self.logger.info(format_args!("created {}", path.display()));
        self.link(path);
        let _ = self.notifier.status("serving");
        true
    }

    /// Waits for the IIO device, pinging and watching for shutdown.
    fn wait_for_iio(&mut self, uniq: &str) -> Option<PathBuf> {
        let deadline = Instant::now() + IIO_TIMEOUT;
        while Instant::now() < deadline && self.stopping.is_none() {
            if let Some(path) = iio::find(uniq) {
                return Some(path);
            }
            self.ping();
            match self.messages.try_recv() {
                Ok(message) => self.handle_in_wait(message),
                Err(TryRecvError::Empty) => thread::sleep(IIO_POLL),
                Err(TryRecvError::Disconnected) => self.stopping = Some(Exit::Failure),
            }
        }
        None
    }

    fn update(&mut self, temperature: CentiCelsius) {
        let report = lock(&self.sensor)
            .as_mut()
            .map(|sensor| sensor.update(temperature));
        let Some(report) = report else {
            return;
        };
        match uhid::write_event(&self.uhid, ToKernel::Input2(&report)) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(Errno::INVAL.raw_os_error()) => {
                self.logger
                    .warning(format_args!("the kernel stopped the device; recreating it"));
                self.supervisor.on_device_dead();
                self.destroy();
            }
            Err(error) => self.logger.debug(format_args!("input report: {error}")),
        }
        if let Some(path) = self.pending_link.take() {
            self.link(path);
        }
    }

    fn link(&mut self, path: PathBuf) {
        if let Err(error) = self.lock.link(&path) {
            self.logger
                .warning(format_args!("linking {}: {error}; will retry", self.label));
            self.pending_link = Some(path);
        }
    }

    /// Removes the link and destroys the device, waiting for the destroy
    /// thread while pinging the watchdog.
    fn destroy(&mut self) {
        self.pending_link = None;
        if let Err(error) = self.lock.unlink() {
            self.logger
                .warning(format_args!("removing the link: {error}"));
        }
        if self.destroy_requests.send(()).is_err() {
            self.stopping = Some(Exit::Failure);
            return;
        }
        loop {
            self.ping();
            match self.messages.recv_timeout(self.tick) {
                Ok(Message::Destroyed(result)) => {
                    if let Err(error) = result {
                        self.logger.debug(format_args!("destroy: {error}"));
                    }
                    break;
                }
                Ok(message) => self.handle_in_wait(message),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.stopping = Some(Exit::Failure);
                    break;
                }
            }
        }
        *lock(&self.sensor) = None;
        let _ = self.notifier.status("waiting for a reading");
    }

    /// Cleans up and returns `exit`.  The other threads are not joined:
    /// process exit ends them wherever they are blocked.
    fn stop(&mut self, exit: Exit) -> Exit {
        let _ = self.notifier.stopping();
        if self.supervisor.is_present() || lock(&self.sensor).is_some() {
            self.destroy();
        } else if let Err(error) = self.lock.unlink() {
            self.logger
                .warning(format_args!("removing the link: {error}"));
        }
        self.logger.info(format_args!("stopped"));
        exit
    }
}
