//! The HID sensor the daemon presents through uhid: its report
//! descriptor, and the state that answers the kernel's report requests.
//!
//! Sources, all in the kernel tree: `include/linux/hid-sensor-ids.h`
//! (usages), `drivers/hid/hid-sensor-hub.c` (`sensor_hub_input_get_
//! attribute_info()`: a field is found by its physical collection's
//! usage, and by its first usage or its logical collection's usage),
//! `drivers/iio/common/hid-sensors/hid-sensor-attributes.c` and
//! `hid-sensor-trigger.c` (which properties are read and written), and
//! `drivers/iio/temperature/hid-sensor-temperature.c`.  Item encodings
//! follow the HID 1.11 specification, section 6.2.2.  See `PLAN.md`,
//! "Report descriptor requirements".

use rustix::io::Errno;
use tempered_hid::protocol::CentiCelsius;

use crate::uhid::FromKernel;
use crate::uhid::ReportNumber;
use crate::uhid::ReportType;
use crate::uhid::RequestId;
use crate::uhid::ToKernel;

/// The report ID of both the feature and the input report.  Numbered,
/// because kernel 7.0 reserves a leading byte for unnumbered reports.
const REPORT_ID: u8 = 1;

/// Named array values are 1-based: the kernel forces Logical Minimum to
/// 1 for power and reporting state (commit b0f847e16c1e), and the
/// descriptor declares the same.
const ENUM_BASE: u8 = 1;

/// Reporting State selectors, in descriptor order.
const REPORTING_STATE_NO_EVENTS: u8 = ENUM_BASE;

/// Power State selectors, in descriptor order: Undefined, then D0.
const POWER_STATE_D0_FULL_POWER: u8 = ENUM_BASE + 1;

/// The report descriptor.  One physical collection, the temperature
/// sensor, holding a feature report and an input report, both ID 1.
#[rustfmt::skip]
pub(crate) const DESCRIPTOR: &[u8] = &[
    0x05, 0x20,                   // Usage Page (Sensors)
    0x09, 0x33,                   // Usage (Environmental: Temperature)
    0xa1, 0x00,                   // Collection (Physical)
    0x85, REPORT_ID,              //   Report ID (1)

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
    0x27, 0xff, 0xff, 0xff, 0xff, //   Logical Maximum (4294967295)
    0x75, 0x20,                   //   Report Size (32)
    0x95, 0x01,                   //   Report Count (1)
    0x65, 0x00,                   //   Unit (None)
    0x55, 0x00,                   //   Unit Exponent (0)
    0xb1, 0x02,                   //   Feature (Data, Variable, Absolute)

    // Always 0; present so in_temp_hysteresis reads succeed.
    0x0a, 0x34, 0x14,             //   Usage (Change Sensitivity Absolute | Temperature)
    0x15, 0x00,                   //   Logical Minimum (0)
    0x27, 0xff, 0xff, 0x00, 0x00, //   Logical Maximum (65535)
    0x75, 0x10,                   //   Report Size (16)
    0x95, 0x01,                   //   Report Count (1)
    0x65, 0x00,                   //   Unit (None)
    0x55, 0x0e,                   //   Unit Exponent (-2)
    0xb1, 0x02,                   //   Feature (Data, Variable, Absolute)

    // Input report.  Unit None with exponent -2: the kernel's scale
    // table matches only Unit 0 or degrees, giving in_temp_scale 10, so
    // raw centi-degrees C read as milli-degrees C.
    0x0a, 0x34, 0x04,             //   Usage (Data: Environmental Temperature)
    0x16, 0x00, 0x80,             //   Logical Minimum (-32768)
    0x26, 0xff, 0x7f,             //   Logical Maximum (32767)
    0x75, 0x10,                   //   Report Size (16)
    0x95, 0x01,                   //   Report Count (1)
    0x65, 0x00,                   //   Unit (None)
    0x55, 0x0e,                   //   Unit Exponent (-2)
    0x81, 0x02,                   //   Input (Data, Variable, Absolute)
    0xc0,                         // End Collection
];

/// Feature report length: ID, reporting state, power state, report
/// interval (u32), sensitivity (u16).
const FEATURE_REPORT_LEN: usize = 1 + 1 + 1 + 4 + 2;

/// Input report length: ID, temperature (i16).
const INPUT_REPORT_LEN: usize = 1 + 2;

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

/// What the virtual sensor reports: the latest reading and the
/// feature values.  There is always a reading: the daemon creates the
/// device only once it has one.
#[derive(Debug)]
pub(crate) struct Sensor {
    temperature: CentiCelsius,
    features: Features,
}

impl Sensor {
    pub(crate) fn new(temperature: CentiCelsius) -> Self {
        Self {
            temperature,
            features: Features {
                reporting_state: REPORTING_STATE_NO_EVENTS,
                power_state: POWER_STATE_D0_FULL_POWER,
            },
        }
    }

    /// Records a new reading and returns the input report to push.
    pub(crate) fn update(&mut self, temperature: CentiCelsius) -> Vec<u8> {
        self.temperature = temperature;
        self.input_report()
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

    fn get_report(&self, number: ReportNumber, kind: ReportType) -> Result<Vec<u8>, Errno> {
        match (number, kind) {
            (ReportNumber(REPORT_ID), ReportType::Feature) => Ok(self.feature_report()),
            (ReportNumber(REPORT_ID), ReportType::Input) => Ok(self.input_report()),
            _ => Err(Errno::INVAL),
        }
    }

    /// Stores the reporting and power states; the rest is ignored.
    fn set_report(
        &mut self,
        number: ReportNumber,
        kind: ReportType,
        data: &[u8],
    ) -> Result<(), Errno> {
        match (number, kind, data) {
            (ReportNumber(REPORT_ID), ReportType::Feature, [REPORT_ID, reporting, power, ..])
                if data.len() == FEATURE_REPORT_LEN =>
            {
                self.features = Features {
                    reporting_state: *reporting,
                    power_state: *power,
                };
                Ok(())
            }
            _ => Err(Errno::INVAL),
        }
    }

    /// Report interval and sensitivity are always 0.
    fn feature_report(&self) -> Vec<u8> {
        let mut report = Vec::with_capacity(FEATURE_REPORT_LEN);
        report.extend_from_slice(&[
            REPORT_ID,
            self.features.reporting_state,
            self.features.power_state,
        ]);
        report.extend_from_slice(&0_u32.to_le_bytes());
        report.extend_from_slice(&0_u16.to_le_bytes());
        report
    }

    fn input_report(&self) -> Vec<u8> {
        let mut report = Vec::with_capacity(INPUT_REPORT_LEN);
        report.push(REPORT_ID);
        report.extend_from_slice(&self.temperature.get().to_le_bytes());
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (input, feature) = report_bits(DESCRIPTOR);
        assert_eq!(1 + input / 8, INPUT_REPORT_LEN);
        assert_eq!(1 + feature / 8, FEATURE_REPORT_LEN);
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
        let mut sensor = Sensor::new(CentiCelsius::new(3512));
        assert_eq!(
            get(&mut sensor, ReportType::Input),
            Reply::GetReport {
                id: RequestId::new(5),
                result: Ok(vec![REPORT_ID, 0xb8, 0x0d]),
            }
        );
        assert_eq!(
            sensor.update(CentiCelsius::new(-1000)),
            [REPORT_ID, 0x18, 0xfc]
        );
    }

    #[test]
    fn feature_report_defaults() {
        let mut sensor = Sensor::new(CentiCelsius::new(0));
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
        let mut sensor = Sensor::new(CentiCelsius::new(0));
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
        let mut sensor = Sensor::new(CentiCelsius::new(0));
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
    fn lifecycle_events_need_no_reply() {
        let mut sensor = Sensor::new(CentiCelsius::new(0));
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

    const UNIQ: &str = "tempered-sensor-test";
    const TEMPERATURE: CentiCelsius = CentiCelsius::new(3512);

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
    const TWO_SENSORS: &str = "TEMPERED_TWO_SENSORS";

    /// How long the surviving sensor keeps sending input reports.
    const FLOOD: Duration = Duration::from_secs(2);

    /// Answers the kernel's requests from a [`Sensor`] until `stop`.
    fn serve(uhid: &File, stop: &AtomicBool) {
        let mut sensor = Sensor::new(TEMPERATURE);
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

    /// A virtual temperature sensor served from [`TEMPERATURE`].
    #[derive(Debug)]
    struct TestSensor {
        uhid: Arc<File>,
        stop: Arc<AtomicBool>,
        server: Option<thread::JoinHandle<()>>,
        /// Its IIO device.
        iio: PathBuf,
    }

    impl TestSensor {
        fn create(uniq: &str) -> Self {
            let uhid = Arc::new(uhid::open().unwrap());
            let create = Create2 {
                name: "tempered sensor test".to_owned(),
                phys: uniq.to_owned(),
                uniq: uniq.to_owned(),
                bus: Bus::VIRTUAL,
                vendor: 0,
                product: 0,
                version: 0,
                country: 0,
                descriptor: DESCRIPTOR.to_vec(),
            };
            write_event(&uhid, ToKernel::Create2(&create)).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let server = {
                let uhid = Arc::clone(&uhid);
                let stop = Arc::clone(&stop);
                thread::spawn(move || serve(&uhid, &stop))
            };
            let start = Instant::now();
            let iio = loop {
                if let Some(path) = iio::find(uniq) {
                    break path;
                }
                assert!(start.elapsed() < SETUP_TIMEOUT, "no IIO device appeared");
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

    #[test]
    #[ignore = "needs root for /dev/uhid"]
    fn iio_device_reads_temperature() {
        let _one = test_support::one_temperature_sensor();
        let sensor = TestSensor::create(UNIQ);
        assert_eq!(read_attribute(&sensor.iio, "name"), "temperature");
        assert_eq!(
            read_attribute(&sensor.iio, "in_temp_raw"),
            TEMPERATURE.get().to_string()
        );
        let scale: f64 = read_attribute(&sensor.iio, "in_temp_scale")
            .parse()
            .unwrap();
        assert!((scale - 10.0).abs() < f64::EPSILON, "scale {scale}");
        read_attribute(&sensor.iio, "in_temp_hysteresis");

        thread::sleep(IDLE);
        assert_eq!(
            read_attribute(&sensor.iio, "in_temp_raw"),
            TEMPERATURE.get().to_string()
        );
    }

    /// The kernel bug in `iio::temperature_sensor`'s comment: with two
    /// sensors, destroying the second while the first sends input
    /// reports.  A stock `hid-sensor-temperature` oopses here, so this
    /// runs only with `TEMPERED_TWO_SENSORS=1`, against a kernel carrying
    /// the per-instance callbacks fix (`patches/`).
    #[test]
    #[ignore = "needs root, and oopses a kernel without the patches/ fix"]
    fn two_sensors_survive_a_destroy() {
        if env::var_os(TWO_SENSORS).is_none() {
            eprintln!("skipped: set {TWO_SENSORS}=1 on a patched kernel");
            return;
        }
        let _one = test_support::one_temperature_sensor();
        let first = TestSensor::create("tempered-two-first");
        let mut second = TestSensor::create("tempered-two-second");
        let report = Sensor::new(TEMPERATURE).update(TEMPERATURE);
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
            read_attribute(&first.iio, "in_temp_raw"),
            TEMPERATURE.get().to_string()
        );
    }
}
