//! Shared by the tests.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::MutexGuard;

use crate::iio;
use crate::mutex::lock;

/// Serializes tests that create a HID temperature sensor: the kernel's
/// `hid-sensor-temperature` cannot handle two at once (see
/// `iio::temperature_sensor`), and doing so oopses the kernel.
static TEMPERATURE_SENSOR: Mutex<()> = Mutex::new(());

/// Holds the right to create a HID temperature sensor, once no other
/// exists, such as one from a running `tempered@` service.
pub(crate) fn one_temperature_sensor() -> MutexGuard<'static, ()> {
    let guard = lock(&TEMPERATURE_SENSOR);
    if let Some(other) = iio::temperature_sensor() {
        panic!(
            "a HID temperature sensor already exists ({}); stop it first, \
             e.g. systemctl stop 'tempered@*'",
            other.display()
        );
    }
    guard
}

/// An environment lookup over fixed `(name, value)` pairs.
pub(crate) fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    move |name| map.get(name).cloned()
}
