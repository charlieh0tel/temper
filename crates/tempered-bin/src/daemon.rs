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
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::TryRecvError;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use rustix::io::Errno;
use signal_hook::consts::SIGHUP;
use signal_hook::consts::SIGINT;
use signal_hook::consts::SIGTERM;
use signal_hook::iterator::Signals;
use tempered_hid::hidraw::Hidraw;
use tempered_hid::hidraw::PRODUCT_ID;
use tempered_hid::hidraw::VENDOR_ID;
use tempered_hid::protocol;
use tempered_hid::protocol::CentiCelsius;
use tempered_hid::protocol::Firmware;
use tempered_hid::protocol::Stick;

use crate::VERSION;
use crate::iio;
use crate::label::Label;
use crate::label::LabelLock;
use crate::listen;
use crate::listen::ListenError;
use crate::logger::Logger;
use crate::mutex::lock;
use crate::notify;
use crate::notify::Notifier;
use crate::schedule::Schedule;
use crate::sensor::Quantity;
use crate::sensor::Sample;
use crate::sensor::Sensor;
use crate::supervisor::Action;
use crate::supervisor::BootTime;
use crate::supervisor::FailureLog;
use crate::supervisor::Supervisor;
use crate::uhid;
use crate::uhid::Bus;
use crate::uhid::Create2;
use crate::uhid::FromKernel;
use crate::uhid::ToKernel;

/// `HID_PHYS` of the virtual device.
const PHYS: &str = "tempered";

/// Firmware queries at startup before giving up: a stick can be slow to
/// answer right after it is plugged in.
const FIRMWARE_ATTEMPTS: u32 = 5;

/// The pause between those attempts.
const FIRMWARE_RETRY: Duration = Duration::from_secs(1);

/// How long to wait for a HID sensor left by an earlier
/// instance of this daemon to go away: one that crashed releases its
/// label lock before the kernel finishes destroying its device.
const LEFTOVER_WAIT: Duration = Duration::from_secs(10);

/// How long the kernel may take to create the IIO devices.
const IIO_TIMEOUT: Duration = Duration::from_secs(10);

/// How often to look for the IIO devices while waiting.
const IIO_POLL: Duration = Duration::from_millis(100);

/// Main-loop tick when systemd's watchdog is off.
const DEFAULT_TICK: Duration = Duration::from_secs(1);

/// How much more start time to ask for each tick while waiting for
/// the label's lock: two ticks, so one late tick does not time out.
const LOCK_WAIT_EXTENSION: Duration = DEFAULT_TICK.saturating_mul(2);

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
    /// The device cannot be presented: unsupported firmware, another
    /// HID temperature sensor, or no IIO device.  Not restarted.
    CannotPresent,
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        Self::from(match exit {
            Exit::Clean => 0,
            Exit::Failure => 1,
            Exit::Config => 2,
            Exit::CannotPresent => 3,
        })
    }
}

/// The daemon's settings, from the command line or environment.
#[derive(Debug)]
pub(crate) struct Config {
    /// The stick's hidraw node; found if `None`.
    pub(crate) device: Option<PathBuf>,
    /// The name of the link under `run_dir`.
    pub(crate) label: Label,
    /// Time between readings.
    pub(crate) interval: Duration,
    /// How long a reading is served after the stick stops answering.
    pub(crate) hold: Duration,
    /// Where the label's link and lock live.
    pub(crate) run_dir: PathBuf,
}

/// What the worker threads tell the main thread.
#[derive(Debug)]
enum Message {
    /// A stick query's result.
    Reading(Result<CentiCelsius, protocol::Error>),
    /// `SIGTERM`, `SIGINT` or `SIGHUP`.
    Shutdown,
    /// A `DESTROY` the destroy thread wrote.
    Destroyed(io::Result<()>),
    /// A worker thread cannot go on.
    ThreadDied { thread: &'static str, error: String },
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

/// A quantity's IIO device link: its label, owned through the lock.
#[derive(Debug)]
struct Link {
    /// Which IIO device it points to.
    quantity: Quantity,
    /// The link's name.
    label: Label,
    /// Ownership of the label, and its link.
    lock: LabelLock,
    /// A target that could not be linked yet; retried with each reading.
    pending: Option<PathBuf>,
}

/// Everything the main thread owns.
struct Daemon {
    /// Shared with the uhid thread.
    logger: Arc<Logger>,
    /// systemd's notification socket.
    notifier: Notifier,
    /// How long the main loop waits between watchdog pings.
    tick: Duration,
    /// Whether systemd's watchdog is on.
    watchdog: bool,
    /// The uhid device's name: the first link's label.
    name: String,
    /// One per quantity the sensor reports, in descriptor order.
    links: Vec<Link>,
    /// `/dev/uhid`, shared with the uhid and destroy threads.
    uhid: Arc<File>,
    /// What the virtual device serves; `None` while there is none.
    sensor: Arc<Mutex<Option<Sensor>>>,
    /// When the poll thread last finished a query.
    progress: Arc<Mutex<Instant>>,
    /// Time between readings.
    interval: Duration,
    /// From the worker threads.
    messages: Receiver<Message>,
    /// To the destroy thread.
    destroy_requests: Sender<()>,
    /// The virtual device's lifecycle.
    supervisor: Supervisor,
    /// When to log stick failures.
    failures: FailureLog,
    /// Devices created so far, to make each `uniq` unique.
    creations: u32,
    /// Set once the daemon must stop, with its exit status.
    stopping: Option<Exit>,
}

/// A startup failure: the exit status it calls for, and why.
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
    let firmware = query_firmware(&mut stick, &notifier)?;
    let uhid = Arc::new(listen::uhid(var).map_err(|error| match error {
        ListenError::Io(_) => failure(error),
        _ => (Exit::Config, error.into()),
    })?);
    let labels = [(Quantity::Temperature, config.label)];
    let mut links = Vec::with_capacity(labels.len());
    for (quantity, label) in labels {
        let lock = acquire(&config.run_dir, &label, logger, &notifier)?;
        links.push(Link {
            quantity,
            label,
            lock,
            pending: None,
        });
    }
    logger.info(format_args!(
        "tempered {}: {firmware} at {}, {}",
        VERSION,
        stick.transport().path().display(),
        links
            .iter()
            .map(|link| format!("{} label {}", link.quantity.iio_name(), link.label))
            .collect::<Vec<_>>()
            .join(", ")
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
        name: links[0].label.as_str().to_owned(),
        links,
        uhid,
        sensor,
        progress,
        interval: config.interval,
        messages,
        destroy_requests,
        supervisor: Supervisor::new(config.hold),
        failures: FailureLog::default(),
        creations: 0,
        stopping: None,
    })
}

/// Queries the firmware, retrying while the stick may still be settling
/// after it was plugged in.  A stick that answers but is not supported
/// is not retried, and its exit status keeps systemd from restarting.
fn query_firmware(stick: &mut Stick<Hidraw>, notifier: &Notifier) -> Result<Firmware, StartError> {
    let mut attempt = 1;
    loop {
        match stick.firmware() {
            Ok(firmware) => return Ok(firmware),
            Err(
                error @ (protocol::Error::UnsupportedFirmware { .. }
                | protocol::Error::FirmwareNotAscii { .. }),
            ) => return Err((Exit::CannotPresent, error.into())),
            Err(protocol::Error::Gone) => {
                return Err((Exit::Clean, anyhow::anyhow!("stick removed")));
            }
            Err(error) if attempt >= FIRMWARE_ATTEMPTS => return Err(failure(error)),
            Err(_) => {
                attempt += 1;
                let _ = notifier.extend_timeout(LOCK_WAIT_EXTENSION);
                thread::sleep(FIRMWARE_RETRY);
            }
        }
    }
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
        let _ = notifier.extend_timeout(LOCK_WAIT_EXTENSION);
        thread::sleep(DEFAULT_TICK);
    }
}

fn spawn_signals(sender: Sender<Message>) -> io::Result<()> {
    // SIGHUP too: systemd counts it as a clean exit, and a terminal
    // hangup would otherwise skip the cleanup.
    let mut signals = Signals::new([SIGTERM, SIGINT, SIGHUP])?;
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
            Message::Reading(Err(error)) if !matches!(error, protocol::Error::Gone) => {
                self.on_failure(error);
            }
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
        let samples = [Sample {
            quantity: Quantity::Temperature,
            centi: temperature.get().into(),
        }];
        match self.supervisor.on_reading(samples, BootTime::now()) {
            Action::Create(samples) => {
                if let Some((quantity, other)) = self.other_sensor() {
                    let kind = quantity.iio_name();
                    self.logger.error(format_args!(
                        "another HID {kind} sensor exists ({}); the kernel's \
                         hid-sensor-{kind} cannot handle two, so not creating one",
                        other.display()
                    ));
                    self.stop_with(Exit::CannotPresent);
                    return;
                }
                let created = self.create(&samples);
                // A create cut short by shutdown is not a failure of the
                // kernel's.
                if self.stopping.is_none() && self.supervisor.created(created) {
                    self.logger.error(format_args!(
                        "the IIO devices never appeared; are hid_sensor_hub and \
                         the sensor drivers available?"
                    ));
                    self.stop_with(Exit::CannotPresent);
                }
            }
            Action::Update(samples) => self.update(&samples),
        }
    }

    /// Another HID sensor of a quantity this one reports, once any left
    /// by a crashed instance of this daemon has had time to go away.
    fn other_sensor(&mut self) -> Option<(Quantity, PathBuf)> {
        let quantities = self
            .links
            .iter()
            .map(|link| link.quantity)
            .collect::<Vec<_>>();
        let deadline = Instant::now() + LEFTOVER_WAIT;
        loop {
            let (quantity, other) = quantities
                .iter()
                .find_map(|&quantity| Some((quantity, iio::sensor(quantity)?)))?;
            if !iio::sensor_has_phys(&other, PHYS) || Instant::now() >= deadline {
                return Some((quantity, other));
            }
            self.ping();
            thread::sleep(IIO_POLL);
        }
    }

    /// Stops with `exit`, unless a stop is already under way.
    fn stop_with(&mut self, exit: Exit) {
        self.stopping = self.stopping.or(Some(exit));
    }

    fn on_failure(&mut self, error: protocol::Error) {
        let now = BootTime::now();
        if self.failures.failure(now) {
            self.logger.warning(format_args!(
                "stick not answering: {:#}",
                anyhow::Error::from(error)
            ));
        }
        if self.supervisor.on_failure(now) {
            self.logger.warning(format_args!(
                "held reading expired; removing the IIO device"
            ));
            self.destroy();
        }
    }

    /// Creates the device and links its IIO devices; on failure leaves
    /// none behind.
    fn create(&mut self, samples: &[Sample]) -> bool {
        self.creations += 1;
        let uniq = format!("{PHYS}-{}-{}", process::id(), self.creations);
        let sensor = Sensor::new(samples);
        let create = Create2 {
            name: self.name.clone(),
            phys: PHYS.to_owned(),
            uniq: uniq.clone(),
            bus: Bus::VIRTUAL,
            // The stick's own IDs, on `BUS_VIRTUAL`.
            vendor: VENDOR_ID.into(),
            product: PRODUCT_ID.into(),
            version: 0,
            country: 0,
            descriptor: sensor.descriptor(),
        };
        // The sensor exists before CREATE2, so the probe's requests are
        // answered.
        *lock(&self.sensor) = Some(sensor);
        if let Err(error) = uhid::write_event(&self.uhid, ToKernel::Create2(&create)) {
            self.logger
                .error(format_args!("creating the uhid device: {error}"));
            *lock(&self.sensor) = None;
            return false;
        }
        let Some(paths) = self.wait_for_iio(&uniq) else {
            if self.stopping.is_none() {
                self.logger.warning(format_args!(
                    "no IIO devices within {IIO_TIMEOUT:?}; removing the uhid device"
                ));
            }
            self.destroy();
            return false;
        };
        for (index, path) in paths.into_iter().enumerate() {
            self.logger.info(format_args!("created {}", path.display()));
            self.link(index, path);
        }
        let _ = self.notifier.status("serving");
        true
    }

    /// Waits for every link's IIO device, pinging and watching for
    /// shutdown; their paths, in link order.
    fn wait_for_iio(&mut self, uniq: &str) -> Option<Vec<PathBuf>> {
        let deadline = Instant::now() + IIO_TIMEOUT;
        while Instant::now() < deadline && self.stopping.is_none() {
            let found = self
                .links
                .iter()
                .map(|link| iio::find(uniq, link.quantity.iio_name()))
                .collect::<Option<Vec<_>>>();
            if found.is_some() {
                return found;
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

    fn update(&mut self, samples: &[Sample]) {
        let reports = lock(&self.sensor)
            .as_mut()
            .map(|sensor| sensor.update(samples));
        let Some(reports) = reports else {
            return;
        };
        for report in reports {
            match uhid::write_event(&self.uhid, ToKernel::Input2(&report)) {
                Ok(()) => {}
                Err(error) if error.raw_os_error() == Some(Errno::INVAL.raw_os_error()) => {
                    self.logger
                        .warning(format_args!("the kernel stopped the device; recreating it"));
                    self.supervisor.on_device_dead();
                    self.destroy();
                    return;
                }
                Err(error) => self.logger.debug(format_args!("input report: {error}")),
            }
        }
        for index in 0..self.links.len() {
            if let Some(path) = self.links[index].pending.take() {
                self.link(index, path);
            }
        }
    }

    /// Points link `index` at `path`, or keeps it pending on failure.
    fn link(&mut self, index: usize, path: PathBuf) {
        let link = &mut self.links[index];
        if let Err(error) = link.lock.link(&path) {
            self.logger
                .warning(format_args!("linking {}: {error}; will retry", link.label));
            link.pending = Some(path);
        }
    }

    /// Removes every link.
    fn unlink(&mut self) {
        for link in &mut self.links {
            link.pending = None;
            if let Err(error) = link.lock.unlink() {
                self.logger
                    .warning(format_args!("removing {}: {error}", link.label));
            }
        }
    }

    /// Removes the links and destroys the device, waiting for the destroy
    /// thread while pinging the watchdog.
    fn destroy(&mut self) {
        self.unlink();
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
        } else {
            self.unlink();
        }
        self.logger.info(format_args!("stopped"));
        exit
    }
}
