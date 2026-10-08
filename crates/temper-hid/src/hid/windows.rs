//! The Windows backend, over hidapi's pure-Rust Windows code
//! (`windows-native`).  Untested on hardware: see `PLAN.md`.

use std::ffi::CString;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use hidapi::BusType;
use hidapi::HidApi;
use hidapi::HidDevice;
use hidapi::HidError;

use super::backend::Backend;
use super::backend::Bus;
use super::backend::Candidate;
use super::backend::ProductId;
use super::backend::UsbId;
use super::backend::VendorId;
use crate::protocol::REPORT_LEN;
use crate::protocol::Report;

/// hidapi takes the report ID first in every write; the data interface
/// has none, so it is 0 (hidapi `windows_native/mod.rs` pads the
/// buffer to the interface's output report length).
const REPORT_ID: u8 = 0x00;

/// hidapi's timeout for a read without limit.
const NO_TIMEOUT: i32 = -1;

/// Win32 errors a removed device gives, from `winerror.h`: the device
/// is gone (`ERROR_DEVICE_NOT_CONNECTED`), its pending I/O was cancelled
/// (`ERROR_OPERATION_ABORTED`), or its driver no longer takes requests
/// (`ERROR_BAD_COMMAND`).  Which of these an unplug gives is unverified.
const REMOVAL_ERRORS: [i32; 3] = [1167, 995, 22];

/// `error` as an `io::Error`, with removal as kind `NotConnected`.  hidapi
/// keeps Win32 errors as raw OS errors (`windows_native/error.rs`).
fn io_error(error: HidError) -> io::Error {
    match error {
        HidError::IoError { error } => match error.raw_os_error() {
            Some(code) if REMOVAL_ERRORS.contains(&code) => {
                io::Error::new(io::ErrorKind::NotConnected, error)
            }
            _ => error,
        },
        other => io::Error::other(other),
    }
}

/// `path` as hidapi takes it: UTF-8 with no NUL, which hidapi would
/// otherwise panic on (`windows_native/mod.rs`).
fn hidapi_path(path: &Path) -> io::Result<CString> {
    let text = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "device path is not UTF-8"))?;
    CString::new(text)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "device path contains NUL"))
}

/// `timeout` in hidapi's milliseconds, rounded up so a short wait still
/// waits; `None` or one too long for an `i32` waits without limit.
fn timeout_millis(timeout: Option<Duration>) -> i32 {
    timeout
        .and_then(|timeout| {
            let millis = timeout.as_nanos().div_ceil(1_000_000);
            i32::try_from(millis).ok()
        })
        .unwrap_or(NO_TIMEOUT)
}

/// The candidate hidapi describes, if its path is usable.
fn candidate(info: &hidapi::DeviceInfo) -> Option<Candidate> {
    Some(Candidate {
        path: PathBuf::from(info.path().to_str().ok()?),
        bus: match info.bus_type() {
            BusType::Usb => Bus::Usb,
            _ => Bus::Other,
        },
        id: UsbId {
            vendor: VendorId(info.vendor_id()),
            product: ProductId(info.product_id()),
        },
        interface: u8::try_from(info.interface_number()).ok(),
    })
}

/// An open HID interface.
#[derive(Debug)]
pub(super) struct Node {
    device: HidDevice,
}

impl Backend for Node {
    fn enumerate() -> io::Result<Vec<Candidate>> {
        let api = HidApi::new().map_err(io_error)?;
        Ok(api.device_list().filter_map(candidate).collect())
    }

    fn open(path: &Path) -> io::Result<Self> {
        let path = hidapi_path(path)?;
        let api = HidApi::new().map_err(io_error)?;
        let device = api.open_path(&path).map_err(io_error)?;
        Ok(Self { device })
    }

    /// hidapi reports 0 for a write that completed at once
    /// (`windows_native/mod.rs`), so only a short nonzero count is short.
    fn write(&mut self, report: &Report) -> io::Result<()> {
        let mut buffer = [REPORT_ID; 1 + REPORT_LEN];
        buffer[1..].copy_from_slice(report);
        let written = self.device.write(&buffer).map_err(io_error)?;
        if written != 0 && written < buffer.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("wrote {written} of {} bytes", buffer.len()),
            ));
        }
        Ok(())
    }

    /// hidapi strips the 0x00 report ID Windows puts before every report
    /// of a device without report IDs (`windows_native/mod.rs`).
    fn read(&mut self, timeout: Option<Duration>) -> io::Result<Option<Report>> {
        let mut report = [0; REPORT_LEN];
        let n = self
            .device
            .read_timeout(&mut report, timeout_millis(timeout))
            .map_err(io_error)?;
        match n {
            0 => Ok(None),
            REPORT_LEN => Ok(Some(report)),
            n => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("HID report of {n} bytes, expected {REPORT_LEN}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_round_up_to_milliseconds() {
        assert_eq!(timeout_millis(Some(Duration::ZERO)), 0);
        assert_eq!(timeout_millis(Some(Duration::from_micros(1))), 1);
        assert_eq!(timeout_millis(Some(Duration::from_millis(500))), 500);
        assert_eq!(timeout_millis(None), NO_TIMEOUT);
        assert_eq!(timeout_millis(Some(Duration::MAX)), NO_TIMEOUT);
    }

    #[test]
    fn removal_errors_are_not_connected() {
        for code in REMOVAL_ERRORS {
            let error = io_error(HidError::IoError {
                error: io::Error::from_raw_os_error(code),
            });
            assert_eq!(error.kind(), io::ErrorKind::NotConnected);
        }
        let other = io_error(HidError::IoError {
            error: io::Error::from_raw_os_error(5),
        });
        assert_ne!(other.kind(), io::ErrorKind::NotConnected);
    }

    #[test]
    fn paths_hidapi_cannot_take_are_rejected() {
        assert!(hidapi_path(Path::new(r"\\?\hid#vid_3553&pid_a001&mi_01#x")).is_ok());
        let error = hidapi_path(Path::new("bad\0path")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
