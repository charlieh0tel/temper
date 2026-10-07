//! Read a PCsensor TEMPerGold USB thermometer over Linux hidraw.
//!
//! [`protocol`] encodes the stick's queries and decodes its replies over
//! any [`protocol::Transport`]; [`hidraw`] finds the stick and provides
//! the hidraw transport.  Linux only.
//!
//! ```no_run
//! use tempered_hid::protocol::Stick;
//!
//! let mut stick = Stick::find()?;
//! println!("{} C", stick.temperature()?);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![warn(missing_docs)]

pub mod hidraw;
pub mod protocol;
