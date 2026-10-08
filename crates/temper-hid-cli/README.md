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
Reading a stick needs access to its hidraw node.

Part of [tempered-hid](https://github.com/charlieh0tel/tempered-hid),
which also presents the stick as Linux IIO devices.

GPL-3.0-or-later.
