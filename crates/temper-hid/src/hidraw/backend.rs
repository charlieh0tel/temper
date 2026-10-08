//! What a platform must provide to reach a stick: one private trait, so
//! the platform code stays small and the rest of the module is shared.

use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use crate::protocol::Report;

/// The bus a HID device sits on, as far as discovery cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Bus {
    /// A real USB device.
    Usb,
    /// Anything else, such as the `temper-iio` daemon's virtual device,
    /// which reuses the stick's IDs.
    Other,
}

/// A USB vendor ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VendorId(pub(super) u16);

/// A USB product ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProductId(pub(super) u16);

/// A USB vendor and product ID pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct UsbId {
    pub(super) vendor: VendorId,
    pub(super) product: ProductId,
}

/// A HID interface found by [`Backend::enumerate`]: what discovery
/// needs to decide whether it is a stick's data interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Candidate {
    /// What [`Backend::open`] takes.
    pub(super) path: PathBuf,
    pub(super) bus: Bus,
    pub(super) id: UsbId,
    /// The USB interface number, if the platform reports one.
    pub(super) interface: Option<u8>,
}

/// One platform's way to reach a stick's data interface.
pub(super) trait Backend: Sized + Send {
    /// Every HID interface attached.  Ones that vanish during the scan
    /// are skipped.
    fn enumerate() -> io::Result<Vec<Candidate>>;

    /// Opens a data interface for reading and writing.
    fn open(path: &Path) -> io::Result<Self>;

    /// Writes one report, behind the 0x00 report ID, all of it or an
    /// error.  Removal is `ENODEV` or kind `NotConnected`.
    fn write(&mut self, report: &Report) -> io::Result<()>;

    /// Waits at most `timeout` (`None`: without limit) for one report.
    /// May return early, with `None` or an error of kind `Interrupted`;
    /// the caller retries until its deadline.  Removal is `ENODEV` or
    /// kind `NotConnected`.
    fn read(&mut self, timeout: Option<Duration>) -> io::Result<Option<Report>>;
}
