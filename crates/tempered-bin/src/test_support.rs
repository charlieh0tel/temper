//! Shared by the root-only kernel tests.

use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;

/// Serializes tests that create a HID temperature sensor: the kernel's
/// `hid-sensor-temperature` cannot handle two at once (see
/// `iio::temperature_sensor`), and doing so oopses the kernel.
static TEMPERATURE_SENSOR: Mutex<()> = Mutex::new(());

/// Holds the right to create a HID temperature sensor.
pub(crate) fn one_temperature_sensor() -> MutexGuard<'static, ()> {
    TEMPERATURE_SENSOR
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}
