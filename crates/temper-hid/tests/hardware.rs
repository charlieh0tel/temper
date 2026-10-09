//! Tests against a real stick.  Need exactly one attached, and root:
//! `sudo -E cargo test -- --ignored`.

// Discovery and the transport exist on Linux and Windows.
#![cfg(any(target_os = "linux", windows))]

use temper_hid::protocol::Stick;

#[test]
#[ignore = "needs a TEMPerGold, TEMPerHUM or TEMPer2, and root"]
fn reads_attached_stick() {
    let mut stick = Stick::find().unwrap();
    let model = stick.firmware().unwrap().model().unwrap();
    let reading = stick.reading().unwrap();
    assert_eq!(reading.inner.humidity.is_some(), model.has_humidity());
}
