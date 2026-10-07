# tempered

Presents a PCsensor TEMPerGold USB thermometer as a Linux IIO device.

A small program, `tempered`, run as a daemon by systemd, reads the
thermometer over hidraw and creates a virtual HID sensor hub through
`/dev/uhid`.  The kernel's `hid-sensor-hub` and
`hid-sensor-temperature` drivers bind to it and create an ordinary
`iio:deviceN`, readable with libiio, `iio_info`, or sysfs.

Status: early development.  Reading the stick works; the IIO side does
not exist yet.  See `PLAN.md`.

## Usage

Reading the stick needs access to its hidraw node, so for now run as
root:

```
sudo tempered read     # temperature in degrees C
sudo tempered info     # firmware, probes, calibration, manufacture date
sudo tempered log      # a JSON line per reading, every 10s
```

`tempered log` takes `--interval` (at least `1s`), `--count`, and
`--time rfc3339|unix` for the `time` field:

```
{"time":"2026-10-06T18:40:12.345Z","temperature_c":34.93,"centi_celsius":3493}
```

A failed read prints `{"time":...,"error":"..."}` and logging
continues.

The stick is found automatically; `--device /dev/hidrawN` picks one.

## Crates

- `crates/tempered`: a library that only talks to the stick: protocol,
  discovery and hidraw I/O.  Usable without the daemon:

  ```rust
  let mut stick = tempered::protocol::Stick::find()?;
  println!("{} C", stick.temperature()?);
  ```

- `crates/tempered-bin`: the `tempered` program: `read` and `info`,
  and (to come) `daemon`, which presents the stick as an IIO device.

## Documentation

- `PLAN.md`: architecture, decisions and open work.
- `docs/protocol.md`: the TEMPerGold commands and reply layouts.
- `docs/daemon.md`: the design of `tempered daemon`.

## License

The `tempered` program is GPL-3.0-or-later; see `LICENSE`.  The `tempered`
library is MIT OR Apache-2.0; see `crates/tempered/LICENSE-MIT` and
`crates/tempered/LICENSE-APACHE`.
