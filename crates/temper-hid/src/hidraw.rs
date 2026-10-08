//! Finding the stick's data interface under `/sys/class/hidraw` and
//! talking to it through its `/dev/hidrawN` node.

use std::fs;
use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::event::Timespec;
use rustix::io::Errno;

use crate::protocol::REPORT_LEN;
use crate::protocol::Report;
use crate::protocol::Stick;
use crate::protocol::Transport;

const SYSFS_HIDRAW: &str = "/sys/class/hidraw";
const DEV: &str = "/dev";

/// `BUS_USB` from `include/uapi/linux/input.h`.  The virtual device
/// the `temper-iio` daemon creates has the same VID:PID on
/// `BUS_VIRTUAL`, so the bus is what tells them apart.
const BUS_USB: Bus = Bus(0x0003);

/// `HID_PHYS` suffix of the stick's data interface, USB interface 1.
/// Interface 0 is a boot keyboard.
const DATA_INTERFACE_PHYS_SUFFIX: &str = "/input1";

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

/// usbhid strips a leading 0x00 report ID from writes
/// (`drivers/hid/usbhid/hid-core.c`); the data interface has no report
/// IDs, so every write carries one.
const REPORT_ID: u8 = 0x00;

/// A HID bus type, as in `HID_ID` and `include/uapi/linux/input.h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bus(u16);

/// A USB vendor ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VendorId(u16);

/// A USB product ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProductId(u16);

/// A USB vendor and product ID pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UsbId {
    vendor: VendorId,
    product: ProductId,
}

/// Sticks this crate supports; only the hardware on hand so far.
const SUPPORTED: &[UsbId] = &[UsbId {
    vendor: VendorId(VENDOR_ID),
    product: ProductId(PRODUCT_ID),
}];

/// The fields of a hidraw parent's `uevent` used for discovery.
#[derive(Debug, PartialEq, Eq)]
struct HidUevent {
    bus: Bus,
    id: UsbId,
    phys: String,
}

impl HidUevent {
    /// Parses `HID_ID=0003:00003553:0000A001` and `HID_PHYS=...` lines.
    fn parse(text: &str) -> Option<Self> {
        let value = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        };
        let mut fields = value("HID_ID")?.split(':');
        let mut hex = || u32::from_str_radix(fields.next()?, 16).ok();
        let bus = Bus(u16::try_from(hex()?).ok()?);
        let vendor = VendorId(u16::try_from(hex()?).ok()?);
        let product = ProductId(u16::try_from(hex()?).ok()?);
        Some(Self {
            bus,
            id: UsbId { vendor, product },
            phys: value("HID_PHYS")?.to_owned(),
        })
    }

    fn is_stick_data_interface(&self) -> bool {
        self.bus == BUS_USB
            && SUPPORTED.contains(&self.id)
            && self.phys.ends_with(DATA_INTERFACE_PHYS_SUFFIX)
    }
}

/// Whether an I/O error means a sysfs entry went away mid-scan, as an
/// unrelated hidraw device does when unplugged.
fn is_gone(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
        || error.raw_os_error() == Some(Errno::NODEV.raw_os_error())
}

/// The `/dev/hidrawN` nodes of every attached stick's data interface,
/// sorted.  Entries that vanish during the scan are skipped.
///
/// # Errors
///
/// [`Error::Scan`] if `/sys/class/hidraw` cannot be read.
pub fn discover() -> Result<Vec<PathBuf>, Error> {
    let scan = |source| Error::Scan { source };
    let mut found = Vec::new();
    for entry in fs::read_dir(SYSFS_HIDRAW).map_err(scan)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if is_gone(&error) => continue,
            Err(error) => return Err(scan(error)),
        };
        let uevent = match fs::read_to_string(entry.path().join("device/uevent")) {
            Ok(uevent) => uevent,
            Err(error) if is_gone(&error) => continue,
            Err(error) => return Err(scan(error)),
        };
        if HidUevent::parse(&uevent).is_some_and(|u| u.is_stick_data_interface()) {
            found.push(Path::new(DEV).join(entry.file_name()));
        }
    }
    found.sort();
    Ok(found)
}

/// Why a stick could not be found or opened.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `/sys/class/hidraw` could not be read.
    #[error("scanning for TEMPer sticks")]
    #[non_exhaustive]
    Scan {
        /// The underlying error.
        source: io::Error,
    },
    /// A hidraw node could not be opened, commonly for lack of
    /// permission.
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

/// An open hidraw node.
#[derive(Debug)]
pub struct Hidraw {
    file: File,
    path: PathBuf,
    /// When the stick may take its first command.
    settled: Instant,
}

impl Hidraw {
    /// Opens a hidraw node for reading and writing.
    ///
    /// # Errors
    ///
    /// [`Error::Open`] if the node cannot be opened.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let file = File::options()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|source| Error::Open {
                path: path.to_owned(),
                source,
            })?;
        Ok(Self {
            file,
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

impl AsFd for Hidraw {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

impl Stick<Hidraw> {
    /// Opens the stick whose data interface is the hidraw node `path`.
    ///
    /// # Errors
    ///
    /// [`Error::Open`] if the node cannot be opened.
    pub fn open(path: &Path) -> Result<Self, Error> {
        Hidraw::open(path).map(Self::new)
    }

    /// Opens the only attached stick.
    ///
    /// # Errors
    ///
    /// [`Error::Scan`], [`Error::NotFound`], [`Error::Several`], or
    /// [`Error::Open`].
    pub fn find() -> Result<Self, Error> {
        let found = discover()?;
        match found.as_slice() {
            [path] => Self::open(path),
            [] => Err(Error::NotFound),
            _ => Err(Error::Several(found)),
        }
    }
}

/// `Stick<Hidraw>` can move to and be shared with another thread, as the
/// `temper-iio` daemon does; a change that broke that would fail here.
const _: () = {
    const fn send_and_sync<T: Send + Sync>() {}
    send_and_sync::<Stick<Hidraw>>();
};

impl Transport for Hidraw {
    /// Waits for the stick to settle after opening, and briefly before
    /// every command.
    fn send(&mut self, report: &Report) -> io::Result<()> {
        thread::sleep(self.settled.saturating_duration_since(Instant::now()));
        thread::sleep(PAUSE_BEFORE_WRITE);
        let mut buffer = [REPORT_ID; 1 + REPORT_LEN];
        buffer[1..].copy_from_slice(report);
        self.file.write_all(&buffer)
    }

    /// A `timeout` too long to represent waits without limit.
    fn receive(&mut self, timeout: Duration) -> io::Result<Option<Report>> {
        let deadline = Instant::now().checked_add(timeout);
        loop {
            let remaining = deadline
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                .and_then(|remaining| Timespec::try_from(remaining).ok());
            let mut fds = [PollFd::new(&self.file, PollFlags::IN)];
            match rustix::event::poll(&mut fds, remaining.as_ref()) {
                Ok(0) => return Ok(None),
                Ok(_) => {}
                Err(Errno::INTR) => continue,
                Err(errno) => return Err(errno.into()),
            }
            // hidraw sets these only once the device is gone; a read
            // would then fail with EIO (drivers/hid/hidraw.c).
            if fds[0].revents().intersects(PollFlags::HUP | PollFlags::ERR) {
                return Err(Errno::NODEV.into());
            }
            let mut report = [0; REPORT_LEN];
            let n = match self.file.read(&mut report) {
                Ok(n) => n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if n != REPORT_LEN {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("hidraw report of {n} bytes, expected {REPORT_LEN}"),
                ));
            }
            return Ok(Some(report));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The data interface's `uevent`, from a stick on this bench.
    const DATA_INTERFACE: &str = "DRIVER=hid-generic
HID_ID=0003:00003553:0000A001
HID_NAME=PCsensor TEMPerGold
HID_PHYS=usb-0000:00:14.0-1.3/input1
HID_UNIQ=
MODALIAS=hid:b0003g0001v00003553p0000A001
";

    #[test]
    fn parses_uevent() {
        assert_eq!(
            HidUevent::parse(DATA_INTERFACE),
            Some(HidUevent {
                bus: BUS_USB,
                id: SUPPORTED[0],
                phys: "usb-0000:00:14.0-1.3/input1".to_owned(),
            })
        );
    }

    #[test]
    fn accepts_data_interface() {
        let uevent = HidUevent::parse(DATA_INTERFACE).unwrap();
        assert!(uevent.is_stick_data_interface());
    }

    #[test]
    fn rejects_keyboard_interface() {
        let text = DATA_INTERFACE.replace("/input1", "/input0");
        let uevent = HidUevent::parse(&text).unwrap();
        assert!(!uevent.is_stick_data_interface());
    }

    #[test]
    fn rejects_virtual_device() {
        let text = DATA_INTERFACE.replace("HID_ID=0003", "HID_ID=0006");
        let uevent = HidUevent::parse(&text).unwrap();
        assert!(!uevent.is_stick_data_interface());
    }

    #[test]
    fn rejects_other_device() {
        let text = DATA_INTERFACE.replace("00003553", "0000046D");
        let uevent = HidUevent::parse(&text).unwrap();
        assert!(!uevent.is_stick_data_interface());
    }

    #[test]
    fn rejects_malformed_uevent() {
        assert_eq!(HidUevent::parse("HID_ID=junk\nHID_PHYS=x\n"), None);
        assert_eq!(HidUevent::parse("HID_PHYS=x\n"), None);
    }
}
