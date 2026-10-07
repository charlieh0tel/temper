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
sensor (usage 0x33).  hid-core groups it as a sensor hub
(`drivers/hid/hid-core.c`, any bus), `hid-sensor-hub` binds
(`HID_BUS_ANY`), and `hid-sensor-temperature` creates the IIO device.
All three modules autoload.  The virtual device reuses the stick's
VID:PID 3553:a001 on `BUS_VIRTUAL`; udev rules tell them apart by
matching the real stick's USB parent.  It also gets a hidraw node of
its own.

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
ID table is easy to extend.  The data interface is USB interface 1:
vendor page 0xFF00, 8-byte reports, no report IDs.  usbhid strips
only a leading 0x00, so commands are written as 9 bytes, a 0x00
report ID then the 8 command bytes.  The virtual device (below) has
the same VID:PID and its own hidraw node, so discovery accepts only
hidraw nodes on bus 0003 (USB) whose parent is interface 1.

The stick also exposes a boot keyboard on interface 0 that can type
readings, toggled by CapsLock/NumLock LED reports, which the console
keyboard handler mirrors to every keyboard.  Merely telling libinput to
ignore it is not enough, so a udev rule sets `authorized=0` on that
interface.  udev runs after the kernel has probed, so usbhid binds it
briefly and the write then unbinds it
(`usb_deauthorize_interface()`); to verify on hardware that the brief
bind cannot start typing.

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

### uhid codec

`struct uhid_event` is encoded and decoded by hand to bytes and moved
with plain `read`/`write`, following `include/uapi/linux/uhid.h`;
reads use a buffer of the full event size, since a short read
truncates and consumes the event.  Only `UHID_CREATE2` is used;
unlike legacy `UHID_CREATE` it does not check the caller's
credentials (`drivers/hid/uhid.c`), which is what makes the privilege
drop below possible.

### Report descriptor requirements

From `drivers/iio/common/hid-sensors/hid-sensor-attributes.c`,
`hid-sensor-trigger.c` and `hid-sensor-temperature.c`:

- Power State feature field: mandatory, probe fails without it.  It
  and Reporting State must list the D0/D4 and All/No Events enum
  usages, or the kernel never sets them.
- Report Interval feature field: mandatory in practice.  Without it
  the power-up path sleeps on an error code taken as milliseconds
  (about 49 days).  See the next section for the value.
- Temperature value, usage 0x200434 in a 0x200033 collection, with a
  negative Logical Minimum so it is sign-extended.
- Unit 0 with Unit Exponent -2: the scale table only matches Unit 0
  or 0x14, so a proper SI Celsius Unit item would fall back to scale
  1.  Exponent -2 gives `in_temp_scale` 10: raw is the stick's
  centi-degrees, scaled to milli-degrees C.
- Numbered reports (IDs >= 1) everywhere, and every GET_REPORT reply
  full length: in 7.0, `hid_report_raw_event()` rejects short
  reports where 6.10 padded them (checked against the 7.0 tag on
  GitHub; no 7.0 tree here).  GET_REPORT is answered for both
  input and feature reports.
- Change Sensitivity Absolute feature field, always 0: without it
  `in_temp_hysteresis` reads fail and `iio_info` shows an error.
- Friendly Name (0x200301) is read by no kernel code but
  hid-sensor-custom; it is not included.

### Report Interval is always 0

On runtime resume the sensor driver reads Report Interval and sleeps
twice that long (`_hid_sensor_power_state()` in
`hid-sensor-trigger.c`); it autosuspends after 3 s idle.  Reporting
the real 10 s poll interval would stall nearly every read by 20 s.  So
the descriptor's Report Interval always reads 0 and the sleep is
skipped (`simple_div()` guards the divide), and
`in_temp_sampling_frequency` reads 0.  The poll interval is daemon
configuration.  Writes to `in_temp_sampling_frequency` are accepted
and ignored: the kernel's SET is followed by a GET that returns 0.

### System suspend

The kernel's sensor suspend hook issues two SET and one GET feature
request after userspace is frozen, each timing out after 5 s, but only
if the sensor is runtime-active: a read in the last 3 s, or buffered
mode on (`hid-sensor-trigger.c`).  Rare, but cheap to avoid.

A `/usr/lib/systemd/system-sleep/` hook runs before
`/sys/power/state` is written.  In `pre` it records the active
`tempered@*` instances to a root-owned file under `/run` and stops
them; in `post` it starts exactly those.  A glob cannot be used for
the start: `systemctl start` globs match only loaded units, and udev's
`SYSTEMD_WANTS` does not fire again because the hidraw device stays
active across suspend.  systemd-sleep(8) calls such hooks hacks and
prefers a logind delay inhibitor, which would add a D-Bus dependency;
the hook is the simpler choice.

### Concurrency

Every kernel request blocks its caller until the daemon answers, for
up to 5 s, and the driver probe issues requests while the daemon is
already running.  So one thread serves uhid events from the cache at
all times; another polls the stick.  They share the cache.  Before
each command, stale hidraw input is drained (the firmware reply spans
two reports).  After a destroy, the old device's queued
`UHID_STOP`/`UHID_CLOSE` arrive before the new `UHID_START`; the
state machine allows for them.

### Stale data: hold, then destroy

`sensor_hub_input_attr_get_raw_value()` in
`drivers/hid/hid-sensor-hub.c` ignores the request's result and waits
up to 5 s; with no input report it returns 0.  A uhid error reply also
reads as 0.  Readers therefore cannot see an error, only a value, so:

- every request is answered at once from the cached reading;
- when the thermometer stops responding, the last good reading is
  held (zero-order hold) for a configurable age, default 60 s;
- after that the link is removed and then the virtual device is
  destroyed (`UHID_DESTROY`, sent by the serving thread after it has
  answered its queue).  The `iio:deviceN` directory goes away, so
  readers fail (`ENOENT`, or `ENODEV` on an open file), which
  smartclockd already logs as absent.  The next good reading
  recreates both; the IIO device number may change, the link does
  not.
- residual window: a read that starts during teardown, on a destroy or
  on any daemon stop, gets the kernel's 5 s timeout and 0
  (`uhid_dev_destroy()` stops answering before IIO unregisters).
  Removing the link first keeps link readers out of it; documented.
- rejected: a sentinel value (smartclockd would log it as data), and
  removing only the link (direct `iio:deviceN` readers would keep
  seeing the held value).
- a systemd watchdog (`WatchdogSec=`) covers the daemon hanging.  A
  destroy can block over 5 s while the kernel drains in-flight reads,
  so the watchdog is sized well above that.

### Stable name

`hid-sensor-temperature` names every device `temperature`, and
`iio:deviceN` numbering changes across replugs and boots.  An IIO
`label` attribute cannot be set from userspace: `iio_device_register()`
in `drivers/iio/industrialio-core.c` reads it only from the parent's
firmware node, which a uhid-created device lacks, and
`hid-sensor-temperature` has no `read_label`.

So the daemon links its IIO device as `/run/tempered/<label>`; readers
use `/run/tempered/<label>/in_temp_raw` and friends.  The label is set
by `TEMPERED_LABEL` in `/etc/default/tempered`, default `temperature`,
one value for all sticks: the stick has no serial number, and the
port path is too fragile to key on.  Only one stick is expected; a
second gets `-1` appended, silently; which stick gets the suffix
depends on start order.  Each instance holds an `flock` on
`/run/tempered/<label>.lock` to own a label (released on crash).  The
owner replaces its link atomically (symlink to a temporary name, then
rename).  The link targets the fully resolved sysfs path, which
contains the never-reused HID sequence number
(`.../uhid/0006:3553:A001.NNNN/...`), so a dangling link is
unambiguous.  The label is also the uhid device name (`HID_NAME`).

The daemon finds its IIO device by setting a unique `uniq` in
`UHID_CREATE2` and matching it under
`/sys/devices/virtual/misc/uhid/*/uevent`.

`/run/tempered` is created by tmpfiles.d, owned by the `tempered`
user, so it survives any one instance stopping; the unit's
`ProtectSystem=strict` needs `ReadWritePaths=/run/tempered`.

### Privileges

The template unit `tempered@.service` is started by udev per stick
(`TAG+="systemd"`, `SYSTEMD_WANTS`), and `BindsTo=` the hidraw device
so an unplug stops it.  It runs as the static system user `tempered`
(created in postinst, as smartclockmon does) with no capabilities.
Not `DynamicUser=`: the udev rule below needs a group that exists
before the service runs.

- `/dev/uhid` stays root-only.  systemd's `OpenFile=/dev/uhid`
  (systemd >= 253) opens it as root and passes the descriptor.
  Adopting that inherited descriptor needs `OwnedFd::from_raw_fd`;
  that one call is `unsafe`, allowed with
  `#[expect(unsafe_code, reason = ...)]` (approved).
- `OpenFile=` does not expand `%I` in systemd 255, so the hidraw node
  cannot be passed that way; udev gives the stick's data interface
  `GROUP="tempered", MODE="0660"` instead.
- No `PrivateDevices=`: its private `/dev` has no hidraw nodes.
  Instead `DevicePolicy=closed` with `DeviceAllow=char-hidraw rw`.
  `/dev/uhid` needs no `DeviceAllow=`, since PID 1 opens it before
  the policy applies (to verify).
- udev resolves `GROUP=` names when it parses rules, so postinst
  creates the user before `udevadm control --reload` and
  `udevadm trigger`.  udev's `SYSTEMD_WANTS` fires only when a device
  first becomes active, so postinst also starts `tempered@` for a
  stick already plugged in.
- `RestrictAddressFamilies=AF_UNIX` (for `sd_notify`).
- The daemon also runs without systemd, for tests and manual use: if
  `LISTEN_FDS` is unset or `LISTEN_PID` is not its own, it opens
  `/dev/uhid` itself (needs root).
- The uhid descriptor is a keystroke-injection capability: the same
  descriptor can destroy the sensor and create a keyboard, with no
  credential check.  Mitigated with `SystemCallFilter=`,
  `RestrictAddressFamilies=` and the rest of smartclockmon's
  hardening; documented.

### Polling

Default interval 10 s, minimum 1 s, set in `/etc/default/tempered`;
not settable through IIO (see Report Interval above).  Each poll also
pushes an input report, so IIO buffered mode and triggers get data.

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
4. Daemon: threads, stale policy, `/run/tempered` link, signals,
   watchdog.  Root-only tests (`make test-hw`): one with no stick that
   creates the device, waits for IIO, and checks raw, scale and that a
   read after idle returns at once; one with the stick.
5. udev rules (hotplug start, hidraw group, keyboard deauthorize),
   systemd unit, tmpfiles.d, system-sleep hook.
6. Debian packaging, release and audit workflows, Makefile,
   `RELEASING.md`.
7. Docs: protocol and descriptor rationale in `docs/`.
