// SPDX-License-Identifier: AGPL-3.0-or-later
//! Non-invasive access to the shared Intel 8259A observation seam (§5.1, §10).
//!
//! Intel 8259A datasheet, "Interrupt Sequence"; see
//! `backlog/docs/design/scheduler.md` § Migration for the A/B diagnostic use.

use chipset::SharedPic;
use testkit::MockBus;

#[test]
fn pic_device_face_is_reachable_by_the_observation_harness() {
    let mut bus = MockBus::new(Box::new(SharedPic::new()));
    assert!(bus
        .device_mut()
        .as_any_mut()
        .and_then(|any| any.downcast_mut::<SharedPic>())
        .is_some());
}
