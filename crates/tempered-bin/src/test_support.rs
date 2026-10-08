//! Shared by the tests.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::MutexGuard;

use crate::iio;
use crate::mutex::lock;
use crate::sensor::Quantity;

/// Serializes tests that create HID temperature or humidity sensors:
/// the kernel's drivers cannot handle two of a kind at once (see
/// `iio::sensor`), and doing so oopses the kernel.
static SENSOR: Mutex<()> = Mutex::new(());

/// Holds the right to create HID temperature and humidity sensors, once
/// no other exists, such as one from a running `tempered@` service.
pub(crate) fn one_sensor() -> MutexGuard<'static, ()> {
    let guard = lock(&SENSOR);
    for quantity in [Quantity::Temperature, Quantity::Humidity] {
        if let Some(other) = iio::sensor(quantity) {
            panic!(
                "a HID {} sensor already exists ({}); stop it first, \
                 e.g. systemctl stop 'tempered@*'",
                quantity.iio_name(),
                other.display()
            );
        }
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
