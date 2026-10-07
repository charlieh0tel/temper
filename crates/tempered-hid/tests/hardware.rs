//! Tests against a real stick.  Need exactly one attached, and root:
//! `sudo -E cargo test -- --ignored`.

use tempered_hid::protocol::Stick;

#[test]
#[ignore = "needs a TEMPerGold and root"]
fn reads_attached_stick() {
    let mut stick = Stick::find().unwrap();
    assert!(
        stick
            .firmware()
            .unwrap()
            .as_str()
            .starts_with("TEMPerGold_")
    );
    stick.temperature().unwrap();
}
