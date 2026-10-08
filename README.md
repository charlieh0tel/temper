# temper

Software for the PCsensor TEMPerGold and TEMPerHUM USB sticks
(3553:a001):

- `temper`: a command-line tool.  Linux and Windows.
- `temper-hid`: the Rust library under it.  Linux and Windows.
- `temper-iio`: a Linux daemon that presents a stick as IIO devices.

## Install

- Debian and Ubuntu: `sudo apt install temper temper-iio` from
  [apt-repo](https://github.com/charlieh0tel/apt-repo).
- Rust: `cargo install temper-hid-cli`, or the
  [`temper-hid`](https://crates.io/crates/temper-hid) crate.
- Windows: the `.exe` on the
  [releases](https://github.com/charlieh0tel/temper/releases) page,
  untried on hardware.

## Use

```
temper read    # degrees C, then %RH on a TEMPerHUM
temper info    # firmware, model, calibration
temper log     # JSON Lines
```

The udev rule in the `temper` package grants the stick to the user at
the seat and to group `temper`.

`temper-iio` serves `/run/temper-iio/temperature` and, on a TEMPerHUM,
`/run/temper-iio/humidity`.  It handles one stick per machine, and
holds it: stop it to use `temper`.  See `docs/running.md`.

## Build

```
make ci             # what CI runs
make test-hw        # root tests against the kernel and a stick
make windows-check  # Windows build and tests, from Linux
make deb            # both packages
```

Releases: `RELEASING.md`.  Design: `PLAN.md` and `docs/`.
Licenses: library MIT OR Apache-2.0; programs GPL-3.0-or-later.
