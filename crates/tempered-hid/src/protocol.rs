//! TEMPerGold HID protocol: the queries the stick answers and how to
//! decode its replies.
//!
//! Sources: PCsensor's ElfThing 1.0.2 app (`resources/app.asar`, class
//! `HIDTypeDevice`; see `docs/protocol.md` for the download and its
//! hash), and urwen/temper `temper.py`.  Reply layouts were confirmed
//! against captures from a `TEMPerGold_V3.5` stick in `tests/fixtures/`.

use std::fmt;
use std::io;
use std::iter;
use std::ops::RangeInclusive;
use std::time::Duration;

use rustix::io::Errno;

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

/// Sensor range (ElfThing `parseModel`, `innerTemperatureCRangeMin` and
/// `Max`: -40 to 125 degrees C).
const TEMPERATURE_RANGE: RangeInclusive<CentiCelsius> = CentiCelsius(-4000)..=CentiCelsius(12500);

/// The 2000-based year in the manufacture date reply.
const MANUFACTURE_YEAR_BASE: u16 = 2000;

/// Millidegrees per centidegree, for IIO's millidegree unit.
const MILLI_PER_CENTI: i32 = 10;

/// Centidegrees per degree.
const CENTI_PER_DEGREE: f64 = 100.0;

/// A query the stick answers.  Every reply except `Firmware`'s echoes
/// the command's second byte as its first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
    /// The firmware identification string.
    Firmware,
    /// The current temperature.
    Temperature,
    /// Which probes are fitted.
    SensorType,
    /// The calibration offsets stored on the stick.
    Calibration,
    /// The manufacture date; unverified on TEMPerGold.
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

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Firmware => "firmware",
            Self::Temperature => "temperature",
            Self::SensorType => "sensor type",
            Self::Calibration => "calibration",
            Self::ManufactureDate => "manufacture date",
        })
    }
}

/// Why a query failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Reading from or writing to the stick failed.
    #[error("stick I/O")]
    #[non_exhaustive]
    Io(#[source] io::Error),
    /// The stick was removed.  A [`Transport`] reports this as an
    /// `ENODEV` I/O error.
    #[error("stick removed")]
    Gone,
    /// The stick kept sending unrequested reports before this query.
    #[error("too many stale reports before the {0} query")]
    #[non_exhaustive]
    Stale(Command),
    /// The stick sent fewer reports than the reply needs.
    #[error("reply to the {command} query: expected {expected} reports, got {actual}")]
    #[non_exhaustive]
    ShortReply {
        /// The query.
        command: Command,
        /// Reports the reply needs.
        expected: usize,
        /// Reports received.
        actual: usize,
    },
    /// The reply is not for the query sent.
    #[error("reply to the {command} query: tag 0x{actual:02x}, expected 0x{expected:02x}")]
    #[non_exhaustive]
    WrongTag {
        /// The query.
        command: Command,
        /// The tag a reply to `command` carries.
        expected: u8,
        /// The tag received.
        actual: u8,
    },
    /// The firmware reply is not text; holds its bytes.
    #[error("firmware string is not printable ASCII: {0:02x?}")]
    #[non_exhaustive]
    FirmwareNotAscii(Vec<u8>),
    /// The stick is not a model this crate decodes.
    #[error("unsupported firmware {0}")]
    #[non_exhaustive]
    UnsupportedFirmware(Firmware),
    /// The temperature is outside what the sensor can measure.
    #[error("temperature {0} C outside the sensor range")]
    #[non_exhaustive]
    OutOfRange(CentiCelsius),
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        if error.raw_os_error() == Some(Errno::NODEV.raw_os_error()) {
            Self::Gone
        } else {
            Self::Io(error)
        }
    }
}

/// A channel to the stick's data interface.  [`crate::hidraw::Hidraw`]
/// is the usual one; implement this to reach the stick some other way.
pub trait Transport {
    /// Sends one report.
    ///
    /// # Errors
    ///
    /// Any I/O error; `ENODEV` if the device was removed.
    fn send(&mut self, report: &Report) -> io::Result<()>;

    /// The next report, or `None` if none arrives within `timeout`.
    ///
    /// # Errors
    ///
    /// Any I/O error; `ENODEV` if the device was removed.
    fn receive(&mut self, timeout: Duration) -> io::Result<Option<Report>>;
}

/// Writes `value / 10^decimals` with exactly `decimals` places.
fn write_fixed_point(f: &mut fmt::Formatter<'_>, value: i32, decimals: u32) -> fmt::Result {
    let sign = if value < 0 { "-" } else { "" };
    let scale = 10_u32.pow(decimals);
    let magnitude = value.unsigned_abs();
    let width = decimals as usize;
    write!(
        f,
        "{sign}{}.{:0width$}",
        magnitude / scale,
        magnitude % scale
    )
}

/// Temperature in hundredths of a degree Celsius, the stick's native unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CentiCelsius(i16);

impl CentiCelsius {
    /// Wraps a value in hundredths of a degree Celsius.
    #[must_use]
    pub const fn new(centi: i16) -> Self {
        Self(centi)
    }

    /// The value in hundredths of a degree Celsius.
    #[must_use]
    pub const fn get(self) -> i16 {
        self.0
    }

    /// The value in thousandths of a degree Celsius, IIO's unit.
    #[must_use]
    pub const fn millicelsius(self) -> i32 {
        self.0 as i32 * MILLI_PER_CENTI
    }

    /// The value in degrees Celsius.
    #[must_use]
    pub fn celsius(self) -> f64 {
        f64::from(self.0) / CENTI_PER_DEGREE
    }
}

impl fmt::Display for CentiCelsius {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_fixed_point(f, self.0.into(), 2)
    }
}

/// A temperature offset in tenths of a degree Celsius (ElfThing
/// `getReciveData`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeciCelsius(i8);

impl DeciCelsius {
    /// Wraps a value in tenths of a degree Celsius.
    #[must_use]
    pub const fn new(deci: i8) -> Self {
        Self(deci)
    }

    /// The value in tenths of a degree Celsius.
    #[must_use]
    pub const fn get(self) -> i8 {
        self.0
    }
}

impl fmt::Display for DeciCelsius {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_fixed_point(f, self.0.into(), 1)
    }
}

/// A relative humidity offset in tenths of a percent (ElfThing
/// `getReciveData`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeciPercent(i8);

impl DeciPercent {
    /// Wraps a value in tenths of a percent.
    #[must_use]
    pub const fn new(deci: i8) -> Self {
        Self(deci)
    }

    /// The value in tenths of a percent.
    #[must_use]
    pub const fn get(self) -> i8 {
        self.0
    }
}

impl fmt::Display for DeciPercent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_fixed_point(f, self.0.into(), 1)
    }
}

/// The firmware identification string, e.g. `TEMPerGold_V3.5`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Firmware(String);

impl Firmware {
    /// The string as the stick reports it, trimmed.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Firmware {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One probe slot of the sensor type reply (ElfThing `readSensorType`).
/// Nonzero means a probe is fitted; the value's meaning beyond that is
/// unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Probe(u8);

impl Probe {
    /// Wraps the code the stick reports.
    #[must_use]
    pub const fn new(code: u8) -> Self {
        Self(code)
    }

    /// Whether a probe is fitted.
    #[must_use]
    pub const fn is_present(self) -> bool {
        self.0 != 0
    }

    /// The raw code the stick reports.
    #[must_use]
    pub const fn code(self) -> u8 {
        self.0
    }
}

/// Which probes are fitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SensorType {
    /// The probe inside the stick.
    pub inner: Probe,
    /// An external probe; the TEMPerGold has none.
    pub outer: Probe,
}

/// Calibration offsets stored on the stick (ElfThing `readCalib`, type
/// 6).  The firmware applies them; the TEMPerGold has no humidity or
/// outer probe, so only `inner_temperature` matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Calibration {
    /// Offset of the inner temperature probe.
    pub inner_temperature: DeciCelsius,
    /// Offset of the inner humidity probe.
    pub inner_humidity: DeciPercent,
    /// Offset of the outer temperature probe.
    pub outer_temperature: DeciCelsius,
    /// Offset of the outer humidity probe.
    pub outer_humidity: DeciPercent,
}

/// Manufacture date (ElfThing `readDeviceBirthday`), likely shared by a
/// whole production batch.  Unverified on TEMPerGold: ElfThing sends
/// this query only to TEMPerHUM and to TEMPerX/TEMPer1F/TEMPer2 at
/// firmware 3.6 or later, and the decoded date has not been checked
/// against any marking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ManufactureDate {
    /// Year, e.g. 2019.
    pub year: u16,
    /// Month as reported, nominally 1-12.
    pub month: u8,
    /// Day as reported, nominally 1-31.
    pub day: u8,
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
    /// A stick reached through `transport`.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// The transport the stick is reached through.
    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The transport, mutably.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// The transport, giving up the stick.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.transport
    }

    /// Queries the firmware string.
    ///
    /// # Errors
    ///
    /// The query errors (see [`Error`]), and
    /// [`Error::UnsupportedFirmware`] unless it is a TEMPerGold.
    pub fn firmware(&mut self) -> Result<Firmware, Error> {
        decode_firmware(&self.query(Command::Firmware)?)
    }

    /// Queries the temperature.
    ///
    /// # Errors
    ///
    /// The query errors (see [`Error`]), and [`Error::OutOfRange`] for a
    /// reading the sensor cannot produce.
    pub fn temperature(&mut self) -> Result<CentiCelsius, Error> {
        decode_temperature(&self.query(Command::Temperature)?)
    }

    /// Queries which probes are fitted.
    ///
    /// # Errors
    ///
    /// The query errors; see [`Error`].
    pub fn sensor_type(&mut self) -> Result<SensorType, Error> {
        Ok(decode_sensor_type(&self.query(Command::SensorType)?))
    }

    /// Queries the stored calibration offsets.
    ///
    /// # Errors
    ///
    /// The query errors; see [`Error`].
    pub fn calibration(&mut self) -> Result<Calibration, Error> {
        Ok(decode_calibration(&self.query(Command::Calibration)?))
    }

    /// Queries the manufacture date; see [`ManufactureDate`].
    ///
    /// # Errors
    ///
    /// The query errors; see [`Error`].
    pub fn manufacture_date(&mut self) -> Result<ManufactureDate, Error> {
        Ok(decode_manufacture_date(
            &self.query(Command::ManufactureDate)?,
        ))
    }

    /// Drains stale input, sends `command`, and collects its reply.
    fn query(&mut self, command: Command) -> Result<Vec<Report>, Error> {
        let stale = iter::from_fn(|| self.transport.receive(Duration::ZERO).transpose())
            .take(MAX_STALE_REPORTS + 1)
            .collect::<io::Result<Vec<_>>>()?;
        if stale.len() > MAX_STALE_REPORTS {
            return Err(Error::Stale(command));
        }
        self.transport.send(&command.bytes())?;
        let reply = iter::from_fn(|| self.transport.receive(REPLY_TIMEOUT).transpose())
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
fn decode_firmware(reply: &[Report]) -> Result<Firmware, Error> {
    let bytes = reply
        .iter()
        .flatten()
        .copied()
        .filter(|&b| b != 0)
        .collect::<Vec<_>>();
    if !bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return Err(Error::FirmwareNotAscii(bytes));
    }
    let text = bytes.iter().copied().map(char::from).collect::<String>();
    let firmware = Firmware(text.trim().to_owned());
    if !firmware.0.starts_with(SUPPORTED_FIRMWARE_PREFIX) {
        return Err(Error::UnsupportedFirmware(firmware));
    }
    Ok(firmware)
}

/// Bytes 2-3, big-endian, signed (urwen/temper `_parse_bytes` offset 2,
/// divisor 100; ElfThing `parseByteToData`).
fn decode_temperature(reply: &[Report]) -> Result<CentiCelsius, Error> {
    let report = reply[0];
    let temperature = CentiCelsius(i16::from_be_bytes([report[2], report[3]]));
    if !TEMPERATURE_RANGE.contains(&temperature) {
        return Err(Error::OutOfRange(temperature));
    }
    Ok(temperature)
}

fn decode_sensor_type(reply: &[Report]) -> SensorType {
    let report = reply[0];
    SensorType {
        inner: Probe(report[1]),
        outer: Probe(report[2]),
    }
}

/// Bytes 2-5, each a signed tenth.  Byte 1's meaning is unknown.
fn decode_calibration(reply: &[Report]) -> Calibration {
    let report = reply[0];
    let signed = |i: usize| report[i].cast_signed();
    Calibration {
        inner_temperature: DeciCelsius(signed(2)),
        inner_humidity: DeciPercent(signed(3)),
        outer_temperature: DeciCelsius(signed(4)),
        outer_humidity: DeciPercent(signed(5)),
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

    /// A stick that has `stale` reports waiting and queues `reply` once
    /// a command is sent, so draining stale input does not consume it.
    #[derive(Debug)]
    struct Fake {
        stale: VecDeque<Report>,
        reply: Vec<Report>,
        pending: VecDeque<Report>,
        sent: Vec<Report>,
    }

    impl Fake {
        fn new(stale: &[Report], reply: &[u8]) -> Self {
            Self {
                stale: stale.iter().copied().collect(),
                reply: reports(reply),
                pending: VecDeque::new(),
                sent: Vec::new(),
            }
        }
    }

    impl Transport for Fake {
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
        let mut stick = Stick::new(Fake::new(&[], FIRMWARE));
        assert_eq!(stick.firmware().unwrap().as_str(), "TEMPerGold_V3.5");
        assert_eq!(stick.transport.sent, [Command::Firmware.bytes()]);
    }

    #[test]
    fn captured_temperature() {
        let mut stick = Stick::new(Fake::new(&[], TEMPERATURE));
        let temperature = stick.temperature().unwrap();
        assert_eq!(temperature, CentiCelsius(3512));
        assert_eq!(temperature.millicelsius(), 35_120);
    }

    #[test]
    fn captured_sensor_type() {
        let mut stick = Stick::new(Fake::new(&[], SENSOR_TYPE));
        let sensor_type = stick.sensor_type().unwrap();
        assert!(sensor_type.inner.is_present());
        assert_eq!(sensor_type.inner.code(), 0x80);
        assert!(!sensor_type.outer.is_present());
    }

    #[test]
    fn captured_calibration() {
        let mut stick = Stick::new(Fake::new(&[], CALIBRATION));
        assert_eq!(
            stick.calibration().unwrap(),
            Calibration {
                inner_temperature: DeciCelsius(0),
                inner_humidity: DeciPercent(0),
                outer_temperature: DeciCelsius(0),
                outer_humidity: DeciPercent(0),
            }
        );
    }

    #[test]
    fn captured_manufacture_date() {
        let mut stick = Stick::new(Fake::new(&[], MANUFACTURE_DATE));
        let date = stick.manufacture_date().unwrap();
        assert_eq!(date.to_string(), "2019-09-19");
    }

    #[test]
    fn stale_input_is_drained() {
        let stale = reports(TEMPERATURE);
        let mut stick = Stick::new(Fake::new(&stale, MANUFACTURE_DATE));
        assert_eq!(stick.manufacture_date().unwrap().year, 2019);
    }

    #[test]
    fn endless_stale_input_is_an_error() {
        let stale = vec![[0; REPORT_LEN]; MAX_STALE_REPORTS + 1];
        let mut stick = Stick::new(Fake::new(&stale, TEMPERATURE));
        assert!(matches!(stick.temperature(), Err(Error::Stale(_))));
    }

    #[test]
    fn missing_reply_is_an_error() {
        let mut stick = Stick::new(Fake::new(&[], &[]));
        assert!(matches!(
            stick.firmware(),
            Err(Error::ShortReply {
                expected: 2,
                actual: 0,
                ..
            })
        ));
    }

    /// A stick that was unplugged.
    #[derive(Debug)]
    struct Removed;

    impl Transport for Removed {
        fn send(&mut self, _report: &Report) -> io::Result<()> {
            Err(Errno::NODEV.into())
        }

        fn receive(&mut self, _timeout: Duration) -> io::Result<Option<Report>> {
            Err(Errno::NODEV.into())
        }
    }

    #[test]
    fn removed_stick_is_gone() {
        let mut stick = Stick::new(Removed);
        assert!(matches!(stick.temperature(), Err(Error::Gone)));
    }

    #[test]
    fn other_io_errors_stay_io() {
        let error = Error::from(io::Error::from(Errno::IO));
        assert!(matches!(error, Error::Io(_)));
    }

    #[test]
    fn wrong_tag_is_an_error() {
        let mut stick = Stick::new(Fake::new(&[], SENSOR_TYPE));
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
