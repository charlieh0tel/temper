# temper

Presents a PCsensor TEMPerGold USB thermometer, or a TEMPerHUM
thermometer and hygrometer, as Linux IIO devices.  One stick at a
time.

`temper-iio` reads the stick over hidraw and creates a virtual
HID sensor through `/dev/uhid`; the kernel's HID sensor drivers turn
it into ordinary `iio:deviceN` entries.  Needs systemd 253+ and a kernel
with the HID sensor modules; see `docs/running.md`.

## Usage

```
sudo temper read      # degrees C, then %RH on a TEMPerHUM
sudo temper info      # firmware, model, probes, calibration, manufacture date
sudo temper log       # JSON Lines: --interval, --count, --time rfc3339|unix
sudo temper-iio       # IIO devices: /run/temper-iio/temperature, /run/temper-iio/humidity
```

`read`, `info` and `log` talk to the stick directly; stop the daemon
first (`systemctl stop 'temper-iio@*'`).

## Layout

- `crates/temper-hid`: library that talks to the stick; no daemon
  needed; published to crates.io.  MIT OR Apache-2.0.
- `crates/temper-hid-cli`: the `temper` command-line tool (`read`,
  `info`, `log`); published to crates.io.  GPL-3.0-or-later.
- `crates/temper-iio`: the `temper-iio` daemon.  GPL-3.0-or-later.
- `packaging/`: udev rules, systemd unit, tmpfiles.d, sleep hook,
  Debian maintainer scripts.  `make deb` builds both packages,
  `temper` (the CLI) and `temper-iio` (the daemon);
  `RELEASING.md` covers releases.
- `patches/`: upstream kernel fixes for hid-sensor-hub,
  hid-sensor-temperature and hid-sensor-humidity (shared callbacks).
- `PLAN.md`, `docs/`: decisions, protocol, daemon design, running.
