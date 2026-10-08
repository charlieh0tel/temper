# tempered-hid

Presents a PCsensor TEMPerGold USB thermometer, or a TEMPerHUM
thermometer and hygrometer, as Linux IIO devices.  One stick at a
time.

`tempered daemon` reads the stick over hidraw and creates a virtual
HID sensor through `/dev/uhid`; the kernel's HID sensor drivers turn
it into ordinary `iio:deviceN` entries.  Needs systemd 253+ and a kernel
with the HID sensor modules; see `docs/running.md`.

## Usage

```
sudo tempered read    # degrees C, then %RH on a TEMPerHUM
sudo tempered info    # firmware, model, probes, calibration, manufacture date
sudo tempered log     # JSON Lines: --interval, --count, --time rfc3339|unix
sudo tempered daemon  # IIO devices: /run/tempered/temperature, /run/tempered/humidity
```

`read`, `info` and `log` talk to the stick directly; stop the daemon
first (`systemctl stop 'tempered@*'`).

## Layout

- `crates/temper-hid`: library that talks to the stick; no daemon
  needed; published to crates.io.  MIT OR Apache-2.0.
- `crates/tempered-bin`: the `tempered` program.  GPL-3.0-or-later.
- `packaging/`: udev rules, systemd unit, tmpfiles.d, sleep hook,
  Debian maintainer scripts.  `make deb` builds the package;
  `RELEASING.md` covers releases.
- `patches/`: upstream kernel fixes for hid-sensor-hub,
  hid-sensor-temperature and hid-sensor-humidity (shared callbacks).
- `PLAN.md`, `docs/`: decisions, protocol, daemon design, running.
