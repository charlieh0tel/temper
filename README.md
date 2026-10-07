# tempered-hid

Presents a PCsensor TEMPerGold USB thermometer as a Linux IIO device.

`tempered daemon` reads the stick over hidraw and creates a virtual
HID sensor through `/dev/uhid`; the kernel's HID sensor drivers turn
it into an ordinary `iio:deviceN`.  Status: in development; the
daemon is not done yet.

## Usage

```
sudo tempered read    # temperature in degrees C
sudo tempered info    # firmware, probes, calibration, manufacture date
sudo tempered log     # JSON Lines: --interval, --count, --time rfc3339|unix
```

## Layout

- `crates/tempered-hid`: library that talks to the stick; no daemon
  needed.  MIT OR Apache-2.0.
- `crates/tempered-bin`: the `tempered` program.  GPL-3.0-or-later.
- `PLAN.md`, `docs/`: decisions, protocol, daemon design.
