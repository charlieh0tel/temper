# tempered plan

## Goal

Read a PCsensor TEMPerGold USB thermometer and present it as a real
Linux IIO device, so stock IIO consumers (libiio, `iio_info`,
smartclockd) read it without knowing about the thermometer.  Shipped
as a Debian package with a systemd unit and udev rules.

## Decisions

### IIO via /dev/uhid

The daemon creates a virtual HID device through `/dev/uhid` whose
report descriptor declares a HID sensor (usage page 0x20) temperature
sensor (usage 0x33).  hid-core groups it as a sensor hub,
`hid-sensor-hub` binds (it matches any bus), and
`hid-sensor-temperature` creates the IIO device.

Rejected:

- iiod network server: network-only, not a local IIO device.
- `iio_dummy` / configfs: values come from the driver; userspace
  cannot feed them.
- Fake sysfs tree (FUSE/CUSE): libiio's local backend reads
  `/sys/bus/iio` and cannot be pointed elsewhere.
- Kernel module: the project is userspace.

### Thermometer access

hidraw through `std::fs`; no hidapi, no libusb, no C dependencies.
Only USB ID 3553:a001 is supported for now (the hardware on hand); the
ID table is easy to extend.  The stick also exposes a boot keyboard
interface that can type readings; a udev rule makes the system ignore
it.

### No writable ID on the stick

The stick reports no USB serial number, and no source documents a
command that writes one, a descriptor string, or any name or ID slot.
PCsensor's own ElfThing 1.0.2 app (`resources/app.asar`, class
`HIDTypeDevice`; download `ElfThing-1.0.2-win-x64.zip`, sha256
`0557589d06840bbae85a5f11b46a71f14cbfe3082f5ee999230354c3974a78f5`)
sends one write, set calibration (`01 81 55 01 ...`), and otherwise
only reads.  Storing an ID in the stick's unused calibration slots
was considered and rejected: untested, probably wears flash, and one
slot offsets the reading.  The MCU is unknown and no reflash path is
published.  Hence the label is configuration, not device state.

`tempered info` reports the read-only extras the vendor app uses:
sensor type (`01 87 ee`) and manufacture date (`01 8a`), the latter
untested on TEMPerGold.

### uhid without unsafe

`struct uhid_event` is encoded and decoded by hand to bytes and moved
with plain `read`/`write`, following `include/uapi/linux/uhid.h`.
Only `UHID_CREATE2` is used; unlike legacy `UHID_CREATE` it does not
check the caller's credentials, which is what makes the privilege drop
below possible.

### Stale data: hold or sentinel

`sensor_hub_input_attr_get_raw_value()` in
`drivers/hid/hid-sensor-hub.c` ignores the request's result and waits
up to 5 s; with no input report it returns 0.  A uhid error reply also
reads as 0.  Readers therefore cannot see an error, only a value, so:

- every request is answered at once from the cached reading;
- when the thermometer stops responding, a configurable policy
  chooses what is served: hold the last good reading (zero-order hold)
  for up to a configurable age, then the sentinel.  Default: hold
  60 s, then sentinel.
- the sentinel is fixed at `i16::MIN` (-327.68 degrees C), outside
  any real reading.
- a systemd watchdog (`WatchdogSec=`) covers the daemon hanging.

### Stable name

`hid-sensor-temperature` names every device `temperature`, and
`iio:deviceN` numbering changes across replugs and boots.  An IIO
`label` attribute cannot be set from userspace: `iio_device_register()`
in `drivers/iio/industrialio-core.c` reads it only from the parent's
firmware node, which a uhid-created device lacks, and
`hid-sensor-temperature` has no `read_label`.

So the daemon, which knows which IIO device it created, links it as
`/run/tempered/<label>`, where the label is configurable and defaults
to `temperature`; `-1`, `-2`, ... are appended, in order of arrival,
only when two sticks claim the same label.  The label is also the
uhid device name (`HID_NAME`) and the descriptor's Friendly Name
property.  It is set by `TEMPERED_LABEL` in `/etc/default/tempered`,
one value for all sticks: the stick has no serial number, and the
port path is too fragile to key on.  Only one stick is expected.
Sharing of `/run/tempered` between unit instances is
to be settled in phase 5.

### Privileges

The template unit `tempered@.service` is started by udev per stick.
systemd's `OpenFile=` (systemd >= 253) opens `/dev/uhid` and the
stick's hidraw node as root and passes the descriptors; the daemon
runs as `DynamicUser=yes` with no capabilities.  `/dev/uhid` stays
root-only.

### Polling

Default interval 10 s, minimum 1 s; IIO `sampling_frequency` writes
are clamped to the minimum.

### Project shape

One package, `src/lib.rs` (thiserror) plus a binary (anyhow).
Toolchain pinned to match smartclockmon.  CI, release and audit use
the shared `charlieh0tel/deb-workflows`.  Released as a .deb through
the apt repo only, not crates.io.  GPL-3.0-or-later.

## Phases

0. Scaffold: AGENTS.md, Cargo.toml, toolchain, lints, CI, this plan.
   **Done.**
1. TEMPerGold protocol module and hidraw discovery; `tempered read`
   to check the hardware.  Fixtures captured from the stick.
2. uhid event codec, golden-byte tests.
3. HID sensor report descriptor and sensor state machine, tests.
4. Daemon: poll loop, stale policy, `/run/tempered` link, signals,
   watchdog; root-only hardware tests (`make test-hw`).
5. udev rules (hotplug start, keyboard ignore), systemd unit.
6. Debian packaging, release and audit workflows, Makefile,
   `RELEASING.md`.
7. Docs: protocol and descriptor rationale in `docs/`.
