# temper

Software for the PCsensor TEMPerGold, TEMPerHUM and TEMPer2 USB sticks
(3553:a001):

- `temper`: a command-line tool.  Linux and Windows.
- `temper-hid`: the Rust library under it.  Linux and Windows.

Earlier releases also had `temper-iio`, a Linux daemon that presented
a stick as IIO devices; it was dropped after 1.3.0 (`PLAN.md`).
Programs that need readings use `temper-hid`.

## Install

- Debian and Ubuntu: `sudo apt install temper` from
  [apt-repo](https://github.com/charlieh0tel/apt-repo).
- Rust: `cargo install temper-hid-cli`, or the
  [`temper-hid`](https://crates.io/crates/temper-hid) crate.
- Windows: the `.exe` on the
  [releases](https://github.com/charlieh0tel/temper/releases) page,
  untried on hardware.

## Use

```
temper read    # degrees C; then %RH on a TEMPerHUM, outer probe on a TEMPer2
temper info    # firmware, model, calibration
temper log     # JSON Lines
```

The udev rule in the `temper` package grants the stick to the user at
the seat and to group `temper`.  One process at a time can have a
stick open.

## Build

```
make ci             # what CI runs
make test-hw        # root test against the attached stick
make windows-check  # Windows build and tests, from Linux
make deb            # the temper package
```

Releases: `RELEASING.md`.  Design: `PLAN.md` and `docs/protocol.md`.
Licenses: library MIT OR Apache-2.0; programs GPL-3.0-or-later.
