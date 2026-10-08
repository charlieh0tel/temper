//! Tests against a real stick.  Need exactly one attached, and root:
//! `sudo -E cargo test -- --ignored`.

// Discovery and the hidraw transport are Linux only, for now.
#![cfg(target_os = "linux")]

use temper_hid::protocol::Stick;

#[test]
#[ignore = "needs a TEMPerGold or TEMPerHUM, and root"]
fn reads_attached_stick() {
    let mut stick = Stick::find().unwrap();
    let model = stick.firmware().unwrap().model().unwrap();
    let reading = stick.reading().unwrap();
    assert_eq!(reading.humidity.is_some(), model.has_humidity());
}
