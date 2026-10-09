//! Read a PCsensor TEMPerGold or TEMPer2 USB thermometer or TEMPerHUM
//! thermometer and hygrometer, on Linux (hidraw) and Windows.
//!
//! [`protocol`] encodes the stick's queries and decodes its replies over
//! any [`protocol::Transport`]; [`hid`] finds the stick and provides
//! the transport to it (Linux and Windows); [`schedule`] paces repeated
//! queries.  `protocol` and `schedule` are portable.
//!
//! ```no_run
//! # #[cfg(any(target_os = "linux", windows))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use temper_hid::protocol::Stick;
//!
//! let mut stick = Stick::find()?;
//! let reading = stick.reading()?;
//! println!("{} C", reading.temperature);
//! if let Some(humidity) = reading.humidity {
//!     println!("{humidity} %RH");
//! }
//! if let Some(outer) = reading.outer_temperature {
//!     println!("{outer} C outer probe");
//! }
//! # Ok(())
//! # }
//! # #[cfg(not(any(target_os = "linux", windows)))]
//! # fn main() {}
//! ```

#![warn(missing_docs)]

#[cfg(any(target_os = "linux", windows))]
pub mod hid;
pub mod protocol;
pub mod schedule;
