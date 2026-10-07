//! Encoding and decoding `struct uhid_event`, the messages exchanged
//! with the kernel through `/dev/uhid`.
//!
//! Source: `include/uapi/linux/uhid.h`.  Every struct is packed and
//! native-endian.  The kernel zero-fills a short write, so an event is
//! written only as far as its last meaningful byte; a read, though,
//! must offer [`EVENT_SIZE`] bytes or the event is truncated and lost
//! (`uhid_char_read()` in `drivers/hid/uhid.c`).

use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Write;

use rustix::io::Errno;

/// `UHID_DATA_MAX`: largest report payload.
const DATA_MAX: usize = 4096;

/// `HID_MAX_DESCRIPTOR_SIZE` from `include/uapi/linux/hid.h`.
const DESCRIPTOR_MAX: usize = 4096;

/// Sizes of `struct uhid_create2_req`'s string fields, including the
/// terminating NUL the kernel's `strscpy()` needs.
const NAME_SIZE: usize = 128;
const PHYS_SIZE: usize = 64;
const UNIQ_SIZE: usize = 64;

/// Size of the `type` field that heads every event.
const TYPE_SIZE: usize = 4;

/// Offset of `rd_data` in `struct uhid_create2_req`: three strings, then
/// `rd_size` and `bus` (u16 each), then `vendor`, `product`, `version`
/// and `country` (u32 each).
const CREATE2_HEADER_SIZE: usize = NAME_SIZE + PHYS_SIZE + UNIQ_SIZE + 2 * 2 + 4 * 4;

/// `sizeof(struct uhid_event)`: the type plus the largest union member,
/// `struct uhid_create2_req`.
pub(crate) const EVENT_SIZE: usize = TYPE_SIZE + CREATE2_HEADER_SIZE + DESCRIPTOR_MAX;

/// `enum uhid_event_type` values this module uses.
mod event_type {
    pub(super) const DESTROY: u32 = 1;
    pub(super) const START: u32 = 2;
    pub(super) const STOP: u32 = 3;
    pub(super) const OPEN: u32 = 4;
    pub(super) const CLOSE: u32 = 5;
    pub(super) const OUTPUT: u32 = 6;
    pub(super) const GET_REPORT: u32 = 9;
    pub(super) const GET_REPORT_REPLY: u32 = 10;
    pub(super) const CREATE2: u32 = 11;
    pub(super) const INPUT2: u32 = 12;
    pub(super) const SET_REPORT: u32 = 13;
    pub(super) const SET_REPORT_REPLY: u32 = 14;
}

/// A HID bus type, as in `include/uapi/linux/input.h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bus(pub(crate) u16);

impl Bus {
    /// `BUS_VIRTUAL`.
    pub(crate) const VIRTUAL: Self = Self(0x06);
}

/// Identifies a `GET_REPORT` or `SET_REPORT` request; its reply must carry
/// the same id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RequestId(u32);

impl RequestId {
    #[cfg(test)]
    pub(crate) const fn new(raw: u32) -> Self {
        Self(raw)
    }
}

/// A report number (report ID); 0 when the device uses none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReportNumber(pub(crate) u8);

/// `enum uhid_report_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReportType {
    Feature,
    Output,
    Input,
    /// A value this module does not know.
    Unknown(u8),
}

impl ReportType {
    fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Feature,
            1 => Self::Output,
            2 => Self::Input,
            _ => Self::Unknown(raw),
        }
    }
}

/// `struct uhid_start_req`'s `dev_flags`: which report kinds the
/// device's descriptor numbers (`enum uhid_dev_flag`: bit 0 feature,
/// bit 1 output, bit 2 input).  Only logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DevFlags(u64);

/// The device to create: `struct uhid_create2_req`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Create2 {
    /// `HID_NAME`; at most 127 bytes, no NUL.
    pub(crate) name: String,
    /// `HID_PHYS`; at most 63 bytes, no NUL.
    pub(crate) phys: String,
    /// `HID_UNIQ`; at most 63 bytes, no NUL.
    pub(crate) uniq: String,
    pub(crate) bus: Bus,
    pub(crate) vendor: u32,
    pub(crate) product: u32,
    pub(crate) version: u32,
    pub(crate) country: u32,
    /// The report descriptor; 1 to 4096 bytes.
    pub(crate) descriptor: Vec<u8>,
}

/// An event this process writes to the kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToKernel<'a> {
    Create2(&'a Create2),
    Destroy,
    /// An input report, report number first if the device numbers them.
    Input2(&'a [u8]),
    /// Answers a [`FromKernel::GetReport`].
    GetReportReply {
        id: RequestId,
        result: Result<&'a [u8], Errno>,
    },
    /// Answers a [`FromKernel::SetReport`].
    SetReportReply {
        id: RequestId,
        result: Result<(), Errno>,
    },
}

/// An event the kernel sends this process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FromKernel {
    /// The HID driver bound; `flags` says which reports are numbered.
    Start { flags: DevFlags },
    /// The HID driver unbound.
    Stop,
    /// A reader opened the device; input reports are wanted.
    Open,
    /// The last reader closed the device.
    Close,
    /// An output report to deliver to the device.
    Output { kind: ReportType, data: Vec<u8> },
    /// The kernel wants a report; answer with
    /// [`ToKernel::GetReportReply`] carrying `id`.
    GetReport {
        id: RequestId,
        number: ReportNumber,
        kind: ReportType,
    },
    /// The kernel sets a report; answer with
    /// [`ToKernel::SetReportReply`] carrying `id`.
    SetReport {
        id: RequestId,
        number: ReportNumber,
        kind: ReportType,
        data: Vec<u8>,
    },
    /// An event type this module does not handle.
    Unknown(u32),
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum EncodeError {
    #[error("{field} is {len} bytes; at most {max} fit")]
    StringTooLong {
        field: &'static str,
        len: usize,
        max: usize,
    },
    #[error("{0} contains a NUL byte")]
    StringHasNul(&'static str),
    #[error("report descriptor is {0} bytes; 1 to {DESCRIPTOR_MAX} fit")]
    DescriptorSize(usize),
    #[error("report is {0} bytes; at most {DATA_MAX} fit")]
    DataTooLong(usize),
    #[error("error reply carries errno {0}, which does not fit the reply")]
    ErrnoRange(i32),
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DecodeError {
    #[error("event of {len} bytes is too short for type {event_type}")]
    TooShort { event_type: u32, len: usize },
    #[error("event of {0} bytes has no type")]
    NoType(usize),
    #[error("report size {0} exceeds {DATA_MAX}")]
    DataTooLong(usize),
}

/// Appends native-endian integers and byte strings.
#[derive(Debug, Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u16(&mut self, value: u16) {
        self.0.extend_from_slice(&value.to_ne_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_ne_bytes());
    }

    fn bytes(&mut self, value: &[u8]) {
        self.0.extend_from_slice(value);
    }

    /// A NUL-padded fixed-size string field.
    fn string(&mut self, field: &'static str, value: &str, size: usize) -> Result<(), EncodeError> {
        let max = size - 1;
        if value.len() > max {
            return Err(EncodeError::StringTooLong {
                field,
                len: value.len(),
                max,
            });
        }
        if value.as_bytes().contains(&0) {
            return Err(EncodeError::StringHasNul(field));
        }
        self.bytes(value.as_bytes());
        self.0.resize(self.0.len() + size - value.len(), 0);
        Ok(())
    }

    /// A u16 length, as `size` fields carry it.
    fn len16(&mut self, len: usize) {
        self.u16(u16::try_from(len).expect("length checked against DATA_MAX"));
    }

    /// A report payload of at most [`DATA_MAX`] bytes.
    fn data(&mut self, data: &[u8]) -> Result<(), EncodeError> {
        if data.len() > DATA_MAX {
            return Err(EncodeError::DataTooLong(data.len()));
        }
        self.bytes(data);
        Ok(())
    }
}

/// The `err` field of a reply: 0 or a positive errno.
fn reply_err(result: Result<(), Errno>) -> Result<u16, EncodeError> {
    match result {
        Ok(()) => Ok(0),
        Err(errno) => {
            let raw = errno.raw_os_error();
            u16::try_from(raw).map_err(|_| EncodeError::ErrnoRange(raw))
        }
    }
}

/// Encodes `event` for a single `write()` to `/dev/uhid`.
pub(crate) fn encode(event: ToKernel<'_>) -> Result<Vec<u8>, EncodeError> {
    let mut w = Writer::default();
    match event {
        ToKernel::Create2(create) => {
            let rd_size = create.descriptor.len();
            if !(1..=DESCRIPTOR_MAX).contains(&rd_size) {
                return Err(EncodeError::DescriptorSize(rd_size));
            }
            w.u32(event_type::CREATE2);
            w.string("name", &create.name, NAME_SIZE)?;
            w.string("phys", &create.phys, PHYS_SIZE)?;
            w.string("uniq", &create.uniq, UNIQ_SIZE)?;
            w.len16(rd_size);
            w.u16(create.bus.0);
            w.u32(create.vendor);
            w.u32(create.product);
            w.u32(create.version);
            w.u32(create.country);
            w.bytes(&create.descriptor);
        }
        ToKernel::Destroy => w.u32(event_type::DESTROY),
        ToKernel::Input2(data) => {
            w.u32(event_type::INPUT2);
            w.len16(data.len().min(DATA_MAX));
            w.data(data)?;
        }
        ToKernel::GetReportReply { id, result } => {
            let data = result.unwrap_or_default();
            w.u32(event_type::GET_REPORT_REPLY);
            w.u32(id.0);
            w.u16(reply_err(result.map(drop))?);
            w.len16(data.len().min(DATA_MAX));
            w.data(data)?;
        }
        ToKernel::SetReportReply { id, result } => {
            w.u32(event_type::SET_REPORT_REPLY);
            w.u32(id.0);
            w.u16(reply_err(result)?);
        }
    }
    Ok(w.0)
}

/// Reads native-endian integers from an event body.
#[derive(Debug)]
struct Reader<'a> {
    event_type: u32,
    rest: &'a [u8],
    len: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.rest.len() < n {
            return Err(DecodeError::TooShort {
                event_type: self.event_type,
                len: self.len,
            });
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        Ok(self.take(N)?.try_into().expect("took N bytes"))
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        self.array().map(u16::from_ne_bytes)
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        self.array().map(u32::from_ne_bytes)
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        self.array().map(u64::from_ne_bytes)
    }
}

/// A `size` field, checked against [`DATA_MAX`].
fn data_size(size: u16) -> Result<usize, DecodeError> {
    let size = usize::from(size);
    if size > DATA_MAX {
        return Err(DecodeError::DataTooLong(size));
    }
    Ok(size)
}

/// Decodes one event as read from `/dev/uhid`.
pub(crate) fn decode(bytes: &[u8]) -> Result<FromKernel, DecodeError> {
    let (head, rest) = bytes
        .split_first_chunk::<TYPE_SIZE>()
        .ok_or(DecodeError::NoType(bytes.len()))?;
    let event_type = u32::from_ne_bytes(*head);
    let mut r = Reader {
        event_type,
        rest,
        len: bytes.len(),
    };
    Ok(match event_type {
        event_type::START => FromKernel::Start {
            flags: DevFlags(r.u64()?),
        },
        event_type::STOP => FromKernel::Stop,
        event_type::OPEN => FromKernel::Open,
        event_type::CLOSE => FromKernel::Close,
        event_type::OUTPUT => {
            // struct uhid_output_req: data[UHID_DATA_MAX], size, rtype.
            let data = r.take(DATA_MAX)?;
            let size = data_size(r.u16()?)?;
            let kind = ReportType::from_raw(r.u8()?);
            FromKernel::Output {
                kind,
                data: data[..size].to_vec(),
            }
        }
        event_type::GET_REPORT => FromKernel::GetReport {
            id: RequestId(r.u32()?),
            number: ReportNumber(r.u8()?),
            kind: ReportType::from_raw(r.u8()?),
        },
        event_type::SET_REPORT => {
            let id = RequestId(r.u32()?);
            let number = ReportNumber(r.u8()?);
            let kind = ReportType::from_raw(r.u8()?);
            let size = data_size(r.u16()?)?;
            FromKernel::SetReport {
                id,
                number,
                kind,
                data: r.take(size)?.to_vec(),
            }
        }
        other => FromKernel::Unknown(other),
    })
}

/// Reads one event, retrying `EINTR`.  The buffer is a full
/// [`EVENT_SIZE`], since a short read truncates and consumes the event.
pub(crate) fn read_event(mut uhid: &File) -> io::Result<FromKernel> {
    let mut buffer = vec![0; EVENT_SIZE];
    loop {
        match uhid.read(&mut buffer) {
            Ok(n) => {
                return decode(&buffer[..n])
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

/// Writes one event in a single `write()`, retrying `EINTR`: uhid takes
/// the whole event or nothing, so `write_all`, which could split it into
/// two events, is not used.  uhid's device lock is interruptible, so a
/// signal can interrupt even a write that `SA_RESTART` would restart.
pub(crate) fn write_event(mut uhid: &File, event: ToKernel<'_>) -> io::Result<()> {
    let bytes =
        encode(event).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    loop {
        match uhid.write(&bytes) {
            Ok(n) if n == bytes.len() => return Ok(()),
            Ok(n) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    format!("uhid took {n} of {} bytes", bytes.len()),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds expected bytes field by field, independently of `Writer`.
    fn concat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    fn padded(value: &str, size: usize) -> Vec<u8> {
        let mut bytes = value.as_bytes().to_vec();
        bytes.resize(size, 0);
        bytes
    }

    fn create2() -> Create2 {
        Create2 {
            name: "PCsensor TEMPerGold".to_owned(),
            phys: "tempered".to_owned(),
            uniq: "temperature".to_owned(),
            bus: Bus::VIRTUAL,
            vendor: 0x3553,
            product: 0xa001,
            version: 1,
            country: 0,
            descriptor: vec![0x05, 0x20, 0xc0],
        }
    }

    #[test]
    fn event_size_matches_uapi() {
        // 4 + 128 + 64 + 64 + 2 + 2 + 4 * 4 + 4096.
        assert_eq!(EVENT_SIZE, 4376);
    }

    #[test]
    fn create2_bytes() {
        let expected = concat(&[
            &11_u32.to_ne_bytes(),
            &padded("PCsensor TEMPerGold", 128),
            &padded("tempered", 64),
            &padded("temperature", 64),
            &3_u16.to_ne_bytes(),
            &6_u16.to_ne_bytes(),
            &0x3553_u32.to_ne_bytes(),
            &0xa001_u32.to_ne_bytes(),
            &1_u32.to_ne_bytes(),
            &0_u32.to_ne_bytes(),
            &[0x05, 0x20, 0xc0],
        ]);
        assert_eq!(encode(ToKernel::Create2(&create2())).unwrap(), expected);
    }

    #[test]
    fn create2_rejects_long_name() {
        let create = Create2 {
            name: "x".repeat(NAME_SIZE),
            ..create2()
        };
        assert_eq!(
            encode(ToKernel::Create2(&create)),
            Err(EncodeError::StringTooLong {
                field: "name",
                len: 128,
                max: 127
            })
        );
    }

    #[test]
    fn create2_accepts_longest_name() {
        let create = Create2 {
            name: "x".repeat(NAME_SIZE - 1),
            ..create2()
        };
        assert!(encode(ToKernel::Create2(&create)).is_ok());
    }

    #[test]
    fn create2_rejects_nul() {
        let create = Create2 {
            uniq: "a\0b".to_owned(),
            ..create2()
        };
        assert_eq!(
            encode(ToKernel::Create2(&create)),
            Err(EncodeError::StringHasNul("uniq"))
        );
    }

    #[test]
    fn create2_rejects_descriptor_sizes() {
        for len in [0, DESCRIPTOR_MAX + 1] {
            let create = Create2 {
                descriptor: vec![0; len],
                ..create2()
            };
            assert_eq!(
                encode(ToKernel::Create2(&create)),
                Err(EncodeError::DescriptorSize(len))
            );
        }
    }

    #[test]
    fn destroy_bytes() {
        assert_eq!(encode(ToKernel::Destroy).unwrap(), 1_u32.to_ne_bytes());
    }

    #[test]
    fn input2_bytes() {
        let expected = concat(&[
            &12_u32.to_ne_bytes(),
            &3_u16.to_ne_bytes(),
            &[1, 0xb8, 0x0d],
        ]);
        assert_eq!(
            encode(ToKernel::Input2(&[1, 0xb8, 0x0d])).unwrap(),
            expected
        );
    }

    #[test]
    fn input2_rejects_oversize() {
        let data = vec![0; DATA_MAX + 1];
        assert_eq!(
            encode(ToKernel::Input2(&data)),
            Err(EncodeError::DataTooLong(DATA_MAX + 1))
        );
    }

    #[test]
    fn get_report_reply_bytes() {
        let expected = concat(&[
            &10_u32.to_ne_bytes(),
            &7_u32.to_ne_bytes(),
            &0_u16.to_ne_bytes(),
            &2_u16.to_ne_bytes(),
            &[2, 0],
        ]);
        let event = ToKernel::GetReportReply {
            id: RequestId(7),
            result: Ok(&[2, 0]),
        };
        assert_eq!(encode(event).unwrap(), expected);
    }

    #[test]
    fn get_report_reply_error_bytes() {
        let expected = concat(&[
            &10_u32.to_ne_bytes(),
            &7_u32.to_ne_bytes(),
            &5_u16.to_ne_bytes(),
            &0_u16.to_ne_bytes(),
        ]);
        let event = ToKernel::GetReportReply {
            id: RequestId(7),
            result: Err(Errno::IO),
        };
        assert_eq!(encode(event).unwrap(), expected);
    }

    #[test]
    fn set_report_reply_bytes() {
        let expected = concat(&[
            &14_u32.to_ne_bytes(),
            &9_u32.to_ne_bytes(),
            &0_u16.to_ne_bytes(),
        ]);
        let event = ToKernel::SetReportReply {
            id: RequestId(9),
            result: Ok(()),
        };
        assert_eq!(encode(event).unwrap(), expected);
    }

    /// A full-size event as `read()` returns it: `body` then zeros.
    fn from_kernel(event_type: u32, body: &[&[u8]]) -> Vec<u8> {
        let mut bytes = concat(&[&[&event_type.to_ne_bytes()[..]], body].concat());
        bytes.resize(EVENT_SIZE, 0);
        bytes
    }

    #[test]
    fn decodes_start() {
        let bytes = from_kernel(2, &[&0b101_u64.to_ne_bytes()]);
        assert_eq!(
            decode(&bytes).unwrap(),
            FromKernel::Start {
                flags: DevFlags(0b101)
            }
        );
    }

    #[test]
    fn decodes_bare_events() {
        assert_eq!(decode(&from_kernel(3, &[])).unwrap(), FromKernel::Stop);
        assert_eq!(decode(&from_kernel(4, &[])).unwrap(), FromKernel::Open);
        assert_eq!(decode(&from_kernel(5, &[])).unwrap(), FromKernel::Close);
        assert_eq!(
            decode(&from_kernel(99, &[])).unwrap(),
            FromKernel::Unknown(99)
        );
    }

    #[test]
    fn decodes_output() {
        let mut data = vec![0; DATA_MAX];
        data[..2].copy_from_slice(&[3, 4]);
        let bytes = from_kernel(6, &[&data, &2_u16.to_ne_bytes(), &[1]]);
        assert_eq!(
            decode(&bytes).unwrap(),
            FromKernel::Output {
                kind: ReportType::Output,
                data: vec![3, 4],
            }
        );
    }

    #[test]
    fn decodes_get_report() {
        let bytes = from_kernel(9, &[&42_u32.to_ne_bytes(), &[2], &[0]]);
        assert_eq!(
            decode(&bytes).unwrap(),
            FromKernel::GetReport {
                id: RequestId(42),
                number: ReportNumber(2),
                kind: ReportType::Feature,
            }
        );
    }

    #[test]
    fn decodes_set_report() {
        let bytes = from_kernel(
            13,
            &[
                &43_u32.to_ne_bytes(),
                &[2],
                &[0],
                &3_u16.to_ne_bytes(),
                &[1, 2, 3],
            ],
        );
        assert_eq!(
            decode(&bytes).unwrap(),
            FromKernel::SetReport {
                id: RequestId(43),
                number: ReportNumber(2),
                kind: ReportType::Feature,
                data: vec![1, 2, 3],
            }
        );
    }

    #[test]
    fn rejects_short_events() {
        assert_eq!(decode(&[1, 0]), Err(DecodeError::NoType(2)));
        let bytes = concat(&[&9_u32.to_ne_bytes(), &[0; 3]]);
        assert_eq!(
            decode(&bytes),
            Err(DecodeError::TooShort {
                event_type: 9,
                len: 7
            })
        );
    }

    #[test]
    fn rejects_oversize_set_report() {
        let size = u16::try_from(DATA_MAX + 1).unwrap();
        let bytes = from_kernel(13, &[&1_u32.to_ne_bytes(), &[0], &[0], &size.to_ne_bytes()]);
        assert_eq!(decode(&bytes), Err(DecodeError::DataTooLong(DATA_MAX + 1)));
    }
}

/// Round trips through the real kernel; needs root for `/dev/uhid`.
#[cfg(test)]
mod kernel_tests {
    use std::fs::File;
    use std::time::Duration;

    use rustix::event::PollFd;
    use rustix::event::PollFlags;
    use rustix::event::Timespec;

    use super::*;

    const UHID: &str = "/dev/uhid";

    /// How long the kernel may take to bind a driver.
    const EVENT_TIMEOUT: Duration = Duration::from_secs(5);

    /// One vendor-defined 8-bit input field: nothing binds to it but
    /// hid-generic.  Items per the HID 1.11 specification, section 6.2.2.
    const VENDOR_DESCRIPTOR: &[u8] = &[
        0x06, 0x00, 0xff, // Usage Page (Vendor Defined 0xFF00)
        0x09, 0x01, //       Usage (0x01)
        0xa1, 0x01, //       Collection (Application)
        0x15, 0x00, //         Logical Minimum (0)
        0x26, 0xff, 0x00, //   Logical Maximum (255)
        0x75, 0x08, //         Report Size (8)
        0x95, 0x01, //         Report Count (1)
        0x09, 0x01, //         Usage (0x01)
        0x81, 0x02, //         Input (Data, Variable, Absolute)
        0xc0, //             End Collection
    ];

    fn next_event(uhid: &File) -> FromKernel {
        let timeout = Timespec::try_from(EVENT_TIMEOUT).unwrap();
        let mut fds = [PollFd::new(uhid, PollFlags::IN)];
        let ready = rustix::event::poll(&mut fds, Some(&timeout)).unwrap();
        assert_eq!(ready, 1, "no uhid event within {EVENT_TIMEOUT:?}");
        read_event(uhid).unwrap()
    }

    #[test]
    #[ignore = "needs root for /dev/uhid"]
    fn create_start_destroy_stop() {
        let uhid = File::options().read(true).write(true).open(UHID).unwrap();
        let create = Create2 {
            name: "tempered uhid test".to_owned(),
            phys: "tempered-test".to_owned(),
            uniq: "tempered-test".to_owned(),
            bus: Bus::VIRTUAL,
            vendor: 0,
            product: 0,
            version: 0,
            country: 0,
            descriptor: VENDOR_DESCRIPTOR.to_vec(),
        };
        write_event(&uhid, ToKernel::Create2(&create)).unwrap();
        assert!(matches!(next_event(&uhid), FromKernel::Start { .. }));

        write_event(&uhid, ToKernel::Destroy).unwrap();
        assert_eq!(next_event(&uhid), FromKernel::Stop);
    }
}
