//! Finding the stick's data interface under `/sys/class/hidraw` and
//! talking to it through its `/dev/hidrawN` node.

use std::fs;
use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::event::Timespec;

use crate::temper::REPORT_LEN;
use crate::temper::Report;
use crate::temper::Transport;

const SYSFS_HIDRAW: &str = "/sys/class/hidraw";
const DEV: &str = "/dev";

/// `BUS_USB` from `include/uapi/linux/input.h`.  The virtual device
/// `tempered` creates has the same VID:PID on `BUS_VIRTUAL`, so the bus
/// is what tells them apart.
const BUS_USB: u16 = 0x0003;

/// `HID_PHYS` suffix of the stick's data interface, USB interface 1.
/// Interface 0 is a boot keyboard.
const DATA_INTERFACE_PHYS_SUFFIX: &str = "/input1";

/// usbhid strips a leading 0x00 report ID from writes
/// (`drivers/hid/usbhid/hid-core.c`); the data interface has no report
/// IDs, so every write carries one.
const REPORT_ID: u8 = 0x00;

/// A USB vendor and product ID pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UsbId {
    vendor: u16,
    product: u16,
}

/// Sticks this program supports; only the hardware on hand so far.
const SUPPORTED: &[UsbId] = &[UsbId {
    vendor: 0x3553,
    product: 0xa001,
}];

/// The fields of a hidraw parent's `uevent` used for discovery.
#[derive(Debug, PartialEq, Eq)]
struct HidUevent {
    bus: u16,
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
        let bus = u16::try_from(hex()?).ok()?;
        let vendor = u16::try_from(hex()?).ok()?;
        let product = u16::try_from(hex()?).ok()?;
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

/// The `/dev/hidrawN` nodes of every attached stick's data interface.
pub fn discover() -> io::Result<Vec<PathBuf>> {
    let mut found = fs::read_dir(SYSFS_HIDRAW)?
        .map(|entry| {
            let entry = entry?;
            let uevent = fs::read_to_string(entry.path().join("device/uevent"))?;
            let is_stick = HidUevent::parse(&uevent).is_some_and(|u| u.is_stick_data_interface());
            Ok(is_stick.then(|| Path::new(DEV).join(entry.file_name())))
        })
        .filter_map(Result::transpose)
        .collect::<io::Result<Vec<_>>>()?;
    found.sort();
    Ok(found)
}

/// An open hidraw node.
#[derive(Debug)]
pub struct Hidraw {
    file: File,
}

impl Hidraw {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::options().read(true).write(true).open(path)?;
        Ok(Self { file })
    }
}

impl Transport for Hidraw {
    fn send(&mut self, report: &Report) -> io::Result<()> {
        let mut buffer = [REPORT_ID; 1 + REPORT_LEN];
        buffer[1..].copy_from_slice(report);
        self.file.write_all(&buffer)
    }

    fn receive(&mut self, timeout: Duration) -> io::Result<Option<Report>> {
        let timeout = Timespec::try_from(timeout).map_err(io::Error::other)?;
        let mut fds = [PollFd::new(&self.file, PollFlags::IN)];
        if rustix::event::poll(&mut fds, Some(&timeout))? == 0 {
            return Ok(None);
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
                bus: BUS_USB,
                id: UsbId {
                    vendor: 0x3553,
                    product: 0xa001
                },
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
