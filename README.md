# tempered

Presents a PCsensor TEMPerGold USB thermometer as a Linux IIO device.

A small daemon reads the thermometer over hidraw and creates a virtual
HID sensor hub through `/dev/uhid`.  The kernel's `hid-sensor-hub` and
`hid-sensor-temperature` drivers bind to it and create an ordinary
`iio:deviceN`, readable with libiio, `iio_info`, or sysfs.

Status: early development; nothing works yet.  See `PLAN.md`.

## License

GPL-3.0-or-later; see `LICENSE`.
