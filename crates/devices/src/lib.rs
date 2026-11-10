// SPDX-License-Identifier: AGPL-3.0-or-later
//! Concrete emulated chips (§7).
//!
//! Each chip implements the [`device::Device`] swap seam (§7.1) so the
//! `machine` core and the `bus` never name a concrete device. The serial path
//! uses the [`uart16550::Uart16550`] controller (§7.2); the interrupt and timer
//! path uses the [`pic8259::Pic8259`] controller (§7.3) and the
//! [`pit8253::Pit8253`] timer (§7.4).
//!
//! All chips use [`device::TraceBuffer`]: access diagnostics default to Off,
//! opt into a bounded newest-event ring independently of output, and stay out
//! of snapshots. See `backlog/docs/design/observation.md`, §Traces and later audio.

pub mod ata;
pub mod cmos_rtc;
pub mod coprocessor;
pub mod dma8237;
pub mod i8042;
pub mod mda;
pub mod pic8259;
pub mod pit8253;
pub mod post_port;
pub mod sys_control;
pub mod uart16550;
pub mod upd765;
pub mod vga_text;

pub use ata::{Ata, AtaConfigError, AtaDrive, AtaGeometry};
pub use cmos_rtc::{CmosConfig, CmosRtc};
pub use coprocessor::Absent80287;
pub use device::{DmaMemory, DmaOp, DmaOutcome, DmaRequest};
pub use dma8237::{Dma8237, TransferType};
pub use i8042::I8042;
pub use mda::MdaText;
pub use pic8259::Pic8259;
pub use pit8253::Pit8253;
pub use post_port::PostPort;
pub use sys_control::SysControl;
pub use uart16550::Uart16550;
pub use upd765::{Drive, FloppyGeometry, Upd765};
pub use vga_text::VgaText;

#[cfg(test)]
pub(crate) mod trace_policy_tests {
    use super::*;
    use device::{AccessDir, Device, TraceEvent, TracePolicy, VClock};

    pub(crate) fn traced<T: Device>(mut device: T) -> T {
        device.set_trace_policy(TracePolicy::Ring {
            capacity: 1_000_000,
        });
        device
    }

    macro_rules! ring_test {
        ($name:ident, $device:expr, $port:expr) => {
            #[test]
            fn $name() {
                let mut bus = testkit::MockBus::new(Box::new($device));
                bus.device_mut()
                    .set_trace_policy(TracePolicy::Ring { capacity: 2 });
                let mut expected = Vec::new();
                for at in 1..=5 {
                    bus.tick(1);
                    let value = bus.port_read($port, 1).unwrap();
                    expected.push(TraceEvent::Port {
                        at: VClock(at),
                        port: $port,
                        size: 1,
                        dir: AccessDir::Read,
                        value,
                    });
                }
                assert_eq!(bus.device().trace_dropped(), 3);
                let snapshot = bus.device().save();
                bus.device_mut().restore(&snapshot).unwrap();
                assert_eq!(bus.device_mut().take_trace(), expected[3..]);
                assert_eq!(bus.device().trace_dropped(), 3);
                assert!(bus.device_mut().take_trace().is_empty());
                assert_eq!(bus.device().save(), snapshot);
                let mut restored = $device;
                restored.set_trace_policy(TracePolicy::Ring { capacity: 2 });
                restored.restore(&snapshot).unwrap();
                assert!(restored.take_trace().is_empty());
                assert_eq!(restored.trace_dropped(), 0);
                bus.device_mut().reset();
                bus.port_read($port, 1).unwrap();
                assert_eq!(bus.device_mut().take_trace().len(), 1);
                assert_eq!(bus.device().trace_dropped(), 0);
                bus.device_mut().set_trace_policy(TracePolicy::Off);
                bus.port_read($port, 1).unwrap();
                assert!(bus.device_mut().take_trace().is_empty());
                assert_eq!(bus.device().trace_dropped(), 0);
            }
        };
    }

    #[test]
    fn vga_mmio_and_ports_share_one_ring_independent_of_scanout() {
        let mut bus = testkit::MockBus::new(Box::new(VgaText::new()));
        bus.mmio_write(0xB8000, 2, 0x0741).unwrap();
        assert!(bus.device_mut().take_trace().is_empty());
        bus.device_mut()
            .set_trace_policy(TracePolicy::Ring { capacity: 2 });
        bus.port_write(0x3D4, 1, 0x0F).unwrap();
        bus.mmio_write(0xB8000, 2, 0x0742).unwrap();
        assert_eq!(bus.mmio_read(0xB8000, 2).unwrap(), 0x0742);
        assert_eq!(bus.device().trace_dropped(), 1);
        assert_eq!(
            bus.device_mut().take_trace(),
            vec![
                TraceEvent::Mmio {
                    at: VClock::ZERO,
                    addr: 0xB8000,
                    size: 2,
                    dir: AccessDir::Write,
                    value: 0x0742
                },
                TraceEvent::Mmio {
                    at: VClock::ZERO,
                    addr: 0xB8000,
                    size: 2,
                    dir: AccessDir::Read,
                    value: 0x0742
                },
            ]
        );
        assert_eq!(&bus.device().text_scanout().unwrap().vram[..2], &[b'B', 7]);
    }

    macro_rules! off_test {
        ($name:ident, $device:expr, $port:expr) => {
            #[test]
            fn $name() {
                let mut bus = testkit::MockBus::new(Box::new($device));
                for _ in 0..5 {
                    bus.port_read($port, 1).unwrap();
                }
                assert!(bus.device_mut().take_trace().is_empty());
            }
        };
    }

    ring_test!(uart_trace_ring, Uart16550::new(0x3F8), 0x3FD);
    off_test!(uart_trace_off, Uart16550::new(0x3F8), 0x3FD);
    ring_test!(post_trace_ring, PostPort::with_port(0x80), 0x80);
    off_test!(post_trace_off, PostPort::with_port(0x80), 0x80);
    ring_test!(ata_trace_ring, Ata::new(), 0x1F7);
    off_test!(ata_trace_off, Ata::new(), 0x1F7);
    ring_test!(mda_trace_ring, MdaText::new(), 0x3BA);
    off_test!(mda_trace_off, MdaText::new(), 0x3BA);
    ring_test!(vga_trace_ring, VgaText::new(), 0x3DA);
    off_test!(vga_trace_off, VgaText::new(), 0x3DA);
    ring_test!(pic_trace_ring, Pic8259::new(), 0x21);
    off_test!(pic_trace_off, Pic8259::new(), 0x21);
    ring_test!(pit_trace_ring, Pit8253::new(), 0x40);
    off_test!(pit_trace_off, Pit8253::new(), 0x40);
    ring_test!(keyboard_trace_ring, I8042::new(), 0x64);
    off_test!(keyboard_trace_off, I8042::new(), 0x64);
    ring_test!(rtc_trace_ring, CmosRtc::new(), 0x71);
    off_test!(rtc_trace_off, CmosRtc::new(), 0x71);
    ring_test!(coprocessor_trace_ring, Absent80287::new(), 0xF0);
    off_test!(coprocessor_trace_off, Absent80287::new(), 0xF0);
    ring_test!(word_dma_trace_ring, Dma8237::new_at_word(), 0xD0);
    off_test!(word_dma_trace_off, Dma8237::new_at_word(), 0xD0);
    ring_test!(dma_trace_ring, Dma8237::new(), 0x08);
    off_test!(dma_trace_off, Dma8237::new(), 0x08);
    ring_test!(fdc_trace_ring, Upd765::new(), 0x3F4);
    off_test!(fdc_trace_off, Upd765::new(), 0x3F4);
    ring_test!(system_trace_ring, SysControl::new(), 0x61);
    off_test!(system_trace_off, SysControl::new(), 0x61);
}
