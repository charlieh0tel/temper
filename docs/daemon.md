# `temper-iio`

How the daemon works.  It reads the stick and presents it as IIO
devices through `/dev/uhid`: temperature, and humidity on a
TEMPerHUM, using the codec in
`crates/temper-iio/src/uhid.rs` and the sensor in `sensor.rs`;
`PLAN.md` holds the decisions it builds on.  Kernel
references are to `drivers/hid/uhid.c`, `drivers/hid/hidraw.c` and
`drivers/hid/hid-sensor-hub.c`.

## Command line

```
temper-iio [--device /dev/hidrawN] [--label NAME]
           [--humidity-label NAME] [--interval 10s] [--hold 60s]
```

The options also read an environment variable
(clap's `env` feature), so the unit's `EnvironmentFile=/etc/default/temper-iio`
configures it:

- `TEMPER_IIO_LABEL`, default `temperature`: the temperature link.
  Matches `[a-z0-9][a-z0-9_-]{0,63}`, since it becomes a file name in
  `/run/temper-iio` and the uhid device name.
- `TEMPER_IIO_HUMIDITY_LABEL`, default `humidity`: the humidity link,
  used only for a TEMPerHUM.  Same rules; must differ from the
  temperature label.
- `TEMPER_IIO_INTERVAL`, default `10s`, at least `1s`.
- `TEMPER_IIO_HOLD`, default `60s`, at least twice the interval, so one
  missed reading never expires the hold.

`--device` comes from the unit instance (`/dev/%I`); without it the
only attached stick is used.  Option errors, including the
cross-field hold and label checks, are clap errors; those and a bad `LISTEN_*`
handoff from systemd exit 2, which the unit lists in
`RestartPreventExitStatus=`.

It runs in the foreground and never forks.

## Exit status

| Status | Meaning |
|---|---|
| 0 | Stopped by `SIGTERM`, `SIGINT` or `SIGHUP`, or the stick was unplugged |
| 1 | Runtime failure, e.g. `/run/temper-iio` unusable; restarted |
| 2 | Configuration error; not restarted |
| 3 | The device cannot be presented: the stick's firmware is not supported (e.g. a TEMPer2 with the same USB ID), another HID temperature or humidity sensor exists, or the IIO devices never appeared three times running (e.g. a missing kernel module); not restarted |

Unplugging exits 0: `BindsTo=` stops the unit anyway, and a failure
status would only make `Restart=on-failure` churn.  Status 3 is not
restarted (`RestartPreventExitStatus=2 3`): each attempt takes 30 s
or more, so systemd's start limit would never trip and the daemon
would cycle forever.  A panic aborts the process in release builds
(`panic = "abort"`, `SIGABRT`, restarted).  In dev builds a panic on
the main thread exits 101, and one on a worker thread ends only that
thread; a dead poll thread then stops the watchdog pings.

## Logging

To stderr, one line per state transition, not per reading: started,
reading resumed, readings failing (once, with the error), the IIO
device created (with its path), held value expired and device
destroyed, label in use and waiting, a link not yet written, and
stopped.  A long outage repeats its failure line hourly.  Each line
starts with a `sd-daemon(3)` priority prefix (`<3>` error, `<4>`
warning, `<6>` info, `<7>` debug), which journald honors.  When stderr
is not journald's (`JOURNAL_STREAM` unset or not matching stderr's
device and inode, `systemd.exec(5)`), the prefix is dropped and a
timestamp added instead.  Errors are formatted on one line (`{:#}`), since journald applies a
prefix to one line only.  Write errors on stderr are ignored.

## Startup

1. Parse options.
2. Open the stick's hidraw node; query the firmware to learn the
   model, TEMPerGold or TEMPerHUM, up to 5 times a second apart while extending the
   start timeout, since a stick can be slow right after plug-in.  A
   stick that answers with other firmware exits 3 at once.
3. Get `/dev/uhid`.  If `LISTEN_PID` is this process, the fd named
   `uhid` in `LISTEN_FDNAMES` (the unit says `OpenFile=/dev/uhid:uhid`)
   is fd `3 + its position`, checked against `LISTEN_FDS`.  It is
   adopted with `OwnedFd::from_raw_fd` (the one `unsafe`, approved),
   given `FD_CLOEXEC` with `rustix::io::fcntl_setfd` (systemd passes it
   without), and checked with `fstat` to be a character device with
   rdev 10:239 (`UHID_MINOR`).  If `LISTEN_PID` matches but no `uhid`
   fd is named, that is a configuration error.  If `LISTEN_PID` is
   absent, `/dev/uhid` is opened directly (needs root; tests and
   manual runs).  The `LISTEN_*` variables are left set: unsetting
   them is `unsafe` in edition 2024 and pointless, since the daemon
   never execs.
4. Take each label's lock (below), waiting if another instance holds
   it.  While waiting, it sends `EXTEND_TIMEOUT_USEC=` each tick, since
   the wait counts against `TimeoutStartSec=` and the watchdog is not
   active before `READY=1`.
5. Start the threads and send `READY=1`.

The virtual device is not created yet: it exists only while there is
a reading to serve.  Being up with no device is normal, so `READY=1`
does not wait for the first reading.

## Threads and state

Shared: `Mutex<Option<Sensor>>`, `None` while no virtual device
exists, and the uhid `File`, shared as an `Arc<File>` (one open
instance; never reopened).  Each event is one `write()`, which
the kernel takes whole under its device lock and answers with the
full count.

Lock rule: build the bytes under the sensor mutex, release it, then
write.  The mutex is never held across a uhid write or any wait,
because the driver probe, run from a kernel worker, needs the uhid
thread to take the mutex and answer.  Order: the sensor becomes
`Some` before `CREATE2` is written, and `None` only after `DESTROY`
returns.

- **uhid thread.**  Blocks reading full `EVENT_SIZE` events (`EINTR`
  retried) and decodes them.  A request is answered at once from the
  sensor.  With no sensor it is not answered: a destroy has already
  failed every pending request in the kernel.  Write errors on
  replies (`EINVAL` once the device is not running, a stale request
  id dropped by the kernel) are logged at debug level, never fatal.
  Stale `GET_REPORT`s can survive a destroy in the kernel's queue;
  their replies fail with `EINVAL`, or after a new `CREATE2` are
  accepted and dropped by request id, which is per fd and never
  reused.  Lifecycle events (`START`, `STOP`,
  `OPEN`, `CLOSE`), including the old device's `STOP` and `CLOSE`
  after a destroy, are logged at debug level.
- **poll thread.**  Queries the stick on a fixed schedule from t = 0,
  shared with `temper log` (`temper_hid::schedule`), stamps a shared "last
  progress" time, and sends each result to the main thread.  A query
  normally takes under 1 s; a failing one can take over 5 s (a hidraw
  write waits out the USB control timeout).
- **destroy thread.**  Writes `DESTROY` when the main thread asks and
  reports back.  A destroy can block for a long time (below); main
  keeps pinging the watchdog meanwhile, since killing the daemon could
  not interrupt the kernel anyway.
- **signal thread.**  `signal_hook::iterator::Signals` for `SIGTERM`, `SIGHUP`
  and `SIGINT`; sends a shutdown message to the main thread.
- **main thread.**  Owns the lifecycle.  Waits on its channel with a
  timeout of one watchdog tick, pings the watchdog, and runs the
  supervisor.

Every `poll()`, `read()` and `write()` retries `EINTR`.  uhid's write
takes an interruptible lock (`uhid_char_write()`), so a signal can
interrupt it even under `SA_RESTART`; retrying a reply, `INPUT2` or
`DESTROY` is idempotent.  `panic = "abort"` in the workspace's release
profile, so a dead thread ends the process instead of leaving it half
alive; profiles apply only at the workspace root and never to tests,
so library users are unaffected.

Threads treat a failed channel send as the signal to quit quietly and
never unwrap channel operations: once `main` returns, its receiver is
gone, and a panic there would abort a clean exit.

## Stick failure and unplug

After an unplug, hidraw's `poll` reports `POLLHUP`/`POLLERR` (set only
when the device is gone) and `read` returns `EIO`; `ENODEV` comes only
from write and ioctl (`hidraw.c`).  The public `Transport` trait
returns `io::Result`, so it is not changed: `Hidraw::receive` reports
hang-up as an `ENODEV` `io::Error`, and the library's `protocol::Error`
has a `Gone` variant ("the device was removed") that the conversion
from `io::Error` produces for `ENODEV`, covering the write path too,
and for kind `NotConnected` (the portable form; `ENODEV` is checked
on Unix only).

## Only one HID temperature sensor

`hid-sensor-temperature` keeps one static `hid_sensor_hub_callbacks`
for all its instances and overwrites its `pdev` on every probe
(`drivers/iio/temperature/hid-sensor-temperature.c`; humidity has the
same bug, the other HID sensor drivers keep theirs per instance).
With two temperature sensors, reports for one are delivered with the
other's device; once that one is removed, `temperature_capture_sample()`
dereferences NULL and the kernel oopses, leaving the removal stuck.
Found by running the root tests concurrently.

So the daemon's sensors must be the only ones of their kinds on the
machine, and only one stick is supported at a time:

- Before every create, the daemon looks for a `HID-SENSOR-200033.*`
  platform device, and on a TEMPerHUM a `HID-SENSOR-200032.*`
  (humidity) one; if one exists, it logs why and exits 3.  One left
  by a crashed instance of this daemon (its uhid parent has
  `HID_PHYS=temper-iio`) gets up to 10 s to go away first: a crashed
  process releases its label lock before the kernel finishes
  destroying its device.
- It also only checks before creating.  Anything that creates a HID
  temperature sensor later (another program, a test run beside the
  service) can still oops the kernel and kill the daemon; that
  happened once, 2026-10-07.
- On a replug, the label lock keeps the new instance waiting until the
  old one has exited, after its destroy, so two never overlap.
- The root tests that create a sensor take a process-wide lock (`test_support::one_sensor`).

A fix for the kernel (per-instance callbacks, as the accelerometer
driver does; in `patches/`, sent upstream, not yet merged) would lift
this; until then several sticks on one machine are not supported.

## Supervisor

A pure state machine, unit-tested without devices: inputs are a
reading or a failure with its time; outputs are actions.  An unplug
(`Gone`) is handled by the main thread, not the supervisor.

- **Absent.**  A good reading: set the sensor, write `CREATE2`, wait
  up to 10 s for the IIO devices (pinging the watchdog and checking for
  shutdown every 100 ms), create the links; go to Present.  If the
  `CREATE2` write fails, or an IIO device does not appear: log,
  `DESTROY` if created, clear the sensor, stay Absent.  Three such
  failures in a row: exit 3.  A create cut short by shutdown does not
  count, and never replaces the shutdown's exit status.
- **Present.**  A good reading: update the sensor, write an `INPUT2`
  per quantity (so buffered mode and triggers get data), record the
  time.  If
  `INPUT2` fails with `EINVAL` (only possible if the kernel stopped
  the device; kept as a defensive branch), go to Absent as below.  A
  failure: if the last good reading is older than the hold age, remove
  the links, write `DESTROY`, clear the sensor; go to Absent.  Ages are
  measured on `CLOCK_BOOTTIME`, so time suspended counts: a reading
  from before a suspend is not served as fresh after it.
- **Gone**, handled by the main thread in either state: remove the
  links, `DESTROY` if Present, exit 0.

The descriptor has one application collection per quantity, each with
its own report ID: temperature (1), and on a TEMPerHUM humidity (2,
usage 0x200032, data field 0x200433, which `hid-sensor-humidity`
binds).  `hid-sensor-hub` makes a platform device per collection, so
one uhid device gives one IIO device per quantity, created and
destroyed together.  The collections differ only in usages and report
ID; see `sensor.rs` and `PLAN.md`, "Report descriptor requirements".

`CREATE2` fields: name = the temperature label, phys = `temper-iio`, uniq =
`temper-iio-<pid>-<n>` (unique per creation, so an IIO device from an
earlier creation is never mistaken for the new one), bus
`BUS_VIRTUAL`, VID:PID 3553:a001, version and country 0.

## The links

One link per quantity: `/run/temper-iio/<label>` to the temperature IIO
device, and on a TEMPerHUM `/run/temper-iio/<humidity-label>` to the
humidity one.  Each label has its own lock, as below.

After `CREATE2`, the main thread polls `/sys/bus/iio/devices` every
100 ms for, per quantity, an `iio:device*` entry (not `trigger*`: the
driver also registers a trigger under the same parent) whose `name` is
the driver's (`temperature`, `humidity`) and whose resolved path has
an ancestor with `HID_UNIQ=<uniq>` in its `uevent`.  This matching is
tested against the kernel (`sensor.rs`).

A label is owned through `/run/temper-iio/<label>.lock`, mode 0600,
taken with an exclusive `flock` at startup and held for the process's
life, so a crash releases it.  The mode matters: `flock` works on any
file a process can open, so a lock file others could read could be
held by them, stalling the daemon.  No suffixes: one stick is expected, and on a
replug the new instance may start while the old one still holds the
lock through its destroy, so the new one waits for the lock (logging
once, extending the start timeout) rather than taking another name.  On
taking the lock, a leftover `/run/temper-iio/<label>` is removed: a free
lock means no live owner.  Lock files are never removed.

A link `/run/temper-iio/<label>` is replaced atomically: remove any
leftover `/run/temper-iio/.<label>.tmp`, symlink it to the resolved
sysfs path (which contains the never-reused HID sequence number),
rename it over.  The links are removed before every `DESTROY` and on
exit.  If writing a link fails while Present, that is logged and
retried with each reading.

`/run/temper-iio` itself comes from tmpfiles.d, mode 0755, owned by
`temper-iio`, so readers such as smartclock-sensord can follow the links.

## Shutdown

On the shutdown message the main thread sends `STOPPING=1`, removes
the links, has `DESTROY` written if Present and waits for it, and
returns from `main` without joining the other threads: process exit ends them, wherever
they are blocked.  `DESTROY` needs no help from the uhid thread: the
kernel fails pending requests itself.

## Watchdog and notification

Messages go to `NOTIFY_SOCKET` with an unbound `UnixDatagram`: a path,
or an abstract address for a leading `@`
(`SocketAddrExt::from_abstract_name`); `vsock:` and other forms are
ignored, and nothing is sent when it is unset.  Sent: `READY=1`,
`WATCHDOG=1`, `STOPPING=1`, and `STATUS=` with the state.

The watchdog is enabled when `WATCHDOG_USEC` is set and `WATCHDOG_PID`
is unset or this process, and `WATCHDOG_USEC` is not 0, as
`sd_watchdog_enabled(3)` does; the tick is a quarter of
`WATCHDOG_USEC` but at least 10 ms, else 1 s.  `WATCHDOG=1` is sent each
main-loop iteration and inside every wait, but only while the poll
thread's "last progress" stamp is within interval + 10 s, so a hung
stick query stops the pings.  The unit uses `WatchdogSec=60s`.

A `DESTROY` can block for 5 s per concurrent reader: once uhid stops
answering, each in-flight IIO read still waits out the sensor hub's
5 s completion timeout (`sensor_hub_input_attr_get_raw_value()`),
serialized, and IIO attributes are world-readable.  That is why
`DESTROY` runs on its own thread while main keeps pinging.  Shutdown
is bounded by `TimeoutStopSec=` in the same way; a kill cannot take
effect until the kernel returns.

## Tests

- Supervisor: unit tests over sequences of readings, failures, gone
  and the clock (hold expiry, recovery, `INPUT2` failure, three IIO
  timeouts).
- Label validation, lock, link replacement and leftover cleanup: unit
  tests in a temporary directory.
- Notification and watchdog enablement: unit tests with a
  `UnixDatagram` pair and set environments.
- fd adoption: unit tests of the `LISTEN_*` parsing.
- Sensor: the descriptor's report lengths, the temperature collection
  byte-identical to 2.0.0's, a second collection for humidity with its
  own report ID.
- Root only (`make test-hw`): the descriptor against the real drivers
  (temperature alone, and temperature with humidity: names, raw
  values, scale 10, hysteresis, a read after idle); and a fake stick made from a second
  uhid device (`BUS_USB`, phys ending `/input1`, VID:PID 3553:a001)
  that answers hidraw writes (`UHID_OUTPUT`) with `INPUT2`, so the
  real binary runs as a subprocess with `--device /dev/hidrawN`:
  - the link appears and `in_temp_raw` follows the fake's readings;
  - a failing fake past the hold age removes the link and the IIO
    device, and recovery brings both back;
  - destroying the fake (unplug) makes the daemon clean up and exit 0;
  - `SIGTERM` cleans up and exits 0; `kill -9` leaves no IIO device;
  - a fake TEMPerHUM gets both links, with the right values, and
    `SIGTERM` removes both.
- Root only and opt-in (`TEMPER_IIO_TWO_SENSORS=1`): two sensors, one
  destroyed while the other sends input reports.  Oopses a stock
  kernel; for testing the fix in `patches/`.
- On the real unit: `OpenFile=` under `DevicePolicy=closed` and
  replug work (`PLAN.md`, phase 5); without `DeviceAllow=/dev/uhid
  rw`, and suspend, are untested.
