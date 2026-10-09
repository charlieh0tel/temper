# temper plan

## Goal

Read a PCsensor TEMPerGold USB thermometer, or a TEMPerHUM
thermometer and hygrometer, and present it as real Linux IIO devices, so stock IIO consumers (libiio, `iio_info`,
smartclock-sensord) read it without knowing about the thermometer.  Shipped
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
8-byte reports, no report IDs; its usage page differs by model
(`docs/protocol.md`).  usbhid strips
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
(`usb_deauthorize_interface()`).  Not verified: that the brief bind
cannot start typing.

### No writable ID on the stick

The stick reports no USB serial number, and no source documents a
command that writes one, a descriptor string, or any name or ID slot.
PCsensor's own ElfThing 1.0.2 app (see `docs/protocol.md`, "Sources")
sends one write, set calibration (`01 81 55 01 ...`), and otherwise
only reads.  Storing an ID in the stick's unused calibration slots
was considered and rejected: untested, probably wears flash, and one
slot offsets the reading.  The MCU is unknown and no reflash path is
published.  Hence the label is configuration, not device state.

`temper info` reports the read-only extras the vendor app uses:
sensor type (`01 87 ee`) and manufacture date (`01 8a`).  The stick
answers the latter, but the date is unverified; see `docs/protocol.md`.

### uhid codec

`struct uhid_event` is encoded and decoded by hand to bytes and moved
with plain `read`/`write`, following `include/uapi/linux/uhid.h`
(packed, native-endian, 4376 bytes in full).  Writes stop after the
last meaningful byte, since the kernel zero-fills the rest;
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
  reports where 6.10 padded them.  GET_REPORT is answered for both
  input and feature reports.
- Change Sensitivity Absolute feature field, always 0: without it
  `in_temp_hysteresis` reads fail and `iio_info` shows an error.
- Friendly Name (0x200301) is read by no kernel code but
  hid-sensor-custom; it is not included.

As built (`crates/temper-iio/src/sensor.rs`): one application
collection per quantity, temperature (usage 0x200033, report ID 1)
and, on a TEMPerHUM, humidity (usage 0x200032 with data field
0x200433, report ID 2; `hid-sensor-humidity` has the same Unit 0 scale
row and 32-bit buffered samples).  Each has its report ID for both a
feature report
(Reporting State and Power State as 1-based named arrays in logical
collections, Report Interval u32 with Logical Maximum 2^31 - 1, since
the item is signed; Change Sensitivity Absolute u16; 9 bytes with the
ID) and an input report (the value as 32 bits, though its values
fit 16, because the drivers' buffered path reads every sample as 32
bits; 5 bytes).
Reporting State lists only the No Events and All Events selectors the
kernel defines.  No Sensor State or Event fields: the Linux drivers do
not read them.  Writes to Reporting State and Power State are stored
and read back; writes to Report Interval and sensitivity are dropped.

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
`temper-iio@*` instances to a root-owned file under `/run` and stops
them; in `post` it starts, without blocking, those whose hidraw node
still exists (a stick unplugged during sleep would make a blocking
start wait 90 s for its device).  A glob cannot be used for
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
`UHID_STOP`/`UHID_CLOSE` arrive before the new `UHID_START`; they are
only logged.  Threads, locking and error handling: `docs/daemon.md`.

### Stale data: hold, then destroy

`sensor_hub_input_attr_get_raw_value()` in
`drivers/hid/hid-sensor-hub.c` ignores the request's result and waits
up to 5 s; with no input report it returns 0.  A uhid error reply also
reads as 0.  Readers therefore cannot see an error, only a value, so:

- every request is answered at once from the cached reading;
- when the thermometer stops responding, the last good reading is
  held (zero-order hold) for a configurable age, default 60 s;
- after that the link is removed and then the virtual device is
  destroyed (`UHID_DESTROY`; the kernel fails pending requests itself,
  so no thread needs to be answering).  The `iio:deviceN` directory goes away, so
  readers fail (`ENOENT`, or `ENODEV` on an open file), which
  smartclock-sensord already logs as absent.  The next good reading
  recreates both; the IIO device number may change, the link does
  not.
- residual window: a read that starts during teardown, on a destroy or
  on any daemon stop, gets the kernel's 5 s timeout and 0
  (`uhid_dev_destroy()` stops answering before IIO unregisters).
  Removing the link first keeps link readers out of it; documented.
- rejected: a sentinel value (smartclock-sensord would log it as data), and
  removing only the link (direct `iio:deviceN` readers would keep
  seeing the held value).
- a systemd watchdog (`WatchdogSec=60s`) covers the daemon hanging.
  A destroy can block 5 s per concurrent reader, so it runs on its own
  thread while the main thread keeps pinging; see `docs/daemon.md`.
- the stick unplugged: the daemon cleans up and exits 0.  hidraw
  reports it as hang-up on `poll` and `EIO` on `read`, not `ENODEV`.

### Stable name

`hid-sensor-temperature` names every device `temperature`, and
`iio:deviceN` numbering changes across replugs and boots.  An IIO
`label` attribute cannot be set from userspace: `iio_device_register()`
in `drivers/iio/industrialio-core.c` reads it only from the parent's
firmware node, which a uhid-created device lacks, and
`hid-sensor-temperature` has no `read_label`.

So the daemon links its IIO device as `/run/temper-iio/<label>`; readers
use `/run/temper-iio/<label>/in_temp_raw` and friends.  The label is set
by `TEMPER_IIO_LABEL` in `/etc/default/temper-iio`, default `temperature`
(and a TEMPerHUM's humidity device by `TEMPER_IIO_HUMIDITY_LABEL`,
default `humidity`),
one value for all sticks: the stick has no serial number, and the
port path is too fragile to key on.  Only one stick is expected, so
there are no suffixes: each instance holds an `flock` on
`/run/temper-iio/<label>.lock` (released on crash), and a second
instance, such as one started by a replug while the old one is still
stopping, waits for it.  The label must match
`[a-z0-9][a-z0-9_-]{0,63}`.  The owner replaces its link atomically (symlink to a temporary name, then
rename).  The link targets the fully resolved sysfs path, which
contains the never-reused HID sequence number
(`.../uhid/0006:3553:A001.NNNN/...`), so a dangling link is
unambiguous.  The label is also the uhid device name (`HID_NAME`).

The daemon finds its IIO device by setting a unique `uniq` in
`UHID_CREATE2` and scanning `/sys/bus/iio/devices` for an entry with
an ancestor whose `uevent` has that `HID_UNIQ`.  Only `iio:device*`
entries count: the sensor driver also registers a trigger,
`temperature-devN`, listed as `triggerN` under the same parent.

`/run/temper-iio` is created by tmpfiles.d, mode 0755 so readers can
follow the link, owned by the `temper-iio` user, so it survives any one
instance stopping; the unit's `ProtectSystem=strict` needs
`ReadWritePaths=/run/temper-iio`, which fails the unit if the directory
is missing, so postinst runs `systemd-tmpfiles --create` first.

### Privileges

The template unit `temper-iio@.service` is started by udev per stick
(`TAG+="systemd"`, `SYSTEMD_WANTS`), and `BindsTo=` the hidraw device
so an unplug stops it.  It runs as the static system user `temper-iio`
(created in postinst, as smartclockmon does) with no capabilities.
Not `DynamicUser=`: the udev rule below needs a group that exists
before the service runs.

- `/dev/uhid` stays root-only.  systemd's `OpenFile=/dev/uhid:uhid`
  (systemd >= 253) opens it as root and passes the descriptor, named
  `uhid` in `LISTEN_FDNAMES`.  Adopting that inherited descriptor
  needs `OwnedFd::from_raw_fd`; that one call is `unsafe`, allowed
  with `#[expect(unsafe_code, reason = ...)]` (approved).  The
  `LISTEN_*` variables are not unset: `std::env::remove_var` is also
  `unsafe` in edition 2024, and the daemon never execs.
- `OpenFile=` does not expand `%I` in systemd 255, so the hidraw node
  cannot be passed that way; udev gives the stick's data interface
  `GROUP="temper-iio", MODE="0660"` instead.
- No `PrivateDevices=`: its private `/dev` has no hidraw nodes.
  Instead `DevicePolicy=closed` with `DeviceAllow=char-hidraw rw` and
  `DeviceAllow=/dev/uhid rw`.  `OpenFile=` works this way on the real
  unit; whether the second line is needed (systemd 255 opens
  `OpenFile=` from `systemd-executor`, possibly under the device
  policy) is untested, so it stays as a precaution.  It grants nothing
  extra, since `/dev/uhid` is root-only.
- udev resolves `GROUP=` names when it parses rules, so postinst
  creates the user before `udevadm control --reload` and
  `udevadm trigger`.  udev's `SYSTEMD_WANTS` fires only when a device
  first becomes active, so postinst also starts `temper-iio@` for a
  stick already plugged in.
- `RestrictAddressFamilies=AF_UNIX` (for `sd_notify`).
- The daemon also runs without systemd, for tests and manual use: if
  `LISTEN_PID` is unset, it opens `/dev/uhid` itself (needs root).
- The uhid descriptor is a keystroke-injection capability: the same
  descriptor can destroy the sensor and create a keyboard, with no
  credential check.  Mitigated with `SystemCallFilter=`,
  `RestrictAddressFamilies=` and the rest of smartclockmon's
  hardening; documented.

### One HID temperature sensor per machine

The kernel's `hid-sensor-temperature` mishandles two instances and
can oops when one is removed (static callbacks; see `docs/daemon.md`,
"Only one HID temperature sensor"); `hid-sensor-humidity` too.  So
one stick at a time: several sticks are not supported until the
kernel fix lands, and there is no override flag (considered and
dropped, 2026-10-07).  The daemon refuses to create its sensors while
another of either kind exists (exit 3), the label lock keeps a replug
from overlapping, and the root tests serialize (and refuse to run
while any HID temperature sensor exists, such as a running
`temper-iio@`: running them beside the service oopsed the kernel and
killed the daemon, 2026-10-07).  Upstream fixes are in `patches/`, the
v2 series as sent to linux-iio and linux-input on 2026-10-07: the
hub's callback removal synchronized with raw events (found by the
Sashiko review of v1), then per-instance callbacks in temperature and
humidity.  On 7.3-rc6, checkpatch-clean and compile-tested; a runtime
test with the patched modules (`two_sensors_survive_a_destroy` in
`sensor.rs`, opt-in with `TEMPER_IIO_TWO_SENSORS=1`) waits on a MOK
enrollment for Secure Boot.  7.3 already fixes the temperature
driver's remove order (967d066f5334).

### Access to the stick

The `temper` package owns the stick's udev rule, `60-temper.rules`:
the data interface goes to group `temper` (mode 0660) and to the user
at the seat (`TAG+="uaccess"`, which systemd-logind turns into an ACL),
and the keyboard interface is deauthorized.  Programs that read the
stick as services join the group: `temper-iio` with
`SupplementaryGroups=temper` (it depends on `temper`), and
smartclock-sensord, which is to read sticks directly through
`temper-hid` rather than through IIO (decided 2026-10-08), the same
way.  `temper-iio` keeps only the rule that starts its unit, and is
unchanged otherwise.  Before 1.3.0 the rule was `temper-iio`'s, with
group `temper-iio`, so the CLI alone left the node root-only.

### One process per stick

hidraw hands every input report to every reader, so `temper` run
beside `temper-iio` read the daemon's reply as part of its own
(2026-10-08).  Opening a stick on Linux therefore takes an exclusive,
non-blocking `flock(2)` on the node itself, held while it is open, and
a second open fails with `hid::Error::Busy`.  The node, not a lock
file: no directory, owner or cleanup, nothing beyond the access the
node already needs, and a crash releases it.  Advisory, so only our
programs respect it; Windows has no daemon and no lock.  Skipping
other processes' replies in the protocol was considered and dropped:
both sides would still lose queries to each other.  Released in 1.2.0
(additive: a new variant on a `#[non_exhaustive]` error).

### Polling

Default interval 10 s, minimum 1 s, set in `/etc/default/temper-iio`;
not settable through IIO (see Report Interval above).  Each poll also
pushes an input report, so IIO buffered mode and triggers get data.

### Project shape

A workspace of two crates, as smartclockmon does:

- `crates/temper-hid` (was `tempered-hid`), a library that only
  talks to the stick (protocol, discovery, the `hid` transport), usable
  by third parties without the daemon.  thiserror, no anyhow.  Newtypes for units and IDs
  (`Celsius`, `RelativeHumidityPercent`, `Firmware`, `Probe`),
  `#[non_exhaustive]` public enums and result structs, documented
  (`missing_docs`).  MIT OR Apache-2.0.  Published to crates.io,
  sharing the workspace version and publishing on every release tag,
  for consistency with usbrelay-rs and ut325f-rs (a separate library
  version was considered: fewer republications, but a second number
  to manage).  Only from CI: a release job with crates.io Trusted
  Publishing (OIDC, `id-token: write`, `rust-lang/crates-io-auth-action`
  pinned by commit SHA, no stored token) runs `cargo publish -p
  temper-hid`, gated on the shared workflow's audit job.
- `crates/temper-hid-cli`, the `temper` tool, published: `temper
  read`, `temper info`, and `temper log`, JSON Lines at a fixed
  interval, with `time` as RFC 3339 or Unix seconds by flag and failed
  reads as error lines.  Its `build.rs` stamps both binaries.  anyhow
  and clap.  GPL-3.0-or-later.
- `crates/temper-iio`, the daemon, `temper-iio`, unpublished.  It runs
  in the foreground under systemd (`Type=notify`, watchdog, restarts;
  it never forks): uhid, IIO, `/run` links, configuration.  `temper
  read`, `info` and `log` poll the stick too, so they are used with
  the daemon stopped.  anyhow and clap.  GPL-3.0-or-later.
- The two binaries were one program, `tempered` (with a `daemon`
  subcommand), until the split for temper 1.0.0 (below).
Toolchain pinned to match smartclockmon.  CI, release and audit use
the shared `charlieh0tel/deb-workflows`.  Released as a .deb through
the apt repo; the library also goes to crates.io (above).  Builds are stamped from `git describe`
(`RELEASING.md`), with no version bump after a release.

## 2.0.0

An adversarial review of 1.0.0 (Linux, USB/IIO, Rust and packaging
reviewers, 2026-10-07) found no bug in the normal one-stick path, but
security, robustness, API and packaging issues.  The fixes break the
library's API, so the next release is 2.0.0, and 1.0.0 is yanked
from crates.io once it ships.  Decisions:

- Supported: systemd 253+ (for `OpenFile=`), kernel 4.14+, with the
  HID sensor modules; Ubuntu 24.04+ and Debian trixie, not bookworm.
  `Depends: systemd (>= 253)`.
- Unprivileged IIO readers can stall a destroy (5 s each): accepted
  and documented, not group-restricted, so readers need no setup.
- TEMPer2_V4.1, which shares 3553:a001, is refused cleanly (exit 3).
- The descriptor's temperature field becomes 32 bits (buffered-mode
  values for negative temperatures), and its top collection
  Application.
- 1 s settle after opening the stick, 20 ms before each write
  (ElfThing waits 2 s and 20 ms).
- Purge locks the `tempered` account instead of deleting it.
- `deb-workflows` stays at `@v1`, as in the sibling repos.

Library API (breaking): `hidraw::Error` (`Scan`, `Open` with the
path, `NotFound`, `Several`) replaces `FindError` and the bare
`io::Error` from `Stick::open`; every error variant that carries data
is `#[non_exhaustive]`.  Added: `VENDOR_ID`/`PRODUCT_ID`, `AsFd` for
`Hidraw`, constructors for the value newtypes (`Firmware`, being
validated, has none), `CentiCelsius::celsius`,
`Stick::into_inner`/`transport_mut`, `Display` for `Command`, `Hash` on
the result structs; `discover` skips entries that vanish mid-scan;
`receive` waits without limit on an unrepresentable timeout instead of
panicking; the settle and pause timing above.

Daemon robustness: lock files 0600 (anyone could otherwise hold the
label's lock); unsupported firmware exits 3; the firmware query is
retried at startup; a sensor left by a crashed instance gets 10 s to
go; a create cut short by shutdown is not counted and keeps the
shutdown's status; SIGHUP is handled; hold ages use `CLOCK_BOOTTIME`;
`WATCHDOG_USEC=0` disables the watchdog and the tick is at least
10 ms.

Descriptor: Application collection; Report Interval's maximum encoded
as a positive value; 32-bit temperature field.

Packaging and release: `Depends: systemd (>= 253)`; `postinst` starts
the daemon for sticks already plugged in (udev does not on a
reinstall) and uses `--no-block`; the suspend hook starts without
blocking and only for sticks still present; purge locks the account;
the unit adds `UMask=0077`, `ProtectProc=invisible`, `ProcSubset=pid`,
`RemoveIPC=yes`; `make deb` reruns `build.rs` so the version stamp
sees uncommitted edits; the release workflow checks that the tag,
`Cargo.toml` and the changelog agree before building, and defaults to
no token permissions.

Done: refactor (83e6ab2), library API (a807960), daemon robustness
(0cfb9be), descriptor (78ecf5e), packaging and release (a3af4ca),
docs, the release (3d16e46), and yanking 1.0.0.

## 3.0.0: TEMPerHUM

A TEMPerHUM (`TEMPerHUM_V4.1`, same USB ID 3553:a001) joined the
bench on 2026-10-07; 2.0.0 refused it cleanly (exit 3).  Decisions:

- Support either model, one stick at a time (above).  The firmware
  prefix tells them apart (`docs/protocol.md`).
- The daemon presents a TEMPerHUM as two IIO devices from one uhid
  device, a collection per quantity, linked as `/run/tempered/<label>`
  and `/run/tempered/<humidity-label>`; `--humidity-label`
  (`TEMPERED_HUMIDITY_LABEL`), default `humidity`.  The temperature
  link keeps its name, so readers need no change.
- `tempered read` prints `<C> <%RH>` on one line for a TEMPerHUM, and
  `info` and `log` add humidity.  `log` drops `centi_celsius`.
- Library API (breaking, hence 3.0.0; 2.0.0 to be yanked once it
  ships): values in natural units, `Celsius` and
  `RelativeHumidityPercent` (f64 newtypes; percent rather than a
  fraction), for readings and calibration offsets, replacing
  `CentiCelsius`, `DeciCelsius` and `DeciPercent`, which exposed the
  stick's encoding.  `Stick::temperature()` gives way to
  `Stick::reading()`, which returns a `Reading` (temperature, and
  humidity for a model that has it) and learns the model from a
  firmware query the first time.  Added: `Model`,
  `Firmware::model()`, `Stick::model()`,
  `Error::HumidityOutOfRange`.  The daemon still sends hundredths to
  the kernel, rounding, which is exact for the stick's values, so the
  temperature descriptor and `in_temp_raw` are unchanged.

Done: refactor (bc21e45), TEMPerHUM support and docs (297e751),
the release (01faf31), and yanking 2.0.0.

## temper 1.0.0: rename, split, Windows (planned)

Goal: the library on Linux and Windows behind one API, a clean
portable CLI, and the Linux daemon in its own crate, everything built
and tested from Linux.  Decided 2026-10-07 in interviews, after two
adversarial reviews (one of an earlier hidapi-everywhere draft, one of
this plan).

### Names and repository

`tempered` is taken on crates.io by an unrelated crate, and the
daemon pun no longer fits a project with a separate CLI.  So:

| Crate | Published | Binary | Platforms |
|---|---|---|---|
| `temper-hid` (was `tempered-hid`) | crates.io | none | Linux, Windows |
| `temper-hid-cli` (new, from `tempered-bin`'s `read`, `info`, `log`) | crates.io | `temper` | Linux, Windows |
| `temper-iio` (was `tempered-bin`'s daemon) | no | `temper-iio` | Linux |

- `temper-cli` is taken on crates.io (a placeholder of an unrelated
  "temper"; that project holds several `temper-*` names), so the CLI
  crate is `temper-hid-cli`; its binary is still `temper`.  No
  placeholders are published early: crates.io discourages squatting,
  and the real 1.0.0 follows soon.
- The daemon binary runs only the daemon: options at the top level
  (`temper-iio --device /dev/hidrawN [--label ...]`), no subcommand,
  no `read`/`info`/`log` (those are `temper`'s).
- Two debs: `temper` (the CLI; `Suggests: temper-iio`, which carries
  the udev rule that grants hidraw access) and `temper-iio` (daemon,
  unit, udev rules, tmpfiles.d, sleep hook, defaults).  No
  `Conflicts: tempered`: only this machine ever had it, and it was
  removed by hand.
- System names, all `temper-iio`: unit `temper-iio@`, user
  `temper-iio`, `/run/temper-iio`, `/run/temper-iio-sleep`,
  `/etc/default/temper-iio`, `60-temper-iio.rules`, environment
  `TEMPER_IIO_LABEL`, `TEMPER_IIO_HUMIDITY_LABEL`,
  `TEMPER_IIO_INTERVAL`, `TEMPER_IIO_HOLD`, the uhid device's
  `HID_PHYS` `temper-iio` (no alias for the old `tempered`: it only
  mattered for a crashed old daemon during a swap).  Test variables
  follow (`TEMPER_IIO_TWO_SENSORS`).
- Versions restart at 1.0.0 for all three.  New repository
  `charlieh0tel/temper` with this history but without the `v1.0.0` to
  `v3.0.0` tags, which would collide; the local clone deletes those
  tags and points `origin` at the new repository, or `make release
  VERSION=1.0.0` refuses and every build stamps `3.0.0+git...`.  The
  old repository `charlieh0tel/tempered-hid` is archived with a README
  pointing to the new one (archiving keeps its release assets and
  crates.io links working).  `tempered-hid` 3.0.0 is yanked once
  `temper-hid` 1.0.0 is out, and its trusted publisher removed.

### Shared code

- `Schedule` (fixed-interval, non-drifting; `temper log` and the
  daemon's poll) moves into `temper-hid` as a public `schedule` module,
  made safe: a zero interval means no waiting instead of a division by
  zero, and slot times saturate instead of overflowing.  The 1 s
  minimum poll interval moves there too, as a constant: it is the
  stick's ("slow to answer").
- The version stamp's `build.rs` lives in `temper-hid-cli` (a
  published crate may only use files inside itself); `temper-iio`
  points at it (`build = "../temper-hid-cli/build.rs"`).  Built from
  crates.io, with no git, it falls back to the crate version, as now.
  The variable becomes `TEMPER_VERSION`.
- The duration parsers (`10s`, `1m`; about ten lines over `jiff`) are
  duplicated in the two binaries, the one accepted copy: the
  alternatives were a third published crate or a public library
  target in the CLI crate.

### Migration from `tempered`

None: `tempered` was only ever installed on this machine, and has been
removed by hand.  Readers move from `/run/tempered` to
`/run/temper-iio`; smartclock-sensord does not read either yet.

### Transport: native on Linux, hidapi on Windows

Linux keeps the library's own hidraw code (sysfs discovery, `poll(2)`
with a deadline, `EINTR` retries, hang-up as removal).  Windows uses
the `hidapi` crate (2.6.7), `default-features = false`, feature
`windows-native` (pure Rust over `windows-sys`; its `build.rs` compiles
nothing for it), as a Windows-only dependency.

Rejected: hidapi on both (`linux-native-basic-udev`), reviewed
adversarially and reconsidered, 2026-10-07.  It would have deleted
the sysfs code, but:

- hidapi allows one Linux backend per dependency graph (`build.rs:102-106`;
  cargo unifies features), so any dependent also using hidapi's
  default backend fails to build without a feature dance.
- Parity with today's Linux transport would have to be rebuilt on top
  of it: hang-up arrives as a message string, not `ENODEV`;
  interrupted polls and early empty reads are not retried; open errors
  lose their `io::ErrorKind`; enumeration errors are swallowed
  (`linux_native.rs:46-54`), which cannot be fixed; each context scans
  sysfs (about 30 ms, 2000 opens here).
- The supposed gain, Linux hardware testing the Windows path, is
  small: what is shared is a thin, deterministic wrapper that a fake
  backend tests as well, and what is risky on Windows (pending
  overlapped reads, the 1 s write timeout, Win32 removal codes,
  interface numbers from the path, opening interface 1 beside the
  keyboard) lives in hidapi's Windows backend, which no Linux test
  touches.
- The daemon, the part that runs in production, would change
  transport for no benefit.

Checked against hidapi 2.6.7 and this bench (spike and review): it
builds without C for x86_64-pc-windows-gnu with mingw-w64, with
1.98.1 and the workspace MSRV 1.89; `HidDevice` is `Send`, not
`Sync`; the Windows backend strips the 0x00 report ID Windows
prepends to reads and pads writes; several contexts may coexist,
each enumerating every device.

Interface 1's usage page differs by model: vendor page 0xFF00 on the
TEMPerGold, Generic Desktop on the TEMPerHUM (both descriptors in
`docs/protocol.md`).  So discovery matches bus USB, VID:PID and
interface 1, never the usage page.

### Code structure

So two backends stay small and cannot drift, the platform code sits
behind one private trait, and everything else is shared:

```
crates/temper-hid/src/
  protocol.rs        unchanged but for removal mapping (below)
  hid/mod.rs         public API and shared logic
  hid/backend.rs     the private Backend trait and Candidate
  hid/linux.rs       #[cfg(target_os = "linux")]: today's hidraw code
  hid/windows.rs     #[cfg(windows)]: hidapi
```

- `Backend` (private): `enumerate() -> io::Result<Vec<Candidate>>`;
  `open(&Path) -> io::Result<Self>`; `write(&mut self, &Report)`
  (report ID 0 first, all of it or an error); `read(&mut self,
  Option<Duration>) -> io::Result<Option<Report>>`, one wait that may
  return early with `None` or `Interrupted`, removal as
  `NotConnected`.  `Send`.  One implementation per target, chosen by
  cfg; a fake implements it in the unit tests.
- `Candidate`: what discovery needs from either platform: path, bus,
  vendor and product ID, interface number.  Linux fills it from the
  hidraw parent's `uevent` (`HID_ID`, interface from `HID_PHYS`),
  Windows from hidapi's `DeviceInfo`.
- Shared in `hid/mod.rs`, unit-tested with the fake backend: the
  stick filter (bus USB, 3553:a001, interface 1), sorting and
  deduplication (hidapi lists a path once per top-level usage),
  `find`'s `NotFound`/`Several`, the read loop to a deadline (retrying
  early returns and `Interrupted`), the 1 s settle and 20 ms pause.
- Platform-only in each backend: Linux sysfs parsing, `poll`, hang-up
  to `NotConnected`; Windows path validation (UTF-8, no NUL, where
  hidapi would panic: `windows_native/mod.rs:415`), `Duration` to
  milliseconds rounding up with no limit as -1, the write count quirk
  (0 on synchronous completion, `windows_native/mod.rs:153-155`), and
  Win32 removal codes (`ERROR_DEVICE_NOT_CONNECTED`, likely
  `ERROR_OPERATION_ABORTED` and `ERROR_BAD_COMMAND`) to
  `NotConnected`, unverified without hardware.
- On other targets only `protocol` is built.

### API (module `hidraw` becomes `hid`)

- `hid::Device`: an open stick, implementing `protocol::Transport`;
  `Device::open(&Path)`, `Device::path()`.  `Send` everywhere;
  `Sync` and `AsFd` on Linux only (platform extras, documented).
- `hid::discover() -> Result<Vec<PathBuf>, hid::Error>`, sorted and
  deduplicated; `/dev/hidrawN` on Linux, device interface paths on
  Windows.
- `Stick::open(&Path)`, `Stick::find()` as now.
- `hid::Error`: `Enumerate { source }` (was `Scan`), `Open { path,
  source }`, `NotFound`, `Several`; sources are `io::Error`, so hidapi
  types stay out of the API.
- `VENDOR_ID`, `PRODUCT_ID` stay.
- Removal: both backends report `io::ErrorKind::NotConnected`;
  `protocol::Error::Gone` maps from it, and from `ENODEV` on Unix for
  existing third-party transports.  A replug that keeps the path (a
  udev symlink, a Windows interface path) is `Gone` for the open
  device; the caller reopens.

### Cross-building, CI and release

- Everything builds from Linux: `make windows-check` cross-builds
  `temper-hid` and `temper-hid-cli` for `x86_64-pc-windows-gnu` with
  mingw-w64, runs clippy, and runs their tests under wine.  Under wine
  `HidApi::new()` fails (`WinError: Config(19)`, wine 9.0), so only the
  portable tests and the Windows backend's pure helpers run there.
  The target is installed where needed, not in `rust-toolchain.toml`.
- CI and release use the shared workflows, as the other repos do.
  `rust-build-deb.yml` is called twice, `package: temper-hid-cli` and
  `package: temper-iio` (cargo package names; the deb names come from
  `[package.metadata.deb]`), with `artifact-suffix`.
- `rust-build-exes.yml` could not be used as it was: it built the
  whole workspace (and `temper-iio` does not build on Windows) and ran
  tests only on Linux.  So `deb-workflows` got additive inputs,
  `package`, `artifact-suffix` and `test-windows` (873d0f1, `v1`
  moved 2026-10-08).  The exe keeps that workflow's naming,
  `temper-x86_64-pc-windows-msvc.exe`, as in the other repositories;
  a plain `temper.exe` was dropped.  This repository calls it with
  `package: temper-hid-cli` and the Windows target only, in CI and on
  release, so `temper.exe` is built natively (MSVC, `windows-latest`)
  and its tests run on real Windows (the CLI's; the library's
  Windows-only helpers run under wine in `make windows-check`), and is
  attached to the release, marked untested on hardware.
- Order in `release.yml`: the two deb calls (they share a concurrency
  group), then the exe call (`needs` both, `contents: write`), then
  `notes`, `publish-crate` and `trigger-apt-repo` (`needs` all three).
  The audit runs in one call only.  This relies on GitHub releases
  staying mutable.
- `publish-crate` publishes `temper-hid`, then `temper-hid-cli`.  Both
  are new crates, so 1.0.0 of each is published by hand first (from
  the release commit, before the tag is pushed), then the trusted
  publishers are set (repository `charlieh0tel/temper`, `release.yml`);
  the job skips a version that already exists, so the first tag's run
  stays green.
- apt repository: one row for `charlieh0tel/temper` listing both
  packages (its `build-site.sh` downloads every deb of a repository's
  latest release; two rows would fetch twice), added before the new
  repository's `APT_REPO_TOKEN` is set; the `tempered` row goes once
  the new release exists.
- One Debian changelog, `packaging/debian/changelog`, headed `temper
  (1.0.0-1)`, serves both debs (a binary package's changelog names its
  source package), so `make release`, the version check and
  `release-notes.sh` stay as they are.  It starts fresh, pointing to
  the old repository for earlier history (keeping the `tempered`
  entries would make `release-notes.sh` link `v3.0.0...v1.0.0`).
  Checked with lintian on both debs.
- docs.rs: add `x86_64-pc-windows-msvc` for both published crates.
- Licensing: hidapi is MIT (its crate bundles C hidapi sources, unused
  here); `windows-sys` MIT OR Apache-2.0.

### Steps, one commit each, tests passing at each

1. Rename the library `tempered-hid` to `temper-hid` (code only).
2. Split `tempered-bin` into `temper-hid-cli` and `temper-iio`;
   `Schedule` and the minimum interval into `temper-hid`; the shared
   `build.rs`; the daemon loses its subcommand.
3. Packaging and system names (both debs, the shared changelog,
   `HID_PHYS`), CI and release workflows,
   Makefile, and every doc: README, crate READMEs, RELEASING.md,
   AGENTS.md, `docs/`, the unit's `Documentation=`, Cargo
   `repository`, the root tests' binary path and arguments, the test
   guard's hint, `TEMPER_IIO_TWO_SENSORS`.  `make test-hw` and lintian
   pass.
4. Create `charlieh0tel/temper`, push `main` without tags, delete the
   local old tags and repoint `origin`; apt repository row; set
   `APT_REPO_TOKEN`; archive the old repository with a pointer.
5. Refactor: removal portability (`Gone` also from `NotConnected`;
   `ENODEV` and `rustix` gated to Unix), and both models' interface 1
   descriptors in `docs/protocol.md` (the earlier claim of vendor page
   0xFF00 was the TEMPerGold's only).
6. Refactor: split `hidraw.rs` into the `hid` layout (still `hidraw`,
   still Linux-only), with the fake-backend tests; `make test-hw`
   passes.
7. API: `hidraw` to `hid`, `Hidraw` to `Device`, `Scan` to `Enumerate`;
   the Linux backend reports removal as `NotConnected`.  The API is
   then final.  Steps 1-7 done.
8. Release 1.0.0, Linux only: hand-publish `temper-hid`, then
   `temper-hid-cli`, from the release commit; set their trusted
   publishers; push the tag; install `temper` and `temper-iio` here;
   yank `tempered-hid` 3.0.0 and remove its trusted publisher.
   Decided 2026-10-07: get crates.io sorted with the final API first,
   rather than wait for Windows.
   Released 2026-10-08.
9. `deb-workflows`: the additive `rust-build-exes.yml` inputs (in that
   repository).  Done, `v1` moved.
10. Windows: `windows.rs` over hidapi, cfg gating in `temper-hid` and
    `temper-hid-cli`, `make windows-check`, the exe call in CI and
    release, docs.rs targets.  Released as 1.1.0: the same API on a
    new platform, so additive.  `temper.exe` cross-built here runs
    under wine; discovery fails there, as expected.

Windows ships untested on hardware, and the 1.1.0 notes say so:
opening interface 1 while Windows holds the boot keyboard (which can
type readings; Windows has no equivalent of our udev rule), the
timing, and the removal codes are unverified.

## temper 2.0.0: TEMPer2

A TEMPer2 (`TEMPer2_V4.1`, same USB ID 3553:a001; an inner probe and
a detachable outer temperature probe on a lead) joined the bench on
2026-10-08; 1.3.0 refuses it (unsupported firmware).  Captured: the
sensor type reply is `87 80 01` with the outer probe, `87 80 00`
without, and the temperature reply is two reports with it, one
without; the outer temperature is bytes 2-3 of the second report
(urwen/temper `TEMPer2_V3.7`/`V3.9`, offset 10, divisor 100).  The
stick sees the outer probe at power-up only: pulled at runtime, it
keeps sending two reports with `4e 20` in the outer slot; plugged in
again, it is not seen until the stick is replugged.  Decisions:

- The daemon presents one temperature: the outer probe's if the stick
  reports it fitted when identified, otherwise the inner probe's.
  The choice is fixed until the stick is replugged.  One IIO
  temperature device whose meaning never changes needs no patched
  kernel.
- An outer probe pulled at runtime (`4e 20`) is treated exactly as
  the stick being removed: the daemon exits cleanly, and a replug
  starts it again.  No fallback to the inner probe.  The library
  reports it as `Error::OuterProbeRemoved`, and the daemon logs it as
  such, at warning level, saying to replug the stick, rather than as
  `stick removed`.
- The library returns both temperatures; the CLI prints both.
- The temperature reply's length is a property of the identified
  stick, not of the command: `Stick` learns a private `Layout` when
  it identifies the stick and reads exactly that many reports, rather
  than waiting out a timeout on every reading.
- A TEMPer2 is `TEMPer2_V` and a version of 3.6 or later, the
  firmware ElfThing decodes as type 6; that excludes `TEMPer2_M12`
  (ElfThing type 3; urwen/temper: one report, outer at bytes 4-5).
  Both probes' range is ElfThing's default, -40 to 125 degrees C.
- Each temperature report's byte 1 is checked against its probe's
  code from the sensor type (`Error::WrongProbe`).
- Library API breaking (`Reading` gains the outer temperature,
  `Model::Temper2`), hence 2.0.0.

Done: refactor (cc40cca), TEMPer2 support and docs.  Remaining:
release.

## Phases

0. Scaffold: AGENTS.md, Cargo.toml, toolchain, lints, CI, this plan.
   **Done.**
1. TEMPerGold protocol module and hidraw discovery; `tempered read`
   and `tempered info` to check the hardware.  Fixtures captured from
   the stick; see `docs/protocol.md`.  **Done.**
2. uhid event codec (`crates/tempered-bin/src/uhid.rs`), golden-byte
   tests, and a root-only round trip through the real kernel
   (create, `UHID_START`, destroy, `UHID_STOP`).  **Done.**
3. HID sensor report descriptor and sensor state machine, tests, and
   a root-only test against the real drivers: the IIO device appears,
   `in_temp_raw` and `in_temp_scale` are right, hysteresis reads, and
   a read after 4 s idle returns at once.  **Done.**
4. Daemon, per `docs/daemon.md` (reviewed): threads, stale policy,
   `/run/tempered` link, signals, watchdog.  Root-only tests (`make test-hw`): one with no stick that
   creates the device, waits for IIO, and checks raw, scale and that a
   read after idle returns at once; one with the stick.
   Built: `tempered daemon` (`daemon.rs`, `supervisor.rs`), checked by
   hand against the stick and kernel: the IIO device appears, the
   link reads right, and stopping removes both.  Root tests
   (`make test-hw`) pass on 7.0.0-38, serialized: hold expiry and
   recovery, SIGTERM, unplug and `kill -9` with a fake stick, plus the
   real stick.  **Done.**
5. udev rules (hotplug start, hidraw group, keyboard deauthorize),
   systemd unit, tmpfiles.d, system-sleep hook, in `packaging/`;
   `docs/running.md`.  Written and statically checked (`udevadm
   verify`, `systemd-analyze verify`, `shellcheck`).  Installed from
   the .deb on 7.0.0-38: the unit starts from udev and serves,
   `OpenFile=` works under `DevicePolicy=closed`, and the keyboard is
   deauthorized.  The first install found the hidraw rule never
   matched (all `ATTRS{}` keys must match one ancestor); it now uses
   `usb_id`.  Replug works: `BindsTo=` stops the unit cleanly and
   udev starts it again.  Not yet tested on hardware: the suspend
   hook, and the keyboard's brief bind before it is deauthorized.
6. Debian packaging (`[package.metadata.deb]` in `tempered-bin`,
   `packaging/debian/`), release and audit workflows, Makefile
   (`make ci`, `test-hw` under sudo, `deb`, `release`),
   `RELEASING.md`.  `make deb` builds; lintian shows only warnings
   (`systemctl` in maintainer scripts, needed for the template's glob;
   no manual page).  Released as 1.0.0: the release workflow built
   both .debs, the apt repo picked up `tempered`, and `tempered-hid`
   is on crates.io (the first version by hand; Trusted Publishing
   from the next tag).  **Done.**
7. Docs: `docs/protocol.md`, `docs/daemon.md`, `docs/running.md`, and
   the descriptor rationale above.  Checked against the code for
   2.0.0.  **Done.**
8. TEMPerHUM (3.0.0, above): protocol and fixtures, humidity
   collection and link, CLI, docs; checked by hand against the bench
   TEMPerHUM with `read`, `info` and `log`.  Root tests (`make
   test-hw`) pass on 7.0.0-38: temperature and humidity against the
   real drivers, a fake TEMPerHUM through the daemon, and the real
   TEMPerHUM.  **Done.**
