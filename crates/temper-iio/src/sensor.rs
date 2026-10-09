//! The HID sensor the daemon presents through uhid: its report
//! descriptor, and the state that answers the kernel's report requests.
//!
//! Sources, all in the kernel tree: `include/linux/hid-sensor-ids.h`
//! (usages), `drivers/hid/hid-sensor-hub.c` (`sensor_hub_input_get_
//! attribute_info()`: a field is found by its application collection's
//! usage, and by its first usage or its logical collection's usage),
//! `drivers/iio/common/hid-sensors/hid-sensor-attributes.c` and
//! `hid-sensor-trigger.c` (which properties are read and written), and
//! `drivers/iio/temperature/hid-sensor-temperature.c` and
//! `drivers/iio/humidity/hid-sensor-humidity.c`.  Item encodings
//! follow the HID 1.11 specification, section 6.2.2.  See `PLAN.md`,
//! "Report descriptor requirements".

use rustix::io::Errno;

use crate::uhid::FromKernel;
use crate::uhid::ReportNumber;
use crate::uhid::ReportType;
use crate::uhid::RequestId;
use crate::uhid::ToKernel;

/// The Sensors usage page, the high half of every usage below.
const SENSOR_PAGE: u32 = 0x20;

/// `HID_USAGE_SENSOR_DATA_MOD_CHANGE_SENSITIVITY_ABS`, or'ed into a data
/// field's usage for its sensitivity property.
const CHANGE_SENSITIVITY_ABS: u32 = 0x1000;

/// Named array values are 1-based: the kernel forces Logical Minimum to
/// 1 for power and reporting state (commit b0f847e16c1e), and the
/// descriptor declares the same.
const ENUM_BASE: u8 = 1;

/// Reporting State selectors, in descriptor order.
const REPORTING_STATE_NO_EVENTS: u8 = ENUM_BASE;

/// Power State selectors, in descriptor order: Undefined, then D0.
const POWER_STATE_D0_FULL_POWER: u8 = ENUM_BASE + 1;

/// A quantity the virtual device reports.  Each is an application
/// collection of its own with its own report ID, which
/// `hid-sensor-hub` makes a platform device and the quantity's driver
/// an IIO device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Quantity {
    /// Degrees C; `hid-sensor-temperature`.
    Temperature,
    /// Percent relative humidity; `hid-sensor-humidity`.
    Humidity,
}

impl Quantity {
    /// The report ID of the collection's feature and input reports.
    /// Numbered, because since 6.16 (0d0777ccaa2d) the kernel reserves
    /// a leading byte for unnumbered reports in requests.
    fn report_id(self) -> u8 {
        match self {
            Self::Temperature => 1,
            Self::Humidity => 2,
        }
    }

    /// The collection's usage (`HID_USAGE_SENSOR_TEMPERATURE`,
    /// `HID_USAGE_SENSOR_HUMIDITY`).
    fn usage(self) -> u32 {
        match self {
            Self::Temperature => 0x20_0033,
            Self::Humidity => 0x20_0032,
        }
    }

    /// The input field's usage
    /// (`HID_USAGE_SENSOR_DATA_ENVIRONMENTAL_TEMPERATURE`,
    /// `HID_USAGE_SENSOR_ATMOSPHERIC_HUMIDITY`).
    fn data_usage(self) -> u32 {
        match self {
            Self::Temperature => 0x20_0434,
            Self::Humidity => 0x20_0433,
        }
    }

    /// The `name` of the IIO device its driver registers.
    pub(crate) fn iio_name(self) -> &'static str {
        match self {
            Self::Temperature => "temperature",
            Self::Humidity => "humidity",
        }
    }

    /// The name prefix of the platform devices `hid-sensor-hub` makes for
    /// collections of this usage (`"HID-SENSOR-%x"`, then an instance).
    pub(crate) fn platform_prefix(self) -> String {
        format!("HID-SENSOR-{:x}.", self.usage())
    }

    /// The application collection: a feature report and an input report.
    #[rustfmt::skip]
    fn collection(self) -> Vec<u8> {
        let id = self.report_id();
        let [usage, ..] = self.usage().to_le_bytes();
        let [data_low, data_high, ..] = self.data_usage().to_le_bytes();
        let [sensitivity_low, sensitivity_high, ..] =
            (self.data_usage() | CHANGE_SENSITIVITY_ABS).to_le_bytes();
        let [page, ..] = SENSOR_PAGE.to_le_bytes();
        vec![
            0x05, page,                   // Usage Page (Sensors)
            0x09, usage,                  // Usage (the sensor)
            0xa1, 0x01,                   // Collection (Application)
            0x85, id,                     //   Report ID

            // Feature report.
            0x0a, 0x16, 0x03,             //   Usage (Property: Reporting State)
            0x15, ENUM_BASE,              //   Logical Minimum (1)
            0x25, 0x02,                   //   Logical Maximum (2)
            0x75, 0x08,                   //   Report Size (8)
            0x95, 0x01,                   //   Report Count (1)
            0xa1, 0x02,                   //   Collection (Logical)
            0x0a, 0x40, 0x08,             //     Usage (Reporting State: No Events)
            0x0a, 0x41, 0x08,             //     Usage (Reporting State: All Events)
            0xb1, 0x00,                   //     Feature (Data, Array, Absolute)
            0xc0,                         //   End Collection

            0x0a, 0x19, 0x03,             //   Usage (Property: Power State)
            0x15, ENUM_BASE,              //   Logical Minimum (1)
            0x25, 0x06,                   //   Logical Maximum (6)
            0x75, 0x08,                   //   Report Size (8)
            0x95, 0x01,                   //   Report Count (1)
            0xa1, 0x02,                   //   Collection (Logical)
            0x0a, 0x50, 0x08,             //     Usage (Power State: Undefined)
            0x0a, 0x51, 0x08,             //     Usage (Power State: D0 Full Power)
            0x0a, 0x52, 0x08,             //     Usage (Power State: D1 Low Power)
            0x0a, 0x53, 0x08,             //     Usage (Power State: D2 Standby With Wake)
            0x0a, 0x54, 0x08,             //     Usage (Power State: D3 Sleep With Wake)
            0x0a, 0x55, 0x08,             //     Usage (Power State: D4 Power Off)
            0xb1, 0x00,                   //     Feature (Data, Array, Absolute)
            0xc0,                         //   End Collection

            // Always 0: see PLAN.md, "Report Interval is always 0".  No Unit,
            // so the kernel takes milliseconds.
            0x0a, 0x0e, 0x03,             //   Usage (Property: Report Interval)
            0x15, 0x00,                   //   Logical Minimum (0)
            0x27, 0xff, 0xff, 0xff, 0x7f, //   Logical Maximum (2147483647)
            0x75, 0x20,                   //   Report Size (32)
            0x95, 0x01,                   //   Report Count (1)
            0x65, 0x00,                   //   Unit (None)
            0x55, 0x00,                   //   Unit Exponent (0)
            0xb1, 0x02,                   //   Feature (Data, Variable, Absolute)

            // Always 0; present so in_*_hysteresis reads succeed.
            0x0a, sensitivity_low, sensitivity_high,
                                          //   Usage (Change Sensitivity Absolute | data)
            0x15, 0x00,                   //   Logical Minimum (0)
            0x27, 0xff, 0xff, 0x00, 0x00, //   Logical Maximum (65535)
            0x75, 0x10,                   //   Report Size (16)
            0x95, 0x01,                   //   Report Count (1)
            0x65, 0x00,                   //   Unit (None)
            0x55, 0x0e,                   //   Unit Exponent (-2)
            0xb1, 0x02,                   //   Feature (Data, Variable, Absolute)

            // Input report.  Unit None with exponent -2: the kernel's scale
            // table (hid-sensor-attributes.c) has a Unit 0 row for both
            // quantities, giving a scale of 10, so raw hundredths read as
            // IIO's thousandths.  32
            // bits, though the stick's values fit 16: the drivers' buffered
            // path reads every sample as 32 bits (temperature_capture_sample()),
            // so a 16-bit field would hand it two stray bytes, wrong for
            // negative values.
            0x0a, data_low, data_high,    //   Usage (the data field)
            0x16, 0x00, 0x80,             //   Logical Minimum (-32768)
            0x26, 0xff, 0x7f,             //   Logical Maximum (32767)
            0x75, 0x20,                   //   Report Size (32)
            0x95, 0x01,                   //   Report Count (1)
            0x65, 0x00,                   //   Unit (None)
            0x55, 0x0e,                   //   Unit Exponent (-2)
            0x81, 0x02,                   //   Input (Data, Variable, Absolute)
            0xc0,                         // End Collection
        ]
    }
}

/// Feature report length: ID, reporting state, power state, report
/// interval (u32), sensitivity (u16).
const FEATURE_REPORT_LEN: usize = 1 + 1 + 1 + 4 + 2;

/// Input report length: ID, value (i32).
const INPUT_REPORT_LEN: usize = 1 + 4;

/// Input values are in hundredths: the descriptor's Unit Exponent -2.
const HUNDREDTHS_PER_UNIT: f64 = 100.0;

/// One quantity's value, in hundredths of its unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sample {
    /// What it measures.
    quantity: Quantity,
    /// The value, in hundredths.
    centi: i32,
}

impl Sample {
    /// `value`, in the quantity's unit (degrees C, percent), to the
    /// nearest hundredth, which is exact for the stick's readings.
    pub(crate) fn new(quantity: Quantity, value: f64) -> Self {
        Self {
            quantity,
            centi: (value * HUNDREDTHS_PER_UNIT).round() as i32,
        }
    }
}

/// The feature values the kernel may set.  Report interval and
/// sensitivity are always reported as 0, so writes to them are dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Features {
    reporting_state: u8,
    power_state: u8,
}

/// A reply to one kernel request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Reply {
    /// The report asked for, or why not.
    GetReport {
        /// The request's id.
        id: RequestId,
        /// The report, report ID first.
        result: Result<Vec<u8>, Errno>,
    },
    /// Whether the report was taken.
    SetReport {
        /// The request's id.
        id: RequestId,
        /// The outcome.
        result: Result<(), Errno>,
    },
}

impl Reply {
    /// The event that carries this reply.
    pub(crate) fn event(&self) -> ToKernel<'_> {
        match self {
            Self::GetReport { id, result } => ToKernel::GetReportReply {
                id: *id,
                result: result.as_deref().map_err(|errno| *errno),
            },
            Self::SetReport { id, result } => ToKernel::SetReportReply {
                id: *id,
                result: *result,
            },
        }
    }
}

/// One quantity's collection: its latest value and its feature values.
#[derive(Debug)]
struct Channel {
    sample: Sample,
    features: Features,
}

impl Channel {
    /// Report interval and sensitivity are always 0.
    fn feature_report(&self) -> Vec<u8> {
        let mut report = Vec::with_capacity(FEATURE_REPORT_LEN);
        report.extend_from_slice(&[
            self.sample.quantity.report_id(),
            self.features.reporting_state,
            self.features.power_state,
        ]);
        report.extend_from_slice(&0_u32.to_le_bytes());
        report.extend_from_slice(&0_u16.to_le_bytes());
        report
    }

    fn input_report(&self) -> Vec<u8> {
        let mut report = Vec::with_capacity(INPUT_REPORT_LEN);
        report.push(self.sample.quantity.report_id());
        report.extend_from_slice(&self.sample.centi.to_le_bytes());
        report
    }
}

/// What the virtual sensor reports: a channel per quantity, fixed at
/// creation.  There is always a reading: the daemon creates the device
/// only once it has one.
#[derive(Debug)]
pub(crate) struct Sensor {
    channels: Vec<Channel>,
}

impl Sensor {
    /// A sensor reporting `samples`' quantities, in that order.
    pub(crate) fn new(samples: &[Sample]) -> Self {
        let channels = samples
            .iter()
            .map(|&sample| Channel {
                sample,
                features: Features {
                    reporting_state: REPORTING_STATE_NO_EVENTS,
                    power_state: POWER_STATE_D0_FULL_POWER,
                },
            })
            .collect();
        Self { channels }
    }

    /// The report descriptor: one application collection per quantity,
    /// as the HID sensor usage examples have it.
    pub(crate) fn descriptor(&self) -> Vec<u8> {
        self.channels
            .iter()
            .flat_map(|channel| channel.sample.quantity.collection())
            .collect()
    }

    /// Records new values and returns the input reports to push.  A
    /// sample for a quantity the sensor does not report is dropped.
    pub(crate) fn update(&mut self, samples: &[Sample]) -> Vec<Vec<u8>> {
        for sample in samples {
            if let Some(channel) = self
                .channels
                .iter_mut()
                .find(|channel| channel.sample.quantity == sample.quantity)
            {
                channel.sample = *sample;
            }
        }
        self.channels.iter().map(Channel::input_report).collect()
    }

    /// The reply to `event`, if it needs one.
    pub(crate) fn handle(&mut self, event: &FromKernel) -> Option<Reply> {
        match event {
            FromKernel::GetReport { id, number, kind } => Some(Reply::GetReport {
                id: *id,
                result: self.get_report(*number, *kind),
            }),
            FromKernel::SetReport {
                id,
                number,
                kind,
                data,
            } => Some(Reply::SetReport {
                id: *id,
                result: self.set_report(*number, *kind, data),
            }),
            FromKernel::Start { .. }
            | FromKernel::Stop
            | FromKernel::Open
            | FromKernel::Close
            | FromKernel::Output { .. }
            | FromKernel::Unknown(_) => None,
        }
    }

    /// The channel whose report ID is `number`.
    fn channel(&self, number: ReportNumber) -> Result<usize, Errno> {
        self.channels
            .iter()
            .position(|channel| ReportNumber(channel.sample.quantity.report_id()) == number)
            .ok_or(Errno::INVAL)
    }

    fn get_report(&self, number: ReportNumber, kind: ReportType) -> Result<Vec<u8>, Errno> {
        let channel = &self.channels[self.channel(number)?];
        match kind {
            ReportType::Feature => Ok(channel.feature_report()),
            ReportType::Input => Ok(channel.input_report()),
            ReportType::Output | ReportType::Unknown(_) => Err(Errno::INVAL),
        }
    }

    /// Stores the reporting and power states; the rest is ignored.
    fn set_report(
        &mut self,
        number: ReportNumber,
        kind: ReportType,
        data: &[u8],
    ) -> Result<(), Errno> {
        let index = self.channel(number)?;
        match (kind, data) {
            (ReportType::Feature, [id, reporting, power, ..])
                if *id == number.0 && data.len() == FEATURE_REPORT_LEN =>
            {
                self.channels[index].features = Features {
                    reporting_state: *reporting,
                    power_state: *power,
                };
                Ok(())
            }
            _ => Err(Errno::INVAL),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT_ID: u8 = 1;

    fn temperature(centi: i32) -> Sample {
        Sample {
            quantity: Quantity::Temperature,
            centi,
        }
    }

    /// Totals the bits of each main item kind in `descriptor`, from its
    /// Report Size and Report Count globals; checks collections nest.
    fn report_bits(descriptor: &[u8]) -> (usize, usize) {
        const INPUT: u8 = 0x80;
        const FEATURE: u8 = 0xb0;
        const COLLECTION: u8 = 0xa0;
        const END_COLLECTION: u8 = 0xc0;
        const REPORT_SIZE: u8 = 0x74;
        const REPORT_COUNT: u8 = 0x94;
        let (mut size, mut count, mut depth) = (0_usize, 0_usize, 0_i32);
        let (mut input, mut feature) = (0, 0);
        let mut rest = descriptor;
        while let Some((&prefix, tail)) = rest.split_first() {
            let len = match prefix & 0x03 {
                3 => 4,
                n => usize::from(n),
            };
            let (data, tail) = tail.split_at(len);
            let value = data
                .iter()
                .rev()
                .fold(0_usize, |v, &b| (v << 8) | usize::from(b));
            match prefix & 0xfc {
                REPORT_SIZE => size = value,
                REPORT_COUNT => count = value,
                INPUT => input += size * count,
                FEATURE => feature += size * count,
                COLLECTION => depth += 1,
                END_COLLECTION => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0, "unbalanced End Collection");
            rest = tail;
        }
        assert_eq!(depth, 0, "unclosed collection");
        (input, feature)
    }

    #[test]
    fn descriptor_matches_report_lengths() {
        let (input, feature) = report_bits(&Sensor::new(&[temperature(0)]).descriptor());
        assert_eq!(1 + input / 8, INPUT_REPORT_LEN);
        assert_eq!(1 + feature / 8, FEATURE_REPORT_LEN);
    }

    /// The temperature collection as released in 2.0.0, which the
    /// kernel tests and the field have exercised.
    const RELEASED_TEMPERATURE_DESCRIPTOR: &str = "\
        05200933a10185010a16031501250275089501a1020a40080a4108b100c00a19\
        031501250675089501a1020a50080a51080a52080a53080a54080a5508b100c0\
        0a0e03150027ffffff7f7520950165005500b1020a3414150027ffff00007510\
        95016500550eb1020a340416008026ff7f752095016500550e8102c0";

    #[test]
    fn temperature_descriptor_unchanged() {
        let hex = Sensor::new(&[temperature(0)])
            .descriptor()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(hex, RELEASED_TEMPERATURE_DESCRIPTOR);
    }

    fn get(sensor: &mut Sensor, kind: ReportType) -> Reply {
        let event = FromKernel::GetReport {
            id: RequestId::new(5),
            number: ReportNumber(REPORT_ID),
            kind,
        };
        sensor.handle(&event).unwrap()
    }

    #[test]
    fn input_report_carries_reading() {
        let mut sensor = Sensor::new(&[temperature(3512)]);
        assert_eq!(
            get(&mut sensor, ReportType::Input),
            Reply::GetReport {
                id: RequestId::new(5),
                result: Ok(vec![REPORT_ID, 0xb8, 0x0d, 0, 0]),
            }
        );
        assert_eq!(
            sensor.update(&[temperature(-1000)]),
            [[REPORT_ID, 0x18, 0xfc, 0xff, 0xff]]
        );
    }

    #[test]
    fn feature_report_defaults() {
        let mut sensor = Sensor::new(&[temperature(0)]);
        assert_eq!(
            get(&mut sensor, ReportType::Feature),
            Reply::GetReport {
                id: RequestId::new(5),
                result: Ok(vec![REPORT_ID, 1, 2, 0, 0, 0, 0, 0, 0]),
            }
        );
    }

    #[test]
    fn set_feature_stores_states_and_drops_the_rest() {
        let mut sensor = Sensor::new(&[temperature(0)]);
        let set = FromKernel::SetReport {
            id: RequestId::new(6),
            number: ReportNumber(REPORT_ID),
            kind: ReportType::Feature,
            data: vec![REPORT_ID, 2, 6, 0xe8, 0x03, 0, 0, 7, 0],
        };
        assert_eq!(
            sensor.handle(&set),
            Some(Reply::SetReport {
                id: RequestId::new(6),
                result: Ok(())
            })
        );
        let Reply::GetReport { result, .. } = get(&mut sensor, ReportType::Feature) else {
            panic!("not a GetReport reply");
        };
        assert_eq!(result.unwrap(), [REPORT_ID, 2, 6, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn rejects_unknown_reports() {
        let mut sensor = Sensor::new(&[temperature(0)]);
        let get_other = FromKernel::GetReport {
            id: RequestId::new(1),
            number: ReportNumber(2),
            kind: ReportType::Feature,
        };
        assert!(matches!(
            sensor.handle(&get_other),
            Some(Reply::GetReport {
                result: Err(Errno::INVAL),
                ..
            })
        ));
        let short_set = FromKernel::SetReport {
            id: RequestId::new(2),
            number: ReportNumber(REPORT_ID),
            kind: ReportType::Feature,
            data: vec![REPORT_ID, 1],
        };
        assert!(matches!(
            sensor.handle(&short_set),
            Some(Reply::SetReport {
                result: Err(Errno::INVAL),
                ..
            })
        ));
    }

    #[test]
    fn humidity_has_its_own_collection() {
        let humidity = Sample {
            quantity: Quantity::Humidity,
            centi: 3101,
        };
        let mut sensor = Sensor::new(&[temperature(3512), humidity]);
        let (input, feature) = report_bits(&sensor.descriptor());
        assert_eq!(input / 8, 2 * (INPUT_REPORT_LEN - 1));
        assert_eq!(feature / 8, 2 * (FEATURE_REPORT_LEN - 1));
        let event = FromKernel::GetReport {
            id: RequestId::new(7),
            number: ReportNumber(2),
            kind: ReportType::Input,
        };
        assert_eq!(
            sensor.handle(&event),
            Some(Reply::GetReport {
                id: RequestId::new(7),
                result: Ok(vec![2, 0x1d, 0x0c, 0, 0]),
            })
        );
        assert_eq!(
            sensor.update(&[temperature(-1000)]),
            [
                vec![REPORT_ID, 0x18, 0xfc, 0xff, 0xff],
                vec![2, 0x1d, 0x0c, 0, 0]
            ]
        );
    }

    #[test]
    fn sample_rounds_to_hundredths() {
        assert_eq!(Sample::new(Quantity::Temperature, 33.88).centi, 3388);
        assert_eq!(Sample::new(Quantity::Humidity, -0.05).centi, -5);
    }

    #[test]
    fn lifecycle_events_need_no_reply() {
        let mut sensor = Sensor::new(&[temperature(0)]);
        assert_eq!(sensor.handle(&FromKernel::Open), None);
        assert_eq!(sensor.handle(&FromKernel::Close), None);
    }
}

/// The descriptor and replies against the real kernel drivers; needs
/// root for `/dev/uhid` and the `hid_sensor_hub` and
/// `hid_sensor_temperature` modules.
#[cfg(test)]
mod kernel_tests {
    use std::env;
    use std::fs;
    use std::fs::File;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::Duration;
    use std::time::Instant;

    use rustix::event::PollFd;
    use rustix::event::PollFlags;
    use rustix::event::Timespec;

    use super::*;
    use crate::iio;
    use crate::test_support;
    use crate::uhid;
    use crate::uhid::Bus;
    use crate::uhid::Create2;
    use crate::uhid::read_event;
    use crate::uhid::write_event;

    const UNIQ: &str = "temper-iio-sensor-test";
    const TEMPERATURE: Sample = Sample {
        quantity: Quantity::Temperature,
        centi: 3512,
    };
    const HUMIDITY: Sample = Sample {
        quantity: Quantity::Humidity,
        centi: 3101,
    };

    /// How long the drivers may take to bind and register.
    const SETUP_TIMEOUT: Duration = Duration::from_secs(10);

    /// How often the serving thread checks whether to stop.
    const POLL_PERIOD: Duration = Duration::from_millis(100);

    /// Longer than the drivers' 3 s runtime autosuspend delay.
    const IDLE: Duration = Duration::from_secs(4);

    /// A read must not wait on anything like the kernel's 5 s request
    /// timeout or a report-interval sleep.
    const FAST_READ: Duration = Duration::from_millis(500);

    /// Opt-in for [`two_sensors_survive_a_destroy`].
    const TWO_SENSORS: &str = "TEMPER_IIO_TWO_SENSORS";

    /// How long the surviving sensor keeps sending input reports.
    const FLOOD: Duration = Duration::from_secs(2);

    /// Answers the kernel's requests from `sensor` until `stop`.
    fn serve(uhid: &File, mut sensor: Sensor, stop: &AtomicBool) {
        let timeout = Timespec::try_from(POLL_PERIOD).unwrap();
        while !stop.load(Ordering::Relaxed) {
            let mut fds = [PollFd::new(uhid, PollFlags::IN)];
            if rustix::event::poll(&mut fds, Some(&timeout)).unwrap() == 0 {
                continue;
            }
            if let Some(reply) = sensor.handle(&read_event(uhid).unwrap()) {
                write_event(uhid, reply.event()).unwrap();
            }
        }
    }

    /// A virtual sensor served from fixed samples.
    #[derive(Debug)]
    struct TestSensor {
        uhid: Arc<File>,
        stop: Arc<AtomicBool>,
        server: Option<thread::JoinHandle<()>>,
        /// Its IIO devices, in sample order.
        iio: Vec<PathBuf>,
    }

    impl TestSensor {
        fn create(uniq: &str, samples: &[Sample]) -> Self {
            let sensor = Sensor::new(samples);
            let uhid = Arc::new(uhid::open().unwrap());
            let create = Create2 {
                name: "temper-iio sensor test".to_owned(),
                phys: uniq.to_owned(),
                uniq: uniq.to_owned(),
                bus: Bus::VIRTUAL,
                vendor: 0,
                product: 0,
                version: 0,
                country: 0,
                descriptor: sensor.descriptor(),
            };
            write_event(&uhid, ToKernel::Create2(&create)).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let server = {
                let uhid = Arc::clone(&uhid);
                let stop = Arc::clone(&stop);
                thread::spawn(move || serve(&uhid, sensor, &stop))
            };
            let start = Instant::now();
            let iio = loop {
                let found = samples
                    .iter()
                    .map(|sample| iio::find(uniq, sample.quantity.iio_name()))
                    .collect::<Option<Vec<_>>>();
                if let Some(paths) = found {
                    break paths;
                }
                assert!(start.elapsed() < SETUP_TIMEOUT, "no IIO devices appeared");
                thread::sleep(POLL_PERIOD);
            };
            Self {
                uhid,
                stop,
                server: Some(server),
                iio,
            }
        }

        /// Destroys the device while still serving: removal may issue
        /// requests.
        fn destroy(&mut self) {
            if let Some(server) = self.server.take() {
                write_event(&self.uhid, ToKernel::Destroy).unwrap();
                self.stop.store(true, Ordering::Relaxed);
                server.join().unwrap();
            }
        }
    }

    impl Drop for TestSensor {
        fn drop(&mut self) {
            self.destroy();
        }
    }

    /// Reads an attribute, checking it answers quickly.
    fn read_attribute(iio: &Path, name: &str) -> String {
        let start = Instant::now();
        let value = fs::read_to_string(iio.join(name)).unwrap();
        let elapsed = start.elapsed();
        assert!(elapsed < FAST_READ, "{name} took {elapsed:?}");
        value.trim().to_owned()
    }

    /// Checks one IIO device's name, value and scale of 10.
    fn check_channel(iio: &Path, name: &str, channel: &str, sample: Sample) {
        assert_eq!(read_attribute(iio, "name"), name);
        assert_eq!(
            read_attribute(iio, &format!("in_{channel}_raw")),
            sample.centi.to_string()
        );
        let scale: f64 = read_attribute(iio, &format!("in_{channel}_scale"))
            .parse()
            .unwrap();
        assert!((scale - 10.0).abs() < f64::EPSILON, "scale {scale}");
        read_attribute(iio, &format!("in_{channel}_hysteresis"));
    }

    #[test]
    #[ignore = "needs root for /dev/uhid"]
    fn iio_device_reads_temperature() {
        let _one = test_support::one_sensor();
        let sensor = TestSensor::create(UNIQ, &[TEMPERATURE]);
        check_channel(&sensor.iio[0], "temperature", "temp", TEMPERATURE);

        thread::sleep(IDLE);
        assert_eq!(
            read_attribute(&sensor.iio[0], "in_temp_raw"),
            TEMPERATURE.centi.to_string()
        );
    }

    #[test]
    #[ignore = "needs root for /dev/uhid"]
    fn iio_devices_read_temperature_and_humidity() {
        let _one = test_support::one_sensor();
        let sensor = TestSensor::create(UNIQ, &[TEMPERATURE, HUMIDITY]);
        check_channel(&sensor.iio[0], "temperature", "temp", TEMPERATURE);
        check_channel(&sensor.iio[1], "humidity", "humidityrelative", HUMIDITY);

        thread::sleep(IDLE);
        assert_eq!(
            read_attribute(&sensor.iio[1], "in_humidityrelative_raw"),
            HUMIDITY.centi.to_string()
        );
    }

    /// The kernel bug in `iio::sensor`'s comment: with two
    /// sensors, destroying the second while the first sends input
    /// reports.  A stock `hid-sensor-temperature` oopses here, so this
    /// runs only with `TEMPER_IIO_TWO_SENSORS=1`, against a kernel carrying
    /// the per-instance callbacks fix (`patches/`).
    #[test]
    #[ignore = "needs root, and oopses a kernel without the patches/ fix"]
    fn two_sensors_survive_a_destroy() {
        if env::var_os(TWO_SENSORS).is_none() {
            eprintln!("skipped: set {TWO_SENSORS}=1 on a patched kernel");
            return;
        }
        let _one = test_support::one_sensor();
        let first = TestSensor::create("temper-iio-two-first", &[TEMPERATURE]);
        let mut second = TestSensor::create("temper-iio-two-second", &[TEMPERATURE]);
        let [report] = Sensor::new(&[TEMPERATURE])
            .update(&[TEMPERATURE])
            .try_into()
            .unwrap();
        let flooding = {
            let uhid = Arc::clone(&first.uhid);
            thread::spawn(move || {
                let start = Instant::now();
                while start.elapsed() < FLOOD {
                    write_event(&uhid, ToKernel::Input2(&report)).unwrap();
                }
            })
        };
        second.destroy();
        flooding.join().unwrap();
        assert_eq!(
            read_attribute(&first.iio[0], "in_temp_raw"),
            TEMPERATURE.centi.to_string()
        );
    }
}
