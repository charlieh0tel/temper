//! Read a PCsensor TEMPerGold USB thermometer or TEMPerHUM
//! thermometer and hygrometer over Linux hidraw.
//!
//! [`protocol`] encodes the stick's queries and decodes its replies over
//! any [`protocol::Transport`]; [`hidraw`] finds the stick and provides
//! the hidraw transport.  Linux only.
//!
//! ```no_run
//! use temper_hid::protocol::Stick;
//!
//! let mut stick = Stick::find()?;
//! let reading = stick.reading()?;
//! println!("{} C", reading.temperature);
//! if let Some(humidity) = reading.humidity {
//!     println!("{humidity} %RH");
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![warn(missing_docs)]

pub mod hidraw;
pub mod protocol;
