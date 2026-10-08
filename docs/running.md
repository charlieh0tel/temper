# Running temper-iio

How the pieces in `packaging/` fit together.  The Debian package
installs them; the steps below do the same by hand.

Requires systemd 253 or later (for `OpenFile=`; the package depends on
it), a kernel of 4.14 or later with `uhid`, `hid-sensor-hub`,
`hid-sensor-temperature` and, for a TEMPerHUM, `hid-sensor-humidity`,
and no other HID temperature or humidity sensor on the machine
(`docs/daemon.md`).  One stick at a time.  Debian trixie and Ubuntu 24.04 qualify;
bookworm and Ubuntu 22.04 do not.

## Files

| Source | Installed as | Does |
|---|---|---|
| `packaging/udev/60-temper-iio.rules` | `/usr/lib/udev/rules.d/` | Gives the stick's data interface to group `temper-iio`, starts `temper-iio@hidrawN`, and deauthorizes the stick's keyboard interface |
| `packaging/systemd/temper-iio@.service` | `/usr/lib/systemd/system/` | Runs `temper-iio` for one stick; started by udev, not enabled |
| `packaging/systemd/temper-iio.default` | `/etc/default/temper-iio` | `TEMPER_IIO_LABEL`, `TEMPER_IIO_HUMIDITY_LABEL`, `TEMPER_IIO_INTERVAL`, `TEMPER_IIO_HOLD` |
| `packaging/tmpfiles.d/temper-iio.conf` | `/usr/lib/tmpfiles.d/` | Creates `/run/temper-iio` at boot |
| `packaging/system-sleep/temper-iio` | `/usr/lib/systemd/system-sleep/` | Stops the daemon around suspend |

## Installing by hand

```
sudo adduser --system --group --no-create-home temper-iio
sudo install -m 755 target/release/temper target/release/temper-iio /usr/bin/
sudo install -m 644 packaging/udev/60-temper-iio.rules /usr/lib/udev/rules.d/
sudo install -m 644 packaging/systemd/temper-iio@.service /usr/lib/systemd/system/
sudo install -m 644 packaging/systemd/temper-iio.default /etc/default/temper-iio
sudo install -m 644 packaging/tmpfiles.d/temper-iio.conf /usr/lib/tmpfiles.d/
sudo install -m 755 packaging/system-sleep/temper-iio /usr/lib/systemd/system-sleep/
sudo systemd-tmpfiles --create /usr/lib/tmpfiles.d/temper-iio.conf
sudo systemctl daemon-reload
sudo udevadm control --reload
sudo udevadm trigger --action=add --settle --parent-match=/sys/bus/usb/devices/3-1.3    # the stick's USB device
sudo systemctl start temper-iio@hidrawN    # the stick's node, as udev would
```

The order matters: the group must exist before udev reloads its
rules, since udev resolves `GROUP=` names when it reads them, and
`/run/temper-iio` must exist before the unit starts, since
`ReadWritePaths=` fails on a missing directory.

The package's `postinst` does the same, starting the daemon for each
stick already plugged in; removing the package stops it, and purging
it locks the `temper-iio` account rather than deleting it, as Debian does
with system accounts.  The suspend hook restarts only the daemons
whose stick is still present, without blocking resume.

## Checking

```
systemctl status 'temper-iio@*'
cat /run/temper-iio/temperature/in_temp_raw /run/temper-iio/temperature/in_temp_scale
cat /run/temper-iio/humidity/in_humidityrelative_raw    # TEMPerHUM
journalctl -u 'temper-iio@*'
```

`in_temp_raw` times `in_temp_scale` is millidegrees C;
`in_humidityrelative_raw` times its scale is thousandths of a percent
RH.

## Only one HID temperature sensor

The daemon exits 3 and stays stopped if the machine already has a HID
temperature sensor (or, for a TEMPerHUM, a HID humidity sensor): the
kernel cannot handle two.  So only one stick is supported at a time.  See
`docs/daemon.md`, "Only one HID temperature sensor".
