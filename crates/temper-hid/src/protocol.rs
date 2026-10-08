//! TEMPerGold and TEMPerHUM HID protocol: the queries the sticks answer
//! and how to decode their replies.
//!
//! Sources: PCsensor's ElfThing 1.0.2 app (`resources/app.asar`, class
//! `HIDTypeDevice`; see `docs/protocol.md` for the download and its
//! hash), and urwen/temper `temper.py`.  Reply layouts were confirmed
//! against captures from `TEMPerGold_V3.5` and `TEMPerHUM_V4.1` sticks
//! in `tests/fixtures/`.

use std::fmt;
use std::io;
use std::iter;
use std::ops::RangeInclusive;
use std::time::Duration;

#[cfg(unix)]
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

/// TEMPerGold sensor range (ElfThing `parseModel`, default
/// `innerTemperatureCRangeMin` and `Max`: -40 to 125 degrees C).
const TEMPER_GOLD_TEMPERATURE_RANGE: RangeInclusive<i16> = -4000..=12500;

/// TEMPerHUM sensor range (ElfThing `parseModel`, `TEMPerHUM_` branch:
/// -40 to 85 degrees C).
const TEMPER_HUM_TEMPERATURE_RANGE: RangeInclusive<i16> = -4000..=8500;

/// Relative humidity range, 0 to 100 percent (the TEMPerHUM's case is
/// marked "0-100%RH": ccwienk/temper `README.md`).
const HUMIDITY_RANGE: RangeInclusive<i16> = 0..=10000;

/// The 2000-based year in the manufacture date reply.
const MANUFACTURE_YEAR_BASE: u16 = 2000;

/// The stick reports readings in hundredths.
const READING_SCALE: f64 = 100.0;

/// And calibration offsets in tenths.
const CALIBRATION_SCALE: f64 = 10.0;

/// A stick model this crate decodes, told by its firmware string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Model {
    /// Temperature only.
    TemperGold,
    /// Temperature and relative humidity.
    TemperHum,
}

impl Model {
    /// Every model, for matching firmware strings.
    const ALL: [Self; 2] = [Self::TemperGold, Self::TemperHum];

    /// The firmware string prefix, e.g. `TEMPerGold_` of
    /// `TEMPerGold_V3.5`.  ElfThing's `parseModel` tells TEMPerHUM by
    /// `TEMPerHUM_`, which excludes the differently decoded
    /// `TEMPerHumM12`.
    fn firmware_prefix(self) -> &'static str {
        match self {
            Self::TemperGold => "TEMPerGold_",
            Self::TemperHum => "TEMPerHUM_",
        }
    }

    /// In hundredths of a degree, as the stick reports.
    fn temperature_range(self) -> RangeInclusive<i16> {
        match self {
            Self::TemperGold => TEMPER_GOLD_TEMPERATURE_RANGE,
            Self::TemperHum => TEMPER_HUM_TEMPERATURE_RANGE,
        }
    }

    /// Whether the stick measures relative humidity.
    #[must_use]
    pub const fn has_humidity(self) -> bool {
        match self {
            Self::TemperGold => false,
            Self::TemperHum => true,
        }
    }
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TemperGold => "TEMPerGold",
            Self::TemperHum => "TEMPerHUM",
        })
    }
}

/// A query the stick answers.  Every reply except `Firmware`'s echoes
/// the command's second byte as its first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
    /// The firmware identification string.
    Firmware,
    /// The current temperature, and humidity on a TEMPerHUM.
    Temperature,
    /// Which probes are fitted.
    SensorType,
    /// The calibration offsets stored on the stick.
    Calibration,
    /// The manufacture date; unverified.
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
    /// The stick was removed.  A [`Transport`] reports this as an I/O
    /// error of kind `NotConnected`, or, on Unix, `ENODEV`.
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
    OutOfRange(Celsius),
    /// The relative humidity is outside 0 to 100 percent.
    #[error("humidity {0} %RH outside the sensor range")]
    #[non_exhaustive]
    HumidityOutOfRange(RelativeHumidityPercent),
}

/// Whether `error` means the device was removed: kind `NotConnected`,
/// which any platform's transport can produce, or `ENODEV`, which
/// hidraw writes return (`drivers/hid/hidraw.c`).
fn is_removal(error: &io::Error) -> bool {
    #[cfg(unix)]
    if error.raw_os_error() == Some(Errno::NODEV.raw_os_error()) {
        return true;
    }
    error.kind() == io::ErrorKind::NotConnected
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        if is_removal(&error) {
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
    /// Any I/O error; one of kind `NotConnected` (or, on Unix,
    /// `ENODEV`) if the device was removed.
    fn send(&mut self, report: &Report) -> io::Result<()>;

    /// The next report, or `None` if none arrives within `timeout`.
    ///
    /// # Errors
    ///
    /// Any I/O error; one of kind `NotConnected` (or, on Unix,
    /// `ENODEV`) if the device was removed.
    fn receive(&mut self, timeout: Duration) -> io::Result<Option<Report>>;
}

/// A temperature in degrees Celsius.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Celsius(f64);

impl Celsius {
    /// Wraps a value in degrees Celsius.
    #[must_use]
    pub const fn new(degrees: f64) -> Self {
        Self(degrees)
    }

    /// The value in degrees Celsius.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

/// Two decimal places, the stick's resolution.
impl fmt::Display for Celsius {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.2}", self.0)
    }
}

/// A relative humidity in percent, 0 to 100.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct RelativeHumidityPercent(f64);

impl RelativeHumidityPercent {
    /// Wraps a value in percent.
    #[must_use]
    pub const fn new(percent: f64) -> Self {
        Self(percent)
    }

    /// The value in percent.
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

/// Two decimal places, the stick's resolution.
impl fmt::Display for RelativeHumidityPercent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.2}", self.0)
    }
}

/// One reading of everything the stick measures.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Reading {
    /// The temperature.
    pub temperature: Celsius,
    /// The relative humidity, on models that measure it.
    pub humidity: Option<RelativeHumidityPercent>,
}

impl Reading {
    /// A reading of `temperature`, and `humidity` if measured.
    #[must_use]
    pub const fn new(temperature: Celsius, humidity: Option<RelativeHumidityPercent>) -> Self {
        Self {
            temperature,
            humidity,
        }
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

    /// The model the string names, if it is one this crate decodes.
    #[must_use]
    pub fn model(&self) -> Option<Model> {
        Model::ALL
            .into_iter()
            .find(|model| self.0.starts_with(model.firmware_prefix()))
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

/// Calibration offsets stored on the stick, each to a tenth (ElfThing
/// `readCalib`, type 6, and `getReciveData`).  The firmware applies
/// them; neither model has an outer probe, and the TEMPerGold no
/// humidity sensor.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Calibration {
    /// Offset of the inner temperature probe.
    pub inner_temperature: Celsius,
    /// Offset of the inner humidity probe.
    pub inner_humidity: RelativeHumidityPercent,
    /// Offset of the outer temperature probe.
    pub outer_temperature: Celsius,
    /// Offset of the outer humidity probe.
    pub outer_humidity: RelativeHumidityPercent,
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

/// A TEMPerGold or TEMPerHUM reached through `T`.
#[derive(Debug)]
pub struct Stick<T> {
    transport: T,
    /// The model, once a firmware query has told it.
    model: Option<Model>,
}

impl<T: Transport> Stick<T> {
    /// A stick reached through `transport`.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            model: None,
        }
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

    /// Queries the firmware string, and records the model it names.
    ///
    /// # Errors
    ///
    /// The query errors (see [`Error`]), and
    /// [`Error::UnsupportedFirmware`] unless it names a [`Model`].
    pub fn firmware(&mut self) -> Result<Firmware, Error> {
        self.identify().map(|(firmware, _)| firmware)
    }

    /// The model, querying the firmware the first time.
    ///
    /// # Errors
    ///
    /// As [`Stick::firmware`].
    pub fn model(&mut self) -> Result<Model, Error> {
        match self.model {
            Some(model) => Ok(model),
            None => self.identify().map(|(_, model)| model),
        }
    }

    /// Queries the firmware and records the model it names.
    fn identify(&mut self) -> Result<(Firmware, Model), Error> {
        let firmware = decode_firmware(&self.query(Command::Firmware)?)?;
        let Some(model) = firmware.model() else {
            return Err(Error::UnsupportedFirmware(firmware));
        };
        self.model = Some(model);
        Ok((firmware, model))
    }

    /// Queries everything the stick measures, querying the firmware the
    /// first time to learn the model.
    ///
    /// # Errors
    ///
    /// As [`Stick::model`] and the query (see [`Error`]), and
    /// [`Error::OutOfRange`] or [`Error::HumidityOutOfRange`] for a
    /// reading the sensor cannot produce.
    pub fn reading(&mut self) -> Result<Reading, Error> {
        let model = self.model()?;
        let reply = self.query(Command::Temperature)?;
        Ok(Reading {
            temperature: decode_temperature(&reply, model)?,
            humidity: if model.has_humidity() {
                Some(decode_humidity(&reply)?)
            } else {
                None
            },
        })
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
/// Whether it names a model is the caller's check.
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
    Ok(Firmware(text.trim().to_owned()))
}

/// Bytes 2-3, big-endian, signed (urwen/temper `_parse_bytes` offset 2,
/// divisor 100; ElfThing `parseByteToData`).
fn decode_temperature(reply: &[Report], model: Model) -> Result<Celsius, Error> {
    let report = reply[0];
    let centi = i16::from_be_bytes([report[2], report[3]]);
    let temperature = Celsius(f64::from(centi) / READING_SCALE);
    if !model.temperature_range().contains(&centi) {
        return Err(Error::OutOfRange(temperature));
    }
    Ok(temperature)
}

/// Bytes 4-5, big-endian, signed (ElfThing type 5, `TEMPerHUM` branch:
/// `parseByteToData` of the bytes after the temperature; urwen/temper
/// `TEMPerHUM_V3.9`, offset 4, divisor 100).
fn decode_humidity(reply: &[Report]) -> Result<RelativeHumidityPercent, Error> {
    let report = reply[0];
    let centi = i16::from_be_bytes([report[4], report[5]]);
    let humidity = RelativeHumidityPercent(f64::from(centi) / READING_SCALE);
    if !HUMIDITY_RANGE.contains(&centi) {
        return Err(Error::HumidityOutOfRange(humidity));
    }
    Ok(humidity)
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
    let offset = |i: usize| f64::from(report[i].cast_signed()) / CALIBRATION_SCALE;
    Calibration {
        inner_temperature: Celsius(offset(2)),
        inner_humidity: RelativeHumidityPercent(offset(3)),
        outer_temperature: Celsius(offset(4)),
        outer_humidity: RelativeHumidityPercent(offset(5)),
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
    const FIRMWARE: &[u8] = include_bytes!("../tests/fixtures/temper_gold/firmware.bin");
    const TEMPERATURE: &[u8] = include_bytes!("../tests/fixtures/temper_gold/temperature.bin");
    const SENSOR_TYPE: &[u8] = include_bytes!("../tests/fixtures/temper_gold/sensor_type.bin");
    const CALIBRATION: &[u8] = include_bytes!("../tests/fixtures/temper_gold/calibration.bin");
    const MANUFACTURE_DATE: &[u8] =
        include_bytes!("../tests/fixtures/temper_gold/manufacture_date.bin");

    /// Replies captured from a `TEMPerHUM_V4.1` stick.
    const HUM_FIRMWARE: &[u8] = include_bytes!("../tests/fixtures/temper_hum/firmware.bin");
    const HUM_TEMPERATURE: &[u8] = include_bytes!("../tests/fixtures/temper_hum/temperature.bin");
    const HUM_SENSOR_TYPE: &[u8] = include_bytes!("../tests/fixtures/temper_hum/sensor_type.bin");
    const HUM_CALIBRATION: &[u8] = include_bytes!("../tests/fixtures/temper_hum/calibration.bin");
    const HUM_MANUFACTURE_DATE: &[u8] =
        include_bytes!("../tests/fixtures/temper_hum/manufacture_date.bin");

    fn reports(bytes: &[u8]) -> Vec<Report> {
        bytes.as_chunks::<REPORT_LEN>().0.to_vec()
    }

    /// A stick that has `stale` reports waiting and queues the next of
    /// `replies` as each command is sent, so draining stale input does
    /// not consume it.  The last reply repeats.
    #[derive(Debug)]
    struct Fake {
        stale: VecDeque<Report>,
        replies: VecDeque<Vec<Report>>,
        pending: VecDeque<Report>,
        sent: Vec<Report>,
    }

    impl Fake {
        fn new(stale: &[Report], replies: &[&[u8]]) -> Self {
            Self {
                stale: stale.iter().copied().collect(),
                replies: replies.iter().map(|reply| reports(reply)).collect(),
                pending: VecDeque::new(),
                sent: Vec::new(),
            }
        }
    }

    impl Transport for Fake {
        fn send(&mut self, report: &Report) -> io::Result<()> {
            self.sent.push(*report);
            let reply = if self.replies.len() > 1 {
                self.replies.pop_front()
            } else {
                self.replies.front().cloned()
            };
            self.pending.extend(reply.into_iter().flatten());
            Ok(())
        }

        fn receive(&mut self, _timeout: Duration) -> io::Result<Option<Report>> {
            Ok(self.stale.pop_front().or_else(|| self.pending.pop_front()))
        }
    }

    #[test]
    fn captured_firmware() {
        let mut stick = Stick::new(Fake::new(&[], &[FIRMWARE]));
        assert_eq!(stick.firmware().unwrap().as_str(), "TEMPerGold_V3.5");
        assert_eq!(stick.transport.sent, [Command::Firmware.bytes()]);
    }

    #[test]
    fn captured_sensor_type() {
        let mut stick = Stick::new(Fake::new(&[], &[SENSOR_TYPE]));
        let sensor_type = stick.sensor_type().unwrap();
        assert!(sensor_type.inner.is_present());
        assert_eq!(sensor_type.inner.code(), 0x80);
        assert!(!sensor_type.outer.is_present());
    }

    #[test]
    fn captured_calibration() {
        let mut stick = Stick::new(Fake::new(&[], &[CALIBRATION]));
        assert_eq!(
            stick.calibration().unwrap(),
            Calibration {
                inner_temperature: Celsius(0.0),
                inner_humidity: RelativeHumidityPercent(0.0),
                outer_temperature: Celsius(0.0),
                outer_humidity: RelativeHumidityPercent(0.0),
            }
        );
    }

    #[test]
    fn captured_manufacture_date() {
        let mut stick = Stick::new(Fake::new(&[], &[MANUFACTURE_DATE]));
        let date = stick.manufacture_date().unwrap();
        assert_eq!(date.to_string(), "2019-09-19");
    }

    #[test]
    fn stale_input_is_drained() {
        let stale = reports(TEMPERATURE);
        let mut stick = Stick::new(Fake::new(&stale, &[MANUFACTURE_DATE]));
        assert_eq!(stick.manufacture_date().unwrap().year, 2019);
    }

    #[test]
    fn endless_stale_input_is_an_error() {
        let stale = vec![[0; REPORT_LEN]; MAX_STALE_REPORTS + 1];
        let mut stick = Stick::new(Fake::new(&stale, &[TEMPERATURE]));
        assert!(matches!(stick.reading(), Err(Error::Stale(_))));
    }

    #[test]
    fn missing_reply_is_an_error() {
        let mut stick = Stick::new(Fake::new(&[], &[&[]]));
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
            Err(io::ErrorKind::NotConnected.into())
        }

        fn receive(&mut self, _timeout: Duration) -> io::Result<Option<Report>> {
            Err(io::ErrorKind::NotConnected.into())
        }
    }

    #[test]
    fn removed_stick_is_gone() {
        let mut stick = Stick::new(Removed);
        assert!(matches!(stick.reading(), Err(Error::Gone)));
    }

    #[cfg(unix)]
    #[test]
    fn enodev_is_gone() {
        let error = Error::from(io::Error::from(Errno::NODEV));
        assert!(matches!(error, Error::Gone));
    }

    #[test]
    fn other_io_errors_stay_io() {
        let error = Error::from(io::Error::from(io::ErrorKind::TimedOut));
        assert!(matches!(error, Error::Io(_)));
        #[cfg(unix)]
        assert!(matches!(
            Error::from(io::Error::from(Errno::IO)),
            Error::Io(_)
        ));
    }

    #[test]
    fn wrong_tag_is_an_error() {
        let mut stick = Stick::new(Fake::new(&[], &[FIRMWARE, SENSOR_TYPE]));
        assert!(matches!(
            stick.reading(),
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
        assert_eq!(
            decode_temperature(&[report], Model::TemperGold).unwrap(),
            Celsius(-10.0)
        );
    }

    #[test]
    fn out_of_range_temperature() {
        let report = [0x80, 0x80, 0x4e, 0x20, 0, 0, 0, 0];
        assert!(matches!(
            decode_temperature(&[report], Model::TemperGold),
            Err(Error::OutOfRange(temperature)) if temperature == Celsius(200.0)
        ));
    }

    #[test]
    fn unsupported_firmware() {
        let mut stick = Stick::new(Fake::new(&[], &[b"TEMPerX_V3.3\0\0\0\0"]));
        assert!(matches!(
            stick.firmware(),
            Err(Error::UnsupportedFirmware(_))
        ));
        assert!(matches!(
            stick.reading(),
            Err(Error::UnsupportedFirmware(_))
        ));
    }

    #[test]
    fn gold_reading_has_no_humidity() {
        let mut stick = Stick::new(Fake::new(&[], &[FIRMWARE, TEMPERATURE]));
        assert_eq!(
            stick.reading().unwrap(),
            Reading {
                temperature: Celsius(35.12),
                humidity: None,
            }
        );
        assert_eq!(stick.model().unwrap(), Model::TemperGold);
        assert_eq!(stick.transport.sent.len(), 2);
    }

    #[test]
    fn captured_hum_firmware() {
        let mut stick = Stick::new(Fake::new(&[], &[HUM_FIRMWARE]));
        let firmware = stick.firmware().unwrap();
        assert_eq!(firmware.as_str(), "TEMPerHUM_V4.1");
        assert_eq!(firmware.model(), Some(Model::TemperHum));
    }

    #[test]
    fn captured_hum_reading() {
        let mut stick = Stick::new(Fake::new(&[], &[HUM_FIRMWARE, HUM_TEMPERATURE]));
        let reading = stick.reading().unwrap();
        assert_eq!(reading.temperature, Celsius(33.88));
        let humidity = reading.humidity.unwrap();
        assert_eq!(humidity, RelativeHumidityPercent(31.01));
        assert_eq!(humidity.to_string(), "31.01");
        stick.reading().unwrap();
        assert_eq!(
            stick.transport.sent,
            [
                Command::Firmware.bytes(),
                Command::Temperature.bytes(),
                Command::Temperature.bytes()
            ]
        );
    }

    #[test]
    fn captured_hum_details() {
        let mut stick = Stick::new(Fake::new(&[], &[HUM_SENSOR_TYPE]));
        assert_eq!(stick.sensor_type().unwrap().inner.code(), 0x20);
        let mut stick = Stick::new(Fake::new(&[], &[HUM_CALIBRATION]));
        assert_eq!(
            stick.calibration().unwrap().inner_humidity,
            RelativeHumidityPercent(0.0)
        );
        let mut stick = Stick::new(Fake::new(&[], &[HUM_MANUFACTURE_DATE]));
        assert_eq!(stick.manufacture_date().unwrap().to_string(), "2023-03-01");
    }

    #[test]
    fn hum_temperature_range() {
        let report = [0x80, 0x20, 0x27, 0x10, 0x0c, 0x1d, 0, 0];
        let mut stick = Stick::new(Fake::new(&[], &[HUM_FIRMWARE, &report]));
        assert!(matches!(
            stick.reading(),
            Err(Error::OutOfRange(temperature)) if temperature == Celsius(100.0)
        ));
    }

    #[test]
    fn out_of_range_humidity() {
        let report = [0x80, 0x20, 0x0d, 0x3c, 0x27, 0x11, 0, 0];
        assert!(matches!(
            decode_humidity(&[report]),
            Err(Error::HumidityOutOfRange(humidity)) if humidity == RelativeHumidityPercent(100.01)
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
        assert_eq!(calibration.inner_temperature, Celsius(-1.0));
        assert_eq!(calibration.inner_humidity, RelativeHumidityPercent(0.5));
    }

    #[test]
    fn display_two_places() {
        assert_eq!(Celsius(35.12).to_string(), "35.12");
        assert_eq!(Celsius(-0.05).to_string(), "-0.05");
        assert_eq!(RelativeHumidityPercent(31.0).to_string(), "31.00");
    }
}
