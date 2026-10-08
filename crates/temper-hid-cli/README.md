# temper-hid-cli

`temper`, a command-line tool for PCsensor TEMPerGold USB thermometers
and TEMPerHUM thermometers and hygrometers (USB ID 3553:a001), built
on the [`temper-hid`](https://crates.io/crates/temper-hid) library.

```
temper read    # degrees C, then %RH on a TEMPerHUM
temper info    # firmware, model, probes, calibration, manufacture date
temper log     # JSON Lines: --interval, --count, --time rfc3339|unix
```

`--device` picks a stick; without it the only attached one is used.
On Linux, reading a stick needs access to its hidraw node.

Runs on Linux and Windows.  Each release on GitHub carries
`temper-x86_64-pc-windows-msvc.exe`, built and tested on Windows
without a stick; it is untried on hardware.

Part of [temper](https://github.com/charlieh0tel/temper),
which also presents the stick as Linux IIO devices.

GPL-3.0-or-later.
