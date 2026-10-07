//! Drives [`Stick`] through a third-party [`Transport`], using only the
//! public API: what an outside crate can do.

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use rustix::io::Errno;
use tempered_hid::protocol::CentiCelsius;
use tempered_hid::protocol::Error;
use tempered_hid::protocol::REPORT_LEN;
use tempered_hid::protocol::Report;
use tempered_hid::protocol::Stick;
use tempered_hid::protocol::Transport;

/// The temperature command's second byte, which its reply echoes first.
const TEMPERATURE_TAG: u8 = 0x80;

/// A stick that answers every command with the queued replies.
#[derive(Debug, Default)]
struct Scripted {
    /// Replies not yet read.
    replies: VecDeque<Report>,
    /// Commands sent.
    sent: Vec<Report>,
}

impl Transport for Scripted {
    fn send(&mut self, report: &Report) -> io::Result<()> {
        self.sent.push(*report);
        Ok(())
    }

    fn receive(&mut self, _timeout: Duration) -> io::Result<Option<Report>> {
        // Nothing is pending before a command, so draining finds nothing.
        if self.sent.is_empty() {
            return Ok(None);
        }
        Ok(self.replies.pop_front())
    }
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
        replies: VecDeque::from([temperature_reply(-1234)]),
        ..Scripted::default()
    };
    let mut stick = Stick::new(transport);
    let temperature = stick.temperature().unwrap();
    assert_eq!(temperature, CentiCelsius::new(-1234));
    assert_eq!(temperature.millicelsius(), -12_340);
    assert_eq!(stick.into_inner().sent.len(), 1);
}

#[test]
fn removed_transport_is_gone() {
    /// A transport whose device was unplugged.
    #[derive(Debug)]
    struct Unplugged;

    impl Transport for Unplugged {
        fn send(&mut self, _report: &Report) -> io::Result<()> {
            Err(Errno::NODEV.into())
        }

        fn receive(&mut self, _timeout: Duration) -> io::Result<Option<Report>> {
            Err(Errno::NODEV.into())
        }
    }

    assert!(matches!(
        Stick::new(Unplugged).temperature(),
        Err(Error::Gone)
    ));
}
