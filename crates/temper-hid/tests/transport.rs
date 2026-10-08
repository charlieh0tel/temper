//! Drives [`Stick`] through a third-party [`Transport`], using only the
//! public API: what an outside crate can do.

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use temper_hid::protocol::Celsius;
use temper_hid::protocol::Error;
use temper_hid::protocol::REPORT_LEN;
use temper_hid::protocol::Report;
use temper_hid::protocol::Stick;
use temper_hid::protocol::Transport;

/// The temperature command's second byte, which its reply echoes first.
const TEMPERATURE_TAG: u8 = 0x80;

/// The firmware reply, two reports of NUL-padded ASCII.
const FIRMWARE: &[u8; 2 * REPORT_LEN] = b"TEMPerGold_V3.5\0";

/// A stick that answers each command with the next scripted reply.
#[derive(Debug, Default)]
struct Scripted {
    /// Replies to commands not yet sent, in order.
    replies: VecDeque<Vec<Report>>,
    /// The reply to the last command, not yet read.
    pending: VecDeque<Report>,
    /// Commands sent.
    sent: Vec<Report>,
}

impl Transport for Scripted {
    fn send(&mut self, report: &Report) -> io::Result<()> {
        self.sent.push(*report);
        self.pending = self.replies.pop_front().unwrap_or_default().into();
        Ok(())
    }

    fn receive(&mut self, _timeout: Duration) -> io::Result<Option<Report>> {
        Ok(self.pending.pop_front())
    }
}

fn firmware_reply() -> Vec<Report> {
    FIRMWARE.as_chunks::<REPORT_LEN>().0.to_vec()
}

fn temperature_reply(centi: i16) -> Report {
    let [high, low] = centi.to_be_bytes();
    let mut report = [0; REPORT_LEN];
    report[..4].copy_from_slice(&[TEMPERATURE_TAG, TEMPERATURE_TAG, high, low]);
    report
}

#[test]
fn reads_temperature_through_own_transport() {
    let transport = Scripted {
        replies: VecDeque::from([firmware_reply(), vec![temperature_reply(-1234)]]),
        ..Scripted::default()
    };
    let mut stick = Stick::new(transport);
    let reading = stick.reading().unwrap();
    assert_eq!(reading.temperature, Celsius::new(-12.34));
    assert_eq!(reading.humidity, None);
    assert_eq!(stick.into_inner().sent.len(), 2);
}

#[test]
fn removed_transport_is_gone() {
    /// A transport whose device was unplugged.
    #[derive(Debug)]
    struct Unplugged;

    impl Transport for Unplugged {
        fn send(&mut self, _report: &Report) -> io::Result<()> {
            Err(io::ErrorKind::NotConnected.into())
        }

        fn receive(&mut self, _timeout: Duration) -> io::Result<Option<Report>> {
            Err(io::ErrorKind::NotConnected.into())
        }
    }

    assert!(matches!(Stick::new(Unplugged).reading(), Err(Error::Gone)));
}
