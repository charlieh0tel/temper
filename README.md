# tempered-hid

Presents a PCsensor TEMPerGold USB thermometer as a Linux IIO device.

`tempered daemon` reads the stick over hidraw and creates a virtual
HID sensor through `/dev/uhid`; the kernel's HID sensor drivers turn
it into an ordinary `iio:deviceN`.  Needs systemd 253+ and a kernel
with the HID sensor modules; see `docs/running.md`.

## Usage

```
sudo tempered read    # temperature in degrees C
sudo tempered info    # firmware, probes, calibration, manufacture date
sudo tempered log     # JSON Lines: --interval, --count, --time rfc3339|unix
sudo tempered daemon  # IIO device, linked as /run/tempered/temperature
```

## Layout

- `crates/tempered-hid`: library that talks to the stick; no daemon
  needed; published to crates.io.  MIT OR Apache-2.0.
- `crates/tempered-bin`: the `tempered` program.  GPL-3.0-or-later.
- `packaging/`: udev rules, systemd unit, tmpfiles.d, sleep hook,
  Debian maintainer scripts.  `make deb` builds the package;
  `RELEASING.md` covers releases.
- `PLAN.md`, `docs/`: decisions, protocol, daemon design, running.
