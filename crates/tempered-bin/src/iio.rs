//! Finding the IIO device the kernel made for a uhid device.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

const IIO_DEVICES: &str = "/sys/bus/iio/devices";

/// IIO device entries; the bus also lists `triggerN` entries, and the
/// HID sensor driver registers a trigger under the same parent.
const IIO_DEVICE_PREFIX: &str = "iio:device";

/// Whether the device at `dir` has `HID_UNIQ=<uniq>` in its `uevent`.
fn has_uniq(dir: &Path, uniq: &str) -> bool {
    let line = format!("HID_UNIQ={uniq}");
    fs::read_to_string(dir.join("uevent")).is_ok_and(|uevent| uevent.lines().any(|l| l == line))
}

/// The resolved sysfs path of the IIO device whose ancestors include
/// the HID device created with `uniq`, if it exists yet.
pub(crate) fn find(uniq: &str) -> Option<PathBuf> {
    fs::read_dir(IIO_DEVICES).ok()?.find_map(|entry| {
        let entry = entry.ok()?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(IIO_DEVICE_PREFIX)
        {
            return None;
        }
        let path = fs::canonicalize(entry.path()).ok()?;
        path.ancestors()
            .any(|dir| has_uniq(dir, uniq))
            .then_some(path)
    })
}
