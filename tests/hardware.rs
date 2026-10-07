//! Tests against a real stick.  Need one attached and root:
//! `sudo -E cargo test -- --ignored`.

use tempered::hidraw;
use tempered::hidraw::Hidraw;
use tempered::temper::Stick;

#[test]
#[ignore = "needs a TEMPerGold and root"]
fn reads_attached_stick() {
    let found = hidraw::discover().unwrap();
    let [device] = found.as_slice() else {
        panic!("expected exactly one stick, found {found:?}");
    };
    let mut stick = Stick::new(Hidraw::open(device).unwrap());
    assert!(stick.firmware().unwrap().starts_with("TEMPerGold_"));
    stick.temperature().unwrap();
}
