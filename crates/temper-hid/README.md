# temper-hid

Read a PCsensor TEMPerGold or TEMPer2 USB thermometer or TEMPerHUM
thermometer and hygrometer (all USB ID 3553:a001) on Linux (hidraw) and Windows
(hidapi): temperature, humidity, firmware, probes, calibration and
manufacture date.  Windows is built and tested without a stick but
untried on hardware.

```rust
use temper_hid::protocol::Stick;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stick = Stick::find()?;
    let reading = stick.reading()?;
    println!("{} C", reading.inner.temperature);
    if let Some(humidity) = reading.inner.humidity {
        println!("{humidity} %RH");
    }
    if let Some(outer) = reading.outer {
        println!("{} C outer probe", outer.temperature);
    }
    Ok(())
}
```

On Linux, reading the stick needs access to its hidraw node.  Other
transports plug in through `protocol::Transport`.

Part of [temper](https://github.com/charlieh0tel/temper).  The
protocol and its sources are in `docs/protocol.md` there.

MIT OR Apache-2.0.
