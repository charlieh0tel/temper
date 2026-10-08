//! Finding a stick's data interface and talking to it.  The platform
//! code is behind a private backend (`backend.rs`): on Linux,
//! `/sys/class/hidraw` and the `/dev/hidrawN` node (`linux.rs`); on
//! Windows, hidapi (`windows.rs`).  Everything else is shared: which
//! interfaces are sticks, settling, and reading to a deadline.

mod backend;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

use std::io;
#[cfg(target_os = "linux")]
use std::os::fd::AsFd;
#[cfg(target_os = "linux")]
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use self::backend::Backend;
use self::backend::Bus;
use self::backend::Candidate;
use self::backend::ProductId;
use self::backend::UsbId;
use self::backend::VendorId;
#[cfg(target_os = "linux")]
use self::linux::Node;
#[cfg(windows)]
use self::windows::Node;
use crate::protocol::Report;
use crate::protocol::Stick;
use crate::protocol::Transport;

/// How long the stick needs after it is opened before the first command,
/// and the pause before every command after that.  ElfThing waits 2 s
/// after opening (`HIDTypeDevice` constructor) and 20 ms before each
/// write (`write(msg, delay2 = 20)`).
const SETTLE_AFTER_OPEN: Duration = Duration::from_secs(1);
const PAUSE_BEFORE_WRITE: Duration = Duration::from_millis(20);

/// The sticks' USB vendor ID, shared by the TEMPerGold and TEMPerHUM.
pub const VENDOR_ID: u16 = 0x3553;

/// The sticks' USB product ID, shared by the TEMPerGold and TEMPerHUM.
pub const PRODUCT_ID: u16 = 0xa001;

/// Sticks this crate supports; only the hardware on hand so far.
const SUPPORTED: &[UsbId] = &[UsbId {
    vendor: VendorId(VENDOR_ID),
    product: ProductId(PRODUCT_ID),
}];

/// The stick's data interface.  Interface 0 is a boot keyboard.  Its
/// usage page differs by model, so it is never matched on
/// (`docs/protocol.md`).
const DATA_INTERFACE: u8 = 1;

/// Whether `candidate` is a stick's data interface.  The bus rules out
/// the `temper-iio` daemon's virtual device, which has the same IDs.
fn is_stick_data_interface(candidate: &Candidate) -> bool {
    candidate.bus == Bus::Usb
        && SUPPORTED.contains(&candidate.id)
        && candidate.interface == Some(DATA_INTERFACE)
}

/// The stick data interfaces among `candidates`, sorted, each once.
fn stick_paths(candidates: Vec<Candidate>) -> Vec<PathBuf> {
    let mut found = candidates
        .into_iter()
        .filter(is_stick_data_interface)
        .map(|candidate| candidate.path)
        .collect::<Vec<_>>();
    found.sort();
    found.dedup();
    found
}

/// The one path among `found`.
fn only(found: Vec<PathBuf>) -> Result<PathBuf, Error> {
    match <[PathBuf; 1]>::try_from(found) {
        Ok([path]) => Ok(path),
        Err(found) if found.is_empty() => Err(Error::NotFound),
        Err(found) => Err(Error::Several(found)),
    }
}

/// The nodes of every attached stick's data interface, sorted.  Entries
/// that vanish during the scan are skipped.
///
/// # Errors
///
/// [`Error::Enumerate`] if the attached devices cannot be listed.
pub fn discover() -> Result<Vec<PathBuf>, Error> {
    let candidates = Node::enumerate().map_err(|source| Error::Enumerate { source })?;
    Ok(stick_paths(candidates))
}

/// Why a stick could not be found or opened.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The attached devices could not be listed.
    #[error("scanning for TEMPer sticks")]
    #[non_exhaustive]
    Enumerate {
        /// The underlying error.
        source: io::Error,
    },
    /// A node could not be opened, commonly for lack of permission.
    #[error("opening {}", path.display())]
    #[non_exhaustive]
    Open {
        /// The node.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// No stick is attached.
    #[error("no TEMPer stick found")]
    NotFound,
    /// More than one stick is attached; holds their nodes.
    #[error("several TEMPer sticks found: {}", display_paths(.0))]
    #[non_exhaustive]
    Several(Vec<PathBuf>),
}

/// `paths`, space-separated.
fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// An open stick data interface: a `/dev/hidrawN` node on Linux, a HID
/// device interface on Windows.
#[derive(Debug)]
pub struct Device {
    node: Node,
    path: PathBuf,
    /// When the stick may take its first command.
    settled: Instant,
}

impl Device {
    /// Opens a stick's data interface for reading and writing.
    ///
    /// # Errors
    ///
    /// [`Error::Open`] if the node cannot be opened.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let node = Node::open(path).map_err(|source| Error::Open {
            path: path.to_owned(),
            source,
        })?;
        Ok(Self {
            node,
            path: path.to_owned(),
            settled: Instant::now() + SETTLE_AFTER_OPEN,
        })
    }

    /// The node's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(target_os = "linux")]
impl AsFd for Device {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.node.as_fd()
    }
}

impl Stick<Device> {
    /// Opens the stick whose data interface is the node `path`.
    ///
    /// # Errors
    ///
    /// [`Error::Open`] if the node cannot be opened.
    pub fn open(path: &Path) -> Result<Self, Error> {
        Device::open(path).map(Self::new)
    }

    /// Opens the only attached stick.
    ///
    /// # Errors
    ///
    /// [`Error::Enumerate`], [`Error::NotFound`], [`Error::Several`], or
    /// [`Error::Open`].
    pub fn find() -> Result<Self, Error> {
        Self::open(&only(discover()?)?)
    }
}

/// `Stick<Device>` can move to and be shared with another thread, as the
/// `temper-iio` daemon does; a change that broke that would fail here.
/// `Sync` is Linux's: Windows promises only `Send` (hidapi's device is
/// not `Sync`).
#[cfg(target_os = "linux")]
const _: () = {
    const fn send_and_sync<T: Send + Sync>() {}
    send_and_sync::<Stick<Device>>();
};

/// One report from `backend` within `timeout`, retrying the backend's
/// early returns until then.  A `timeout` too long to represent waits
/// without limit.
fn receive<B: Backend>(backend: &mut B, timeout: Duration) -> io::Result<Option<Report>> {
    let deadline = Instant::now().checked_add(timeout);
    loop {
        let remaining = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
        match backend.read(remaining) {
            Ok(Some(report)) => return Ok(Some(report)),
            Ok(None) if remaining.is_some_and(|remaining| remaining.is_zero()) => return Ok(None),
            Ok(None) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// `Stick<Device>` can move to another thread on every platform.
const _: () = {
    const fn send<T: Send>() {}
    send::<Stick<Device>>();
};

impl Transport for Device {
    /// Waits for the stick to settle after opening, and briefly before
    /// every command.
    fn send(&mut self, report: &Report) -> io::Result<()> {
        thread::sleep(self.settled.saturating_duration_since(Instant::now()));
        thread::sleep(PAUSE_BEFORE_WRITE);
        self.node.write(report)
    }

    /// A `timeout` too long to represent waits without limit.
    fn receive(&mut self, timeout: Duration) -> io::Result<Option<Report>> {
        receive(&mut self.node, timeout)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    fn candidate(path: &str, bus: Bus, interface: Option<u8>) -> Candidate {
        Candidate {
            path: PathBuf::from(path),
            bus,
            id: SUPPORTED[0],
            interface,
        }
    }

    #[test]
    fn accepts_only_usb_data_interfaces_of_supported_sticks() {
        let other_device = Candidate {
            id: UsbId {
                vendor: VendorId(0x046d),
                product: ProductId(0xc52b),
            },
            ..candidate("/dev/hidraw4", Bus::Usb, Some(DATA_INTERFACE))
        };
        let candidates = vec![
            candidate("/dev/hidraw9", Bus::Usb, Some(DATA_INTERFACE)),
            candidate("/dev/hidraw8", Bus::Usb, Some(0)),
            candidate("/dev/hidraw0", Bus::Other, None),
            candidate("/dev/hidraw7", Bus::Usb, None),
            other_device,
        ];
        assert_eq!(stick_paths(candidates), [PathBuf::from("/dev/hidraw9")]);
    }

    #[test]
    fn sorts_and_deduplicates() {
        let candidates = vec![
            candidate("/dev/hidraw9", Bus::Usb, Some(DATA_INTERFACE)),
            candidate("/dev/hidraw2", Bus::Usb, Some(DATA_INTERFACE)),
            candidate("/dev/hidraw9", Bus::Usb, Some(DATA_INTERFACE)),
        ];
        assert_eq!(
            stick_paths(candidates),
            [PathBuf::from("/dev/hidraw2"), PathBuf::from("/dev/hidraw9")]
        );
    }

    #[test]
    fn only_one_stick() {
        let one = PathBuf::from("/dev/hidraw9");
        assert_eq!(only(vec![one.clone()]).unwrap(), one);
        assert!(matches!(only(Vec::new()), Err(Error::NotFound)));
        assert!(matches!(
            only(vec![one.clone(), one]),
            Err(Error::Several(found)) if found.len() == 2
        ));
    }

    /// What a [`Fake`] read returns, in order.
    #[derive(Debug)]
    enum Step {
        Report(Report),
        Early,
        Interrupted,
        Failed,
    }

    /// A backend that plays back reads.
    #[derive(Debug, Default)]
    struct Fake {
        steps: VecDeque<Step>,
    }

    impl Backend for Fake {
        fn enumerate() -> io::Result<Vec<Candidate>> {
            Ok(Vec::new())
        }

        fn open(_path: &Path) -> io::Result<Self> {
            Ok(Self::default())
        }

        fn write(&mut self, _report: &Report) -> io::Result<()> {
            Ok(())
        }

        fn read(&mut self, _timeout: Option<Duration>) -> io::Result<Option<Report>> {
            match self.steps.pop_front() {
                Some(Step::Report(report)) => Ok(Some(report)),
                Some(Step::Early) | None => Ok(None),
                Some(Step::Interrupted) => Err(io::ErrorKind::Interrupted.into()),
                Some(Step::Failed) => Err(io::ErrorKind::NotConnected.into()),
            }
        }
    }

    fn fake(steps: impl IntoIterator<Item = Step>) -> Fake {
        Fake {
            steps: steps.into_iter().collect(),
        }
    }

    const REPORT: Report = [0x80, 0x80, 0x0d, 0xb8, 0, 0, 0, 0];

    #[test]
    fn early_returns_and_interrupts_are_retried() {
        let mut backend = fake([Step::Early, Step::Interrupted, Step::Report(REPORT)]);
        assert_eq!(
            receive(&mut backend, Duration::from_secs(5)).unwrap(),
            Some(REPORT)
        );
    }

    #[test]
    fn nothing_by_the_deadline_is_none() {
        let mut backend = fake([]);
        assert_eq!(receive(&mut backend, Duration::ZERO).unwrap(), None);
        assert_eq!(
            receive(&mut backend, Duration::from_millis(5)).unwrap(),
            None
        );
    }

    #[test]
    fn errors_end_the_wait() {
        let mut backend = fake([Step::Failed, Step::Report(REPORT)]);
        let error = receive(&mut backend, Duration::from_secs(5)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotConnected);
    }
}
