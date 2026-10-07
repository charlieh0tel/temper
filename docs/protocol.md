# TEMPerGold protocol

What `tempered` sends to a PCsensor TEMPerGold and how it reads the
replies.  Implemented in the `tempered` library,
`crates/tempered/src/protocol.rs` and `crates/tempered/src/hidraw.rs`.

## Sources

- PCsensor ElfThing 1.0.2 for Windows, `resources/app.asar`, class
  `HIDTypeDevice` (download `ElfThing-1.0.2-win-x64.zip`, sha256
  `0557589d06840bbae85a5f11b46a71f14cbfe3082f5ee999230354c3974a78f5`).
- urwen/temper, `temper.py` (commit 40536cf).
- Captures from a `TEMPerGold_V3.5` stick, USB ID 3553:a001, in
  `crates/tempered/tests/fixtures/`.

## Transport

The stick has two HID interfaces.  Interface 0 is a boot keyboard
that can type readings; it is not used.  Interface 1 is the data
interface: vendor usage page 0xFF00, 8-byte input and output reports,
no report IDs.

Each command is 8 bytes, written to the hidraw node as 9: a 0x00
report ID, which usbhid strips, then the command.  Replies are read as
8-byte reports, waiting up to 500 ms each (ElfThing's read timeout).
Stale input is drained before every command.

## Commands

| Command | Bytes | Reply |
|---|---|---|
| Firmware | `01 86 ff 01 00 00 00 00` | Two reports of NUL-padded ASCII, e.g. `TEMPerGold_V3.5` |
| Temperature | `01 80 33 01 00 00 00 00` | `80 ..`, bytes 2-3 big-endian i16 in 0.01 degrees C |
| Sensor type | `01 87 ee 00 00 00 00 00` | `87 ..`, byte 1 inner probe, byte 2 outer; nonzero means present |
| Calibration | `01 82 77 01 00 00 00 00` | `82 ..`, bytes 2-5 signed tenths: inner temperature, inner humidity, outer temperature, outer humidity |
| Manufacture date | `01 8a 00 00 00 00 00 00` | `8a ..`, bytes 1-3 year - 2000, month, day |

ElfThing sends the firmware query with byte 3 `00`; urwen/temper uses
`01`, as does `tempered`.  Every reply except the firmware string
echoes the command's second byte first.

Captured on the bench stick:

```
firmware      54 45 4d 50 65 72 47 6f  6c 64 5f 56 33 2e 35 20   TEMPerGold_V3.5
temperature   80 80 0d b8 4e 20 00 00   35.12 degrees C
sensor type   87 80 00 00 00 00 00 00   inner 0x80, outer none
calibration   82 04 00 00 00 00 00 00   all 0.0
manufacture   8a 13 09 13 00 00 00 00   2019-09-19
```

The manufacture date is unverified on this model.  ElfThing sends
`01 8a` only to TEMPerHUM and to TEMPerX, TEMPer1F and TEMPer2 at
firmware 3.6 or later (`parseModel`), never to a TEMPerGold.  The
reply is tagged and decodes to a plausible date, but the year byte
equals the day byte and no marking on the stick confirms it.

In the temperature reply, bytes 4-5 (`4e 20`, 200.00) are the humidity
slot, unused on this model.  Byte 1 of the calibration reply has no
known meaning.

Readings outside the sensor's -40 to 125 degrees C range (ElfThing
`parseModel`) are rejected.

## Writes

ElfThing sends one write, set calibration (`01 81 55 01 a b c d 00`,
signed tenths, same order as the calibration reply).  `tempered`
never sends it.  No source documents a writable serial number, name or
ID; see `PLAN.md`.
