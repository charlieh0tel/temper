//! TEMPerGold HID protocol: the queries the stick answers and how to
//! decode its replies.
//!
//! Sources: PCsensor's ElfThing 1.0.2 app (`resources/app.asar`, class
//! `HIDTypeDevice`; see `PLAN.md` for the download and its hash), and
//! urwen/temper `temper.py`.  Reply layouts were confirmed against
//! captures from a `TEMPerGold_V3.5` stick in `tests/fixtures/`.

use std::fmt;
use std::io;
use std::time::Duration;

/// Length of every report in either direction.
pub const REPORT_LEN: usize = 8;

/// One report to or from the stick's data interface.
pub type Report = [u8; REPORT_LEN];

/// How long to wait for each reply report (ElfThing `readTimeout(500)`).
const REPLY_TIMEOUT: Duration = Duration::from_millis(500);

/// Upper bound on stale reports drained before a query, so a stick
/// that never stops sending cannot stall the caller forever.
const MAX_STALE_REPORTS: usize = 16;

/// Firmware string prefix of the sticks this module decodes.
const SUPPORTED_FIRMWARE_PREFIX: &str = "TEMPerGold_";

/// Sensor range in centi-degrees C (ElfThing `parseModel`,
/// `innerTemperatureCRangeMin`/`Max`: -40 to 125 degrees C).
const TEMPERATURE_RANGE: std::ops::RangeInclusive<i16> = -4000..=12500;

/// The 2000-based year in the manufacture date reply.
const MANUFACTURE_YEAR_BASE: u16 = 2000;

/// A query the stick answers.  Every reply except `Firmware`'s echoes
/// the command's second byte as its first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Firmware,
    Temperature,
    SensorType,
    Calibration,
    ManufactureDate,
}

impl Command {
    /// The bytes sent, from ElfThing's `read*Cmd` arrays.  Firmware uses
    /// urwen/temper's `01 86 ff 01`; ElfThing sends `01 86 ff 00`.
    fn bytes(self) -> Report {
        match self {
            Self::Firmware => [0x01, 0x86, 0xff, 0x01, 0, 0, 0, 0],
            Self::Temperature => [0x01, 0x80, 0x33, 0x01, 0, 0, 0, 0],
            Self::SensorType => [0x01, 0x87, 0xee, 0x00, 0, 0, 0, 0],
            Self::Calibration => [0x01, 0x82, 0x77, 0x01, 0, 0, 0, 0],
            Self::ManufactureDate => [0x01, 0x8a, 0x00, 0x00, 0, 0, 0, 0],
        }
    }

    /// Number of reports in the reply.  The firmware string spans two.
    fn reply_reports(self) -> usize {
        match self {
            Self::Firmware => 2,
            Self::Temperature | Self::SensorType | Self::Calibration | Self::ManufactureDate => 1,
        }
    }

    /// First byte of a well-formed reply, if the reply is tagged.
    fn reply_tag(self) -> Option<u8> {
        match self {
            Self::Firmware => None,
            Self::Temperature | Self::SensorType | Self::Calibration | Self::ManufactureDate => {
                Some(self.bytes()[1])
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("stick I/O")]
    Io(#[from] io::Error),
    #[error("too many stale reports before {0:?}")]
    Stale(Command),
    #[error("reply to {command:?}: expected {expected} reports, got {actual}")]
    ShortReply {
        command: Command,
        expected: usize,
        actual: usize,
    },
    #[error("reply to {command:?}: tag 0x{actual:02x}, expected 0x{expected:02x}")]
    WrongTag {
        command: Command,
        expected: u8,
        actual: u8,
    },
    #[error("firmware string is not printable ASCII: {0:02x?}")]
    FirmwareNotAscii(Vec<u8>),
    #[error("unsupported firmware {0:?}")]
    UnsupportedFirmware(String),
    #[error("temperature {0} C outside the sensor range")]
    OutOfRange(CentiCelsius),
}

/// A channel to the stick's data interface.
pub trait Transport {
    fn send(&mut self, report: &Report) -> io::Result<()>;

    /// The next report, or `None` if none arrives within `timeout`.
    fn receive(&mut self, timeout: Duration) -> io::Result<Option<Report>>;
}

/// Temperature in hundredths of a degree C, the stick's native unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CentiCelsius(pub(crate) i16);

impl fmt::Display for CentiCelsius {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let magnitude = self.0.unsigned_abs();
        write!(f, "{sign}{}.{:02}", magnitude / 100, magnitude % 100)
    }
}

/// A calibration offset in tenths of a unit (ElfThing `getReciveData`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tenths(pub(crate) i8);

impl fmt::Display for Tenths {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let magnitude = self.0.unsigned_abs();
        write!(f, "{sign}{}.{}", magnitude / 10, magnitude % 10)
    }
}

/// Which probes are present (ElfThing `readSensorType`): nonzero means
/// present.  The value's meaning beyond that is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorType {
    pub inner: u8,
    pub outer: u8,
}

/// Calibration offsets stored on the stick (ElfThing `readCalib`, type
/// 6).  The firmware applies them; the TEMPerGold has no humidity or
/// outer probe, so only `inner_temperature` matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calibration {
    pub inner_temperature: Tenths,
    pub inner_humidity: Tenths,
    pub outer_temperature: Tenths,
    pub outer_humidity: Tenths,
}

/// Manufacture date (ElfThing `readDeviceBirthday`), likely shared by a
/// whole production batch.  Unverified on TEMPerGold: ElfThing sends
/// this query only to TEMPerHUM and to TEMPerX/TEMPer1F/TEMPer2 at
/// firmware 3.6 or later, and the decoded date has not been checked
/// against any marking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManufactureDate {
    pub(crate) year: u16,
    pub(crate) month: u8,
    pub(crate) day: u8,
}

impl fmt::Display for ManufactureDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// A TEMPerGold reached through `T`.
#[derive(Debug)]
pub struct Stick<T> {
    transport: T,
}

impl<T: Transport> Stick<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn firmware(&mut self) -> Result<String, Error> {
        decode_firmware(&self.query(Command::Firmware)?)
    }

    pub fn temperature(&mut self) -> Result<CentiCelsius, Error> {
        decode_temperature(&self.query(Command::Temperature)?)
    }

    pub fn sensor_type(&mut self) -> Result<SensorType, Error> {
        Ok(decode_sensor_type(&self.query(Command::SensorType)?))
    }

    pub fn calibration(&mut self) -> Result<Calibration, Error> {
        Ok(decode_calibration(&self.query(Command::Calibration)?))
    }

    pub fn manufacture_date(&mut self) -> Result<ManufactureDate, Error> {
        Ok(decode_manufacture_date(
            &self.query(Command::ManufactureDate)?,
        ))
    }

    /// Drains stale input, sends `command`, and collects its reply.
    fn query(&mut self, command: Command) -> Result<Vec<Report>, Error> {
        let stale = std::iter::from_fn(|| self.transport.receive(Duration::ZERO).transpose())
            .take(MAX_STALE_REPORTS + 1)
            .collect::<io::Result<Vec<_>>>()?;
        if stale.len() > MAX_STALE_REPORTS {
            return Err(Error::Stale(command));
        }
        self.transport.send(&command.bytes())?;
        let reply = std::iter::from_fn(|| self.transport.receive(REPLY_TIMEOUT).transpose())
            .take(command.reply_reports())
            .collect::<io::Result<Vec<_>>>()?;
        if reply.len() < command.reply_reports() {
            return Err(Error::ShortReply {
                command,
                expected: command.reply_reports(),
                actual: reply.len(),
            });
        }
        if let Some(expected) = command.reply_tag() {
            let actual = reply[0][0];
            if actual != expected {
                return Err(Error::WrongTag {
                    command,
                    expected,
                    actual,
                });
            }
        }
        Ok(reply)
    }
}

/// The firmware string, ASCII and NUL-padded across the reply reports.
fn decode_firmware(reply: &[Report]) -> Result<String, Error> {
    let bytes = reply
        .iter()
        .flatten()
        .copied()
        .filter(|&b| b != 0)
        .collect::<Vec<_>>();
    if !bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return Err(Error::FirmwareNotAscii(bytes));
    }
    let firmware = String::from_utf8_lossy(&bytes).trim().to_owned();
    if !firmware.starts_with(SUPPORTED_FIRMWARE_PREFIX) {
        return Err(Error::UnsupportedFirmware(firmware));
    }
    Ok(firmware)
}

/// Bytes 2-3, big-endian, signed (urwen/temper `_parse_bytes` offset 2,
/// divisor 100; ElfThing `parseByteToData`).
fn decode_temperature(reply: &[Report]) -> Result<CentiCelsius, Error> {
    let report = reply[0];
    let temperature = CentiCelsius(i16::from_be_bytes([report[2], report[3]]));
    if !TEMPERATURE_RANGE.contains(&temperature.0) {
        return Err(Error::OutOfRange(temperature));
    }
    Ok(temperature)
}

fn decode_sensor_type(reply: &[Report]) -> SensorType {
    let report = reply[0];
    SensorType {
        inner: report[1],
        outer: report[2],
    }
}

/// Bytes 2-5, each a signed tenth.  Byte 1's meaning is unknown.
fn decode_calibration(reply: &[Report]) -> Calibration {
    let report = reply[0];
    let tenths = |i: usize| Tenths(i8::from_ne_bytes([report[i]]));
    Calibration {
        inner_temperature: tenths(2),
        inner_humidity: tenths(3),
        outer_temperature: tenths(4),
        outer_humidity: tenths(5),
    }
}

fn decode_manufacture_date(reply: &[Report]) -> ManufactureDate {
    let report = reply[0];
    ManufactureDate {
        year: MANUFACTURE_YEAR_BASE + u16::from(report[1]),
        month: report[2],
        day: report[3],
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    /// Replies captured from a `TEMPerGold_V3.5` stick.
    const FIRMWARE: &[u8] = include_bytes!("../tests/fixtures/firmware.bin");
    const TEMPERATURE: &[u8] = include_bytes!("../tests/fixtures/temperature.bin");
    const SENSOR_TYPE: &[u8] = include_bytes!("../tests/fixtures/sensor_type.bin");
    const CALIBRATION: &[u8] = include_bytes!("../tests/fixtures/calibration.bin");
    const MANUFACTURE_DATE: &[u8] = include_bytes!("../tests/fixtures/manufacture_date.bin");

    fn reports(bytes: &[u8]) -> Vec<Report> {
        bytes.as_chunks::<REPORT_LEN>().0.to_vec()
    }

    /// A transport that replays queued reports and records what was sent.
    #[derive(Debug, Default)]
    struct Fake {
        pending: VecDeque<Report>,
        sent: Vec<Report>,
    }

    impl Transport for Fake {
        fn send(&mut self, report: &Report) -> io::Result<()> {
            self.sent.push(*report);
            Ok(())
        }

        fn receive(&mut self, _timeout: Duration) -> io::Result<Option<Report>> {
            Ok(self.pending.pop_front())
        }
    }

    /// A stick whose reply is queued only once the command is sent, so
    /// draining stale input does not consume it.
    #[derive(Debug)]
    struct Replying {
        stale: VecDeque<Report>,
        reply: Vec<Report>,
        pending: VecDeque<Report>,
        sent: Vec<Report>,
    }

    impl Replying {
        fn new(stale: &[Report], reply: &[u8]) -> Self {
            Self {
                stale: stale.iter().copied().collect(),
                reply: reports(reply),
                pending: VecDeque::new(),
                sent: Vec::new(),
            }
        }
    }

    impl Transport for Replying {
        fn send(&mut self, report: &Report) -> io::Result<()> {
            self.sent.push(*report);
            self.pending.extend(self.reply.iter().copied());
            Ok(())
        }

        fn receive(&mut self, _timeout: Duration) -> io::Result<Option<Report>> {
            Ok(self.stale.pop_front().or_else(|| self.pending.pop_front()))
        }
    }

    #[test]
    fn captured_firmware() {
        let mut stick = Stick::new(Replying::new(&[], FIRMWARE));
        assert_eq!(stick.firmware().unwrap(), "TEMPerGold_V3.5");
        assert_eq!(stick.transport.sent, [Command::Firmware.bytes()]);
    }

    #[test]
    fn captured_temperature() {
        let mut stick = Stick::new(Replying::new(&[], TEMPERATURE));
        assert_eq!(stick.temperature().unwrap(), CentiCelsius(3512));
    }

    #[test]
    fn captured_sensor_type() {
        let mut stick = Stick::new(Replying::new(&[], SENSOR_TYPE));
        assert_eq!(
            stick.sensor_type().unwrap(),
            SensorType {
                inner: 0x80,
                outer: 0
            }
        );
    }

    #[test]
    fn captured_calibration() {
        let mut stick = Stick::new(Replying::new(&[], CALIBRATION));
        let zero = Tenths(0);
        assert_eq!(
            stick.calibration().unwrap(),
            Calibration {
                inner_temperature: zero,
                inner_humidity: zero,
                outer_temperature: zero,
                outer_humidity: zero,
            }
        );
    }

    #[test]
    fn captured_manufacture_date() {
        let mut stick = Stick::new(Replying::new(&[], MANUFACTURE_DATE));
        let date = stick.manufacture_date().unwrap();
        assert_eq!(date.to_string(), "2019-09-19");
    }

    #[test]
    fn stale_input_is_drained() {
        let stale = reports(TEMPERATURE);
        let mut stick = Stick::new(Replying::new(&stale, MANUFACTURE_DATE));
        assert_eq!(stick.manufacture_date().unwrap().year, 2019);
    }

    #[test]
    fn endless_stale_input_is_an_error() {
        let stale = vec![[0; REPORT_LEN]; MAX_STALE_REPORTS + 1];
        let mut stick = Stick::new(Replying::new(&stale, TEMPERATURE));
        assert!(matches!(stick.temperature(), Err(Error::Stale(_))));
    }

    #[test]
    fn missing_reply_is_an_error() {
        let mut stick = Stick::new(Fake::default());
        assert!(matches!(
            stick.firmware(),
            Err(Error::ShortReply {
                expected: 2,
                actual: 0,
                ..
            })
        ));
    }

    #[test]
    fn wrong_tag_is_an_error() {
        let mut stick = Stick::new(Replying::new(&[], SENSOR_TYPE));
        assert!(matches!(
            stick.temperature(),
            Err(Error::WrongTag {
                expected: 0x80,
                actual: 0x87,
                ..
            })
        ));
    }

    #[test]
    fn negative_temperature() {
        let report = [0x80, 0x80, 0xfc, 0x18, 0, 0, 0, 0];
        assert_eq!(decode_temperature(&[report]).unwrap(), CentiCelsius(-1000));
    }

    #[test]
    fn out_of_range_temperature() {
        let report = [0x80, 0x80, 0x4e, 0x20, 0, 0, 0, 0];
        assert!(matches!(
            decode_temperature(&[report]),
            Err(Error::OutOfRange(CentiCelsius(20000)))
        ));
    }

    #[test]
    fn unsupported_firmware() {
        let reply = reports(b"TEMPerX_V3.3\0\0\0\0");
        assert!(matches!(
            decode_firmware(&reply),
            Err(Error::UnsupportedFirmware(_))
        ));
    }

    #[test]
    fn binary_firmware() {
        let reply = reports(&[0xff; 2 * REPORT_LEN]);
        assert!(matches!(
            decode_firmware(&reply),
            Err(Error::FirmwareNotAscii(_))
        ));
    }

    #[test]
    fn negative_calibration() {
        let report = [0x82, 0x04, 0xf6, 0x05, 0, 0, 0, 0];
        let calibration = decode_calibration(&[report]);
        assert_eq!(calibration.inner_temperature.to_string(), "-1.0");
        assert_eq!(calibration.inner_humidity.to_string(), "0.5");
    }

    #[test]
    fn centi_celsius_display() {
        assert_eq!(CentiCelsius(3512).to_string(), "35.12");
        assert_eq!(CentiCelsius(-5).to_string(), "-0.05");
        assert_eq!(CentiCelsius(i16::MIN).to_string(), "-327.68");
    }
}
