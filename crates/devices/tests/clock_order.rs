// SPDX-License-Identifier: AGPL-3.0-or-later
//! Isolated checks of the Device monotonic-clock contract (§6).

use device::{Device, VClock};
use devices::*;
use testkit::MockBus;

fn check_clock_order(device: Box<dyn Device>, sync: bool) {
    let mut bus = MockBus::new(device);
    bus.tick(10);
    let snapshot = bus.device().save();
    // Equal-time synchronization is valid at repeated service boundaries.
    bus.device_mut().sync(VClock(10));
    assert_eq!(bus.device().save(), snapshot);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if sync {
            bus.device_mut().sync(VClock(9));
        } else {
            bus.device_mut().tick(VClock(9));
        }
    }));
    if cfg!(debug_assertions) {
        assert!(result.is_err(), "backwards time must fail in debug builds");
    } else {
        assert!(result.is_ok(), "release builds ignore backwards time");
    }
    assert_eq!(bus.device().save(), snapshot);
    bus.tick(1);
}

macro_rules! clock_checks {
    ($name:ident, $make:expr) => {
        mod $name {
            use super::*;

            #[test]
            fn tick_rejects_backwards_time() {
                check_clock_order(Box::new($make), false);
            }

            #[test]
            fn sync_rejects_backwards_time() {
                check_clock_order(Box::new($make), true);
            }
        }
    };
}

clock_checks!(pit, Pit8253::new());
clock_checks!(rtc, CmosRtc::new());
clock_checks!(pic, Pic8259::new());
clock_checks!(keyboard, I8042::new());
clock_checks!(dma, Dma8237::new());
clock_checks!(fdc, Upd765::new());
clock_checks!(uart, Uart16550::new(0x3F8));
clock_checks!(post, PostPort::new());
clock_checks!(system, SysControl::new());
clock_checks!(vga, VgaText::new());
clock_checks!(mda, MdaText::new());

clock_checks!(ata, Ata::new());
