# tempered

Presents a PCsensor TEMPerGold USB thermometer as a Linux IIO device.

A small daemon reads the thermometer over hidraw and creates a virtual
HID sensor hub through `/dev/uhid`.  The kernel's `hid-sensor-hub` and
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
```

The stick is found automatically; `--device /dev/hidrawN` picks one.

## Documentation

- `PLAN.md`: architecture, decisions and open work.
- `docs/protocol.md`: the TEMPerGold commands and reply layouts.

## License

GPL-3.0-or-later; see `LICENSE`.
