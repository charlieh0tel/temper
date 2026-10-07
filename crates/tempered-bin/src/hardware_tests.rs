//! Root-only tests of the real `tempered` binary against the real
//! kernel, with a fake stick: a second uhid device on `BUS_USB` with the
//! stick's VID:PID and data interface, answering hidraw commands.  Run
//! after `cargo build`, since they start `target/debug/tempered`.

use std::env;
use std::fs;
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::event::Timespec;
use tempfile::TempDir;

use crate::iio::has_uniq;
use crate::test_support;
use crate::uhid;
use crate::uhid::Bus;
use crate::uhid::Create2;
use crate::uhid::FromKernel;
use crate::uhid::ToKernel;
use crate::uhid::read_event;
use crate::uhid::write_event;

const SYSFS_HIDRAW: &str = "/sys/class/hidraw";

/// The stick's IDs, which discovery and the daemon look for.
const VENDOR: u32 = 0x3553;
const PRODUCT: u32 = 0xa001;

/// The stick's data interface: vendor page, one 8-byte input and one
/// 8-byte output report, no report IDs.  HID 1.11, section 6.2.2.
#[rustfmt::skip]
const STICK_DESCRIPTOR: &[u8] = &[
    0x06, 0x00, 0xff,       // Usage Page (Vendor Defined 0xFF00)
    0x09, 0x01,             // Usage (0x01)
    0xa1, 0x01,             // Collection (Application)
    0x15, 0x00,             //   Logical Minimum (0)
    0x26, 0xff, 0x00,       //   Logical Maximum (255)
    0x75, 0x08,             //   Report Size (8)
    0x95, 0x08,             //   Report Count (8)
    0x09, 0x02,             //   Usage (0x02)
    0x81, 0x02,             //   Input (Data, Variable, Absolute)
    0x09, 0x03,             //   Usage (0x03)
    0x91, 0x02,             //   Output (Data, Variable, Absolute)
    0xc0,                   // End Collection
];

/// Replies to the firmware query: `TEMPerGold_V3.5`, as captured.
const FIRMWARE_REPLY: [&[u8; 8]; 2] = [b"TEMPerGo", b"ld_V3.5 "];

/// The second byte of the commands the fake answers.
const FIRMWARE_COMMAND: u8 = 0x86;
const TEMPERATURE_COMMAND: u8 = 0x80;

/// Daemon settings short enough for tests.
const INTERVAL: &str = "1s";
const HOLD: &str = "2s";

/// Generous bounds on how long things take.
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_PERIOD: Duration = Duration::from_millis(100);

/// What the fake stick answers.
#[derive(Debug)]
struct FakeState {
    centi_celsius: i16,
    answering: bool,
}

/// A fake stick, removed when dropped.
#[derive(Debug)]
struct FakeStick {
    uhid: Arc<File>,
    state: Arc<Mutex<FakeState>>,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
    hidraw: PathBuf,
}

/// Polls `condition` until it holds or `timeout` passes.
fn wait_for<T>(timeout: Duration, mut condition: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = condition() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(POLL_PERIOD);
    }
}

/// The `/dev/hidrawN` node of the HID device created with `uniq`.
fn hidraw_with_uniq(uniq: &str) -> Option<PathBuf> {
    fs::read_dir(SYSFS_HIDRAW).ok()?.find_map(|entry| {
        let entry = entry.ok()?;
        has_uniq(&entry.path().join("device"), uniq)
            .then(|| Path::new("/dev").join(entry.file_name()))
    })
}

/// The 8-byte reply to a command, if the fake answers it.
fn replies(command: &[u8], state: &FakeState) -> Vec<[u8; 8]> {
    if !state.answering {
        return Vec::new();
    }
    match command.get(1) {
        Some(&FIRMWARE_COMMAND) => FIRMWARE_REPLY.iter().map(|r| **r).collect(),
        Some(&TEMPERATURE_COMMAND) => {
            let [high, low] = state.centi_celsius.to_be_bytes();
            vec![[0x80, 0x80, high, low, 0x4e, 0x20, 0, 0]]
        }
        _ => Vec::new(),
    }
}

impl FakeStick {
    fn new(uniq: &str) -> Self {
        let uhid = Arc::new(uhid::open().unwrap());
        let create = Create2 {
            name: "PCsensor TEMPerGold (fake)".to_owned(),
            phys: format!("{uniq}/input1"),
            uniq: uniq.to_owned(),
            bus: Bus::USB,
            vendor: VENDOR,
            product: PRODUCT,
            version: 0,
            country: 0,
            descriptor: STICK_DESCRIPTOR.to_vec(),
        };
        write_event(&uhid, ToKernel::Create2(&create)).unwrap();
        let state = Arc::new(Mutex::new(FakeState {
            centi_celsius: 2345,
            answering: true,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let server = {
            let (uhid, state, stop) = (Arc::clone(&uhid), Arc::clone(&state), Arc::clone(&stop));
            thread::spawn(move || serve(&uhid, &state, &stop))
        };
        let hidraw = wait_for(SETUP_TIMEOUT, || hidraw_with_uniq(uniq))
            .expect("the fake stick's hidraw node never appeared");
        Self {
            uhid,
            state,
            stop,
            server: Some(server),
            hidraw,
        }
    }

    fn set_temperature(&self, centi_celsius: i16) {
        self.state.lock().unwrap().centi_celsius = centi_celsius;
    }

    fn set_answering(&self, answering: bool) {
        self.state.lock().unwrap().answering = answering;
    }

    /// Removes the device, as an unplug would.
    fn unplug(&mut self) {
        if let Some(server) = self.server.take() {
            let _ = write_event(&self.uhid, ToKernel::Destroy);
            self.stop.store(true, Ordering::Relaxed);
            server.join().unwrap();
        }
    }
}

impl Drop for FakeStick {
    fn drop(&mut self) {
        self.unplug();
    }
}

/// Answers hidraw commands, which arrive as output reports.
fn serve(uhid: &File, state: &Mutex<FakeState>, stop: &AtomicBool) {
    let timeout = Timespec::try_from(POLL_PERIOD).unwrap();
    while !stop.load(Ordering::Relaxed) {
        let mut fds = [PollFd::new(uhid, PollFlags::IN)];
        if rustix::event::poll(&mut fds, Some(&timeout)).unwrap() == 0 {
            continue;
        }
        let Ok(FromKernel::Output { data, .. }) = read_event(uhid) else {
            continue;
        };
        // hidraw passes the 0x00 report ID through for an unnumbered
        // device.
        let command = data.strip_prefix(&[0]).unwrap_or(&data);
        for reply in replies(command, &state.lock().unwrap()) {
            let _ = write_event(uhid, ToKernel::Input2(&reply));
        }
    }
}

/// `target/debug/tempered`, next to this test binary's `deps`.
fn binary() -> PathBuf {
    let exe = env::current_exe().unwrap();
    let path = exe.parent().unwrap().parent().unwrap().join("tempered");
    assert!(path.exists(), "{} missing; run cargo build", path.display());
    path
}

/// A running `tempered daemon`, killed if still running when dropped.
#[derive(Debug)]
struct Daemon {
    child: Child,
    run_dir: TempDir,
}

const LABEL: &str = "temperature";

impl Daemon {
    fn start(stick: &FakeStick) -> Self {
        let run_dir = tempfile::tempdir().unwrap();
        let child = Command::new(binary())
            .arg("--device")
            .arg(&stick.hidraw)
            .args(["daemon", "--label", LABEL, "--interval", INTERVAL])
            .args(["--hold", HOLD, "--run-dir"])
            .arg(run_dir.path())
            .spawn()
            .unwrap();
        Self { child, run_dir }
    }

    fn link(&self) -> PathBuf {
        self.run_dir.path().join(LABEL)
    }

    /// The link's target, once the IIO device is up.
    fn wait_for_link(&self) -> PathBuf {
        wait_for(SETUP_TIMEOUT, || fs::read_link(self.link()).ok())
            .expect("the link never appeared")
    }

    fn raw(&self) -> Option<i16> {
        fs::read_to_string(self.link().join("in_temp_raw"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn signal(&self, signal: &str) {
        let status = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(self.child.id().to_string())
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn wait_exit(&mut self) -> ExitStatus {
        wait_for(SETUP_TIMEOUT, || self.child.try_wait().unwrap()).expect("the daemon did not exit")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "needs root for /dev/uhid"]
fn serves_follows_holds_and_recovers() {
    let _one = test_support::one_temperature_sensor();
    let stick = FakeStick::new("tempered-test-serve");
    let mut daemon = Daemon::start(&stick);
    let first = daemon.wait_for_link();
    assert_eq!(wait_for(SETUP_TIMEOUT, || daemon.raw()), Some(2345));

    stick.set_temperature(-512);
    assert!(wait_for(SETUP_TIMEOUT, || (daemon.raw() == Some(-512)).then_some(())).is_some());

    stick.set_answering(false);
    assert!(
        wait_for(SETUP_TIMEOUT, || (!first.exists()).then_some(())).is_some(),
        "the IIO device outlived the hold"
    );
    assert!(fs::symlink_metadata(daemon.link()).is_err());

    stick.set_temperature(1999);
    stick.set_answering(true);
    let second = daemon.wait_for_link();
    assert_ne!(first, second);
    assert_eq!(wait_for(SETUP_TIMEOUT, || daemon.raw()), Some(1999));

    daemon.signal("TERM");
    assert_eq!(daemon.wait_exit().code(), Some(0));
    assert!(!second.exists());
    assert!(fs::symlink_metadata(daemon.link()).is_err());
}

#[test]
#[ignore = "needs root for /dev/uhid"]
fn unplug_cleans_up_and_exits_zero() {
    let _one = test_support::one_temperature_sensor();
    let mut stick = FakeStick::new("tempered-test-unplug");
    let mut daemon = Daemon::start(&stick);
    let target = daemon.wait_for_link();
    stick.unplug();
    assert_eq!(daemon.wait_exit().code(), Some(0));
    assert!(!target.exists());
    assert!(fs::symlink_metadata(daemon.link()).is_err());
}

#[test]
#[ignore = "needs root for /dev/uhid"]
fn sigkill_leaves_no_device() {
    let _one = test_support::one_temperature_sensor();
    let stick = FakeStick::new("tempered-test-kill");
    let mut daemon = Daemon::start(&stick);
    let target = daemon.wait_for_link();
    daemon.signal("KILL");
    daemon.wait_exit();
    assert!(
        wait_for(SETUP_TIMEOUT, || (!target.exists()).then_some(())).is_some(),
        "the IIO device outlived the daemon"
    );
}
