//! Read a PCsensor TEMPerGold USB thermometer or TEMPerHUM
//! thermometer and hygrometer over Linux hidraw.
//!
//! [`protocol`] encodes the stick's queries and decodes its replies over
//! any [`protocol::Transport`]; [`hidraw`] finds the stick and provides
//! the hidraw transport (Linux only); [`schedule`] paces repeated
//! queries.  `protocol` and `schedule` are portable.
//!
//! ```no_run
//! # #[cfg(target_os = "linux")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use temper_hid::protocol::Stick;
//!
//! let mut stick = Stick::find()?;
//! let reading = stick.reading()?;
//! println!("{} C", reading.temperature);
//! if let Some(humidity) = reading.humidity {
//!     println!("{humidity} %RH");
//! }
//! # Ok(())
//! # }
//! # #[cfg(not(target_os = "linux"))]
//! # fn main() {}
//! ```

#![warn(missing_docs)]

#[cfg(target_os = "linux")]
pub mod hidraw;
pub mod protocol;
pub mod schedule;
