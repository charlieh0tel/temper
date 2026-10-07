# Running tempered

How the pieces in `packaging/` fit together.  The Debian package
installs them; the steps below do the same by hand.

Requires systemd 253 or later (for `OpenFile=`; the package depends on
it), a kernel of 4.14 or later with `uhid`, `hid-sensor-hub` and
`hid-sensor-temperature`, and no other HID temperature sensor on the
machine (`docs/daemon.md`).  Debian trixie and Ubuntu 24.04 qualify;
bookworm and Ubuntu 22.04 do not.

## Files

| Source | Installed as | Does |
|---|---|---|
| `packaging/udev/60-tempered.rules` | `/usr/lib/udev/rules.d/` | Gives the stick's data interface to group `tempered`, starts `tempered@hidrawN`, and deauthorizes the stick's keyboard interface |
| `packaging/systemd/tempered@.service` | `/usr/lib/systemd/system/` | Runs `tempered daemon` for one stick; started by udev, not enabled |
| `packaging/systemd/tempered.default` | `/etc/default/tempered` | `TEMPERED_LABEL`, `TEMPERED_INTERVAL`, `TEMPERED_HOLD` |
| `packaging/tmpfiles.d/tempered.conf` | `/usr/lib/tmpfiles.d/` | Creates `/run/tempered` at boot |
| `packaging/system-sleep/tempered` | `/usr/lib/systemd/system-sleep/` | Stops the daemon around suspend |

## Installing by hand

```
sudo adduser --system --group --no-create-home tempered
sudo install -m 755 target/release/tempered /usr/bin/
sudo install -m 644 packaging/udev/60-tempered.rules /usr/lib/udev/rules.d/
sudo install -m 644 packaging/systemd/tempered@.service /usr/lib/systemd/system/
sudo install -m 644 packaging/systemd/tempered.default /etc/default/tempered
sudo install -m 644 packaging/tmpfiles.d/tempered.conf /usr/lib/tmpfiles.d/
sudo install -m 755 packaging/system-sleep/tempered /usr/lib/systemd/system-sleep/
sudo systemd-tmpfiles --create /usr/lib/tmpfiles.d/tempered.conf
sudo systemctl daemon-reload
sudo udevadm control --reload
sudo udevadm trigger --action=add --settle --parent-match=/sys/bus/usb/devices/3-1.3    # the stick's USB device
sudo systemctl start tempered@hidrawN    # the stick's node, as udev would
```

The order matters: the group must exist before udev reloads its
rules, since udev resolves `GROUP=` names when it reads them, and
`/run/tempered` must exist before the unit starts, since
`ReadWritePaths=` fails on a missing directory.

The package's `postinst` does the same, starting the daemon for each
stick already plugged in; removing the package stops it, and purging
it locks the `tempered` account rather than deleting it, as Debian does
with system accounts.  The suspend hook restarts only the daemons
whose stick is still present, without blocking resume.

## Checking

```
systemctl status 'tempered@*'
cat /run/tempered/temperature/in_temp_raw /run/tempered/temperature/in_temp_scale
journalctl -u 'tempered@*'
```

`in_temp_raw` times `in_temp_scale` is millidegrees C.

## Only one HID temperature sensor

The daemon exits 3 and stays stopped if the machine already has a HID
temperature sensor: the kernel cannot handle two.  See
`docs/daemon.md`, "Only one HID temperature sensor".
