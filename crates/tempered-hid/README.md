# tempered-hid

Read a PCsensor TEMPerGold USB thermometer (USB ID 3553:a001) over
Linux hidraw: temperature, firmware, probes, calibration and
manufacture date.  Linux only.

```rust
use tempered_hid::protocol::Stick;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stick = Stick::find()?;
    println!("{} C", stick.temperature()?);
    Ok(())
}
```

Reading the stick needs access to its hidraw node.  Other transports
plug in through `protocol::Transport`.

Part of [tempered-hid](https://github.com/charlieh0tel/tempered-hid),
which also presents the stick as a Linux IIO device.  The protocol and
its sources are in `docs/protocol.md` there.

MIT OR Apache-2.0.
