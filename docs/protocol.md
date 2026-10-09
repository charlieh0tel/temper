# TEMPer protocol

What `temper-hid` sends to a PCsensor TEMPerGold, TEMPerHUM or TEMPer2
and how it reads the replies.  All three have USB ID 3553:a001 and the
same commands; the firmware string tells them apart.  Implemented in the `temper-hid` library,
`crates/temper-hid/src/protocol.rs` and `crates/temper-hid/src/hid/`
(shared logic in `mod.rs`, the Linux backend in `linux.rs`).

## Sources

- PCsensor ElfThing 1.0.2 for Windows, `resources/app.asar`, class
  `HIDTypeDevice` (download `ElfThing-1.0.2-win-x64.zip`, sha256
  `0557589d06840bbae85a5f11b46a71f14cbfe3082f5ee999230354c3974a78f5`).
- urwen/temper, `temper.py` (commit 40536cf).
- ccwienk/temper, `README.md` (commit 60889bf), for the TEMPerHUM's
  case markings.
- Captures from a `TEMPerGold_V3.5`, a `TEMPerHUM_V4.1` and a
  `TEMPer2_V4.1` stick, in `crates/temper-hid/tests/fixtures/`
  (`temper_gold/`, `temper_hum/`, and `temper2/` with the outer probe
  fitted, `temper2_no_outer/` without).

## Transport

The stick has two HID interfaces.  Interface 0 is a boot keyboard
that can type readings; it is not used.  Interface 1 is the data
interface: 8-byte input and output reports, no report IDs.  Its usage
page differs by model, so discovery goes by interface number, never
by usage page.  Report descriptors, read from sysfs
(`/sys/class/hidraw/hidrawN/device/report_descriptor`):

```
TEMPerGold_V3.5  06 00 ff 09 01 a1 01 09 01 15 00 26 ff 00 75 08 95 08 81 02
                 09 01 95 08 91 02 05 0c 09 00 15 80 25 7f 75 08 95 08 b1 02
                 c0
TEMPerHUM_V4.1   05 01 09 00 a1 01 09 01 15 00 25 ff 95 08 75 08 81 02 09 01
                 91 02 c0
TEMPer2_V4.1     the same as the TEMPerHUM's
```

The TEMPerGold's is vendor page 0xFF00, usage 1, with an extra 8-byte
feature report on the Consumer page (usage 0) that nothing here uses;
the TEMPerHUM's and TEMPer2's are Generic Desktop, usage 0, with no
feature report.

hidraw hands every input report to every process that has the node
open, so two processes querying one stick read each other's replies:
`temper read` beside the daemon once took the daemon's temperature
reply for half of its firmware string.  So on Linux, opening a stick
takes an exclusive, non-blocking `flock(2)` on the node, held while it
is open; a second open fails with `hid::Error::Busy`.  The lock is on
the node's inode, so a udev symlink to it counts too, and it is
advisory: only programs that take it, such as `temper` and
`temper-iio`, respect it.

Each command is 8 bytes, written to the hidraw node as 9: a 0x00
report ID, which usbhid strips, then the command.  Replies are read as
8-byte reports, waiting up to 500 ms each (ElfThing's read timeout).
Stale input is drained before every command.

On Windows the same reports go through hidapi's `windows-native`
backend (`crates/temper-hid/src/hid/windows.rs`): it takes the 0x00
report ID first on writes as hidraw does, and strips the 0x00 that
Windows puts before each report it reads.  Discovery uses hidapi's
device list (bus USB, VID:PID, interface 1), and the Win32 errors
`ERROR_DEVICE_NOT_CONNECTED`, `ERROR_OPERATION_ABORTED` and
`ERROR_BAD_COMMAND` are taken as removal.  None of this has been tried
with a stick.

The first command waits until 1 s after the node is opened, and every
command is preceded by a 20 ms pause.  ElfThing waits 2 s after
opening and 20 ms before each write; the shorter settle has worked on
the bench stick.

When the stick is unplugged, hidraw's `poll` reports `POLLHUP` and
`POLLERR` and `read` fails with `EIO`; `ENODEV` comes only from
writes (`drivers/hid/hidraw.c`).  The `hid::Device` transport reports
both, hang-up and `ENODEV`, as an I/O error of kind `NotConnected`,
the portable way for any transport to report a removed device, which
the protocol layer turns into `Error::Gone` (as it does `ENODEV`, on
Unix, from other transports).
Interrupted `poll` and `read` calls are retried.

## Commands

| Command | Bytes | Reply |
|---|---|---|
| Firmware | `01 86 ff 01 00 00 00 00` | Two reports of NUL-padded ASCII, e.g. `TEMPerGold_V3.5` |
| Temperature | `01 80 33 01 00 00 00 00` | `80 ..`, bytes 2-3 big-endian i16 in 0.01 degrees C; on a TEMPerHUM, bytes 4-5 big-endian i16 in 0.01 %RH; on a TEMPer2 with the outer probe fitted, a second report, the outer probe's (below) |
| Sensor type | `01 87 ee 00 00 00 00 00` | `87 ..`, byte 1 inner probe, byte 2 outer; nonzero means present |
| Calibration | `01 82 77 01 00 00 00 00` | `82 ..`, bytes 2-5 signed tenths: inner temperature, inner humidity, outer temperature, outer humidity |
| Manufacture date | `01 8a 00 00 00 00 00 00` | `8a ..`, bytes 1-3 year - 2000, month, day |

ElfThing sends the firmware query with byte 3 `00`; urwen/temper uses
`01`, as does `temper-hid`.  Every reply except the firmware string
echoes the command's second byte first.

Captured on the bench sticks:

```
TEMPerGold
firmware      54 45 4d 50 65 72 47 6f  6c 64 5f 56 33 2e 35 20   TEMPerGold_V3.5
temperature   80 80 0d b8 4e 20 00 00   35.12 degrees C
sensor type   87 80 00 00 00 00 00 00   inner 0x80, outer none
calibration   82 04 00 00 00 00 00 00   all 0.0
manufacture   8a 13 09 13 00 00 00 00   2019-09-19

TEMPerHUM
firmware      54 45 4d 50 65 72 48 55  4d 5f 56 34 2e 31 00 00   TEMPerHUM_V4.1
temperature   80 20 0d 3c 0c 1d 00 00   33.88 degrees C, 31.01 %RH
sensor type   87 20 00 00 00 00 00 00   inner 0x20, outer none
calibration   82 04 00 00 00 00 00 00   all 0.0
manufacture   8a 17 03 01 00 00 00 00   2023-03-01

TEMPer2, plugged in with the outer probe
firmware      54 45 4d 50 65 72 32 5f  56 34 2e 31 00 00 00 00   TEMPer2_V4.1
temperature   80 80 0a a5 4e 20 00 00   27.25 degrees C, inner
              80 01 08 ef 4e 20 00 00   22.87 degrees C, outer
sensor type   87 80 01 00 00 00 00 00   inner 0x80, outer 0x01
calibration   82 04 00 00 00 00 00 00   all 0.0
manufacture   8a 17 03 01 00 00 00 00   2023-03-01

TEMPer2, plugged in without it
temperature   80 80 0a 92 4e 20 00 00   27.06 degrees C, inner
sensor type   87 80 00 00 00 00 00 00   inner 0x80, outer none
```

The humidity layout follows ElfThing's type 5 `TEMPerHUM` branch
(`parseByteToData` of the two bytes after the temperature) and
urwen/temper's `TEMPerHUM_V3.9` (offset 4, divisor 100), and matches
the capture.  On the TEMPerGold, bytes 4-5 (`4e 20`, 200.00) are an
unused humidity slot.  Byte 1 of the temperature reply repeats the
sensor type's inner probe code.  Byte 1 of the calibration reply has
no known meaning.

The manufacture date is unverified.  ElfThing sends `01 8a` to
TEMPerHUM (`readDeviceBirthday`) and to TEMPerX, TEMPer1F and TEMPer2
at firmware 3.6 or later (`parseModel`), never to a TEMPerGold.  Both
replies are tagged and decode to plausible dates, but no marking on
either stick confirms them.

The model is the firmware string's prefix: `TEMPerGold_` or
`TEMPerHUM_`, as ElfThing's `parseModel` tests; that excludes
`TEMPerHumM12`, which ElfThing decodes differently.  A TEMPer2 is
`TEMPer2_V` and a version of 3.6 or later: ElfThing decodes only those
as type 6 (`version >= 3.6`), and decodes `TEMPer2_M12` (e.g.
`TEMPer2_M12_V1.3`, one report, the outer probe in bytes 4-5 per
urwen/temper) as type 3, which `temper-hid` does not.  Other firmware
is refused (`temper-iio` exits 3).  Readings outside the sensor's range are
rejected: -40 to 125 degrees C on a TEMPerGold and on either TEMPer2
probe, -40 to 85 on a TEMPerHUM (ElfThing `parseModel`,
`innerTemperatureCRangeMin` and `Max`, and `outer...` for the TEMPer2's
outer probe; type 6 keeps the defaults), and 0 to 100 %RH (the TEMPerHUM's case marking, quoted in
ccwienk/temper `README.md`).  A query fails if more than 16 stale
reports precede it.

The library reports values in natural units, `Celsius` and
`RelativeHumidityPercent` (f64); calibration offsets too.

## TEMPer2 outer probe

The TEMPer2 has an inner probe and a detachable outer one on a lead.
When identifying a TEMPer2, `temper-hid` also queries the sensor type,
as ElfThing's type 6 does, to learn whether the outer probe is fitted
(byte 2 nonzero).  With it, the temperature reply is two reports, the
inner probe's and then the outer probe's, each tagged `80` and laid
out alike (ElfThing `readData`, type 6, reads the second when
`outerSensor > 0`; urwen/temper `TEMPer2_V3.7` and `V3.9` read the
outer temperature at offset 10, divisor 100).  Without it, one
report.  Byte 1 of each temperature report repeats its probe's code
from the sensor type reply (inner 0x80, outer 0x01), so the library
checks it (`Error::WrongProbe`): the tag alone cannot tell a stale
outer report from an inner one.

The stick sees the outer probe only at power-up.  Captured on the
bench stick: plugged in later, it is not seen (sensor type `87 80 00`,
one report) until the stick is replugged; pulled out at runtime, the
stick keeps answering the sensor type `87 80 01` and two reports, the
outer one reading `4e 20` (200.00, which urwen/temper's
`_parse_bytes` skips as no reading), and plugged back in it stays at
`4e 20` until the stick is replugged.  Even at power-up the stick
does not always see it: on the bench, it missed a fully seated probe
on three of four replugs, for no known reason.  So `temper-hid` reads the
number of reports learned when it identified the stick, and reports
an outer `4e 20` as `Error::OuterProbeRemoved`.

## Writes

ElfThing sends one write, set calibration (`01 81 55 01 a b c d 00`,
signed tenths, same order as the calibration reply).  `temper-hid`
never sends it.  No source documents a writable serial number, name or
ID; see `PLAN.md`.
