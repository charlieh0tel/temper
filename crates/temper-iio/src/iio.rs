//! Finding the IIO devices the kernel made for a uhid device, and any
//! other HID sensor of the same kind.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::sensor::Quantity;

const IIO_DEVICES: &str = "/sys/bus/iio/devices";

const PLATFORM_DEVICES: &str = "/sys/bus/platform/devices";

/// IIO device entries; the bus also lists `triggerN` entries, and the
/// HID sensor driver registers a trigger under the same parent.
const IIO_DEVICE_PREFIX: &str = "iio:device";

/// Whether the device at `dir` has `HID_UNIQ=<uniq>` in its `uevent`.
pub(crate) fn has_uniq(dir: &Path, uniq: &str) -> bool {
    let line = format!("HID_UNIQ={uniq}");
    fs::read_to_string(dir.join("uevent")).is_ok_and(|uevent| uevent.lines().any(|l| l == line))
}

/// The resolved sysfs path of the IIO device called `name` whose
/// ancestors include the HID device created with `uniq`, if it exists
/// yet.
pub(crate) fn find(uniq: &str, name: &str) -> Option<PathBuf> {
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
        let named = fs::read_to_string(path.join("name")).is_ok_and(|n| n.trim() == name);
        (named && path.ancestors().any(|dir| has_uniq(dir, uniq))).then_some(path)
    })
}

/// Whether the HID sensor at `platform` belongs to a uhid
/// device with `HID_PHYS=<phys>`: its parent's `uevent` says so.
pub(crate) fn sensor_has_phys(platform: &Path, phys: &str) -> bool {
    let line = format!("HID_PHYS={phys}");
    fs::canonicalize(platform).is_ok_and(|path| {
        path.parent().is_some_and(|hid| {
            fs::read_to_string(hid.join("uevent"))
                .is_ok_and(|uevent| uevent.lines().any(|l| l == line))
        })
    })
}

/// The platform device of a HID sensor of `quantity`, if any exists.
///
/// `hid-sensor-temperature` and `hid-sensor-humidity` each keep one
/// static callback struct for all their instances and overwrite its
/// `pdev` on every probe, so with two sensors, reports for one reach
/// the other's device; once that one is removed, the kernel
/// dereferences NULL (`temperature_capture_sample()` in
/// `drivers/iio/temperature/hid-sensor-temperature.c`).  So the
/// daemon's sensor must be the only one of its kind.
pub(crate) fn sensor(quantity: Quantity) -> Option<PathBuf> {
    let prefix = quantity.platform_prefix();
    fs::read_dir(PLATFORM_DEVICES).ok()?.find_map(|entry| {
        let entry = entry.ok()?;
        entry
            .file_name()
            .to_string_lossy()
            .starts_with(&prefix)
            .then(|| entry.path())
    })
}
