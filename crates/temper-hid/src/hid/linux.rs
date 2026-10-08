//! The Linux backend: discovery under `/sys/class/hidraw`, and the
//! `/dev/hidrawN` node read with `poll(2)`.

use std::fs;
use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::time::Duration;

use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::event::Timespec;
use rustix::io::Errno;

use super::backend::Backend;
use super::backend::Bus;
use super::backend::Candidate;
use super::backend::ProductId;
use super::backend::UsbId;
use super::backend::VendorId;
use crate::protocol::REPORT_LEN;
use crate::protocol::Report;

const SYSFS_HIDRAW: &str = "/sys/class/hidraw";
const DEV: &str = "/dev";

/// `BUS_USB` from `include/uapi/linux/input.h`.
const BUS_USB: u16 = 0x0003;

/// `HID_PHYS` of a USB HID interface ends `/input<N>`, N its interface
/// number (`drivers/hid/usbhid/hid-core.c`, `usbhid_probe()`).
const INTERFACE_PHYS_PREFIX: &str = "/input";

/// usbhid strips a leading 0x00 report ID from writes
/// (`drivers/hid/usbhid/hid-core.c`); the data interface has no report
/// IDs, so every write carries one.
const REPORT_ID: u8 = 0x00;

/// The fields of a hidraw parent's `uevent` used for discovery.
#[derive(Debug, PartialEq, Eq)]
struct HidUevent {
    bus: Bus,
    id: UsbId,
    interface: Option<u8>,
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
        let bus = u16::try_from(hex()?).ok()?;
        let vendor = VendorId(u16::try_from(hex()?).ok()?);
        let product = ProductId(u16::try_from(hex()?).ok()?);
        let phys = value("HID_PHYS")?;
        Some(Self {
            bus: if bus == BUS_USB { Bus::Usb } else { Bus::Other },
            id: UsbId { vendor, product },
            interface: phys
                .rsplit_once(INTERFACE_PHYS_PREFIX)
                .and_then(|(_, number)| number.parse().ok()),
        })
    }
}

/// Whether an I/O error means a sysfs entry went away mid-scan, as an
/// unrelated hidraw device does when unplugged.
fn is_gone(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
        || error.raw_os_error() == Some(Errno::NODEV.raw_os_error())
}

/// The error a removed device is reported as.
fn removed() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "device removed")
}

/// An open `/dev/hidrawN` node.
#[derive(Debug)]
pub(super) struct Node {
    file: File,
}

impl AsFd for Node {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

impl Backend for Node {
    fn enumerate() -> io::Result<Vec<Candidate>> {
        let mut found = Vec::new();
        for entry in fs::read_dir(SYSFS_HIDRAW)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if is_gone(&error) => continue,
                Err(error) => return Err(error),
            };
            let uevent = match fs::read_to_string(entry.path().join("device/uevent")) {
                Ok(uevent) => uevent,
                Err(error) if is_gone(&error) => continue,
                Err(error) => return Err(error),
            };
            if let Some(uevent) = HidUevent::parse(&uevent) {
                found.push(Candidate {
                    path: Path::new(DEV).join(entry.file_name()),
                    bus: uevent.bus,
                    id: uevent.id,
                    interface: uevent.interface,
                });
            }
        }
        Ok(found)
    }

    fn open(path: &Path) -> io::Result<Self> {
        let file = File::options().read(true).write(true).open(path)?;
        Ok(Self { file })
    }

    /// A write to a removed device fails with `ENODEV`
    /// (`drivers/hid/hidraw.c`), reported as `NotConnected`.
    fn write(&mut self, report: &Report) -> io::Result<()> {
        let mut buffer = [REPORT_ID; 1 + REPORT_LEN];
        buffer[1..].copy_from_slice(report);
        self.file.write_all(&buffer).map_err(|error| {
            if error.raw_os_error() == Some(Errno::NODEV.raw_os_error()) {
                removed()
            } else {
                error
            }
        })
    }

    /// A `timeout` too long to represent waits without limit.
    fn read(&mut self, timeout: Option<Duration>) -> io::Result<Option<Report>> {
        let timeout = timeout.and_then(|timeout| Timespec::try_from(timeout).ok());
        let mut fds = [PollFd::new(&self.file, PollFlags::IN)];
        if rustix::event::poll(&mut fds, timeout.as_ref())? == 0 {
            return Ok(None);
        }
        // hidraw sets these only once the device is gone; a read would
        // then fail with EIO (drivers/hid/hidraw.c).
        if fds[0].revents().intersects(PollFlags::HUP | PollFlags::ERR) {
            return Err(removed());
        }
        let mut report = [0; REPORT_LEN];
        let n = self.file.read(&mut report)?;
        if n != REPORT_LEN {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("hidraw report of {n} bytes, expected {REPORT_LEN}"),
            ));
        }
        Ok(Some(report))
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
                bus: Bus::Usb,
                id: UsbId {
                    vendor: VendorId(0x3553),
                    product: ProductId(0xa001),
                },
                interface: Some(1),
            })
        );
    }

    #[test]
    fn parses_keyboard_interface() {
        let text = DATA_INTERFACE.replace("/input1", "/input0");
        assert_eq!(HidUevent::parse(&text).unwrap().interface, Some(0));
    }

    #[test]
    fn virtual_device_is_not_usb() {
        let text = DATA_INTERFACE
            .replace("HID_ID=0003", "HID_ID=0006")
            .replace("usb-0000:00:14.0-1.3/input1", "temper-iio");
        let uevent = HidUevent::parse(&text).unwrap();
        assert_eq!(uevent.bus, Bus::Other);
        assert_eq!(uevent.interface, None);
    }

    #[test]
    fn rejects_malformed_uevent() {
        assert_eq!(HidUevent::parse("HID_ID=junk\nHID_PHYS=x\n"), None);
        assert_eq!(HidUevent::parse("HID_PHYS=x\n"), None);
    }
}
