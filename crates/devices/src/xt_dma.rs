// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM PC/XT single 8237A and its external DMA page latches.
//!
//! IBM Personal Computer XT Technical Reference (6280089, March 1986),
//! §1 "System Board", "DMA" (p. 1-14), "I/O Address Map" (p. 1-24)
//! and §5 BIOS listing; Intel 8237A datasheet, Register Description and
//! Single Transfer Mode. See `backlog/docs/design/firmware-and-machine-classes.md`
//! §4.1 and §9 item 14.
//!
//! ## Coverage matrix
//! | Feature | Coverage / evidence |
//! |---|---|
//! | Single byte controller at 00h–1Fh | READ: §1 I/O Address Map; upper 16 ports alias lower registers in this XT wrapper; shared AT model unchanged |
//! | Page decode at 080h–09Fh | READ: §1 I/O Address Map |
//! | 81h channel 2 page, 83h written during POST | READ: §5 BIOS listing |
//! | Conventional 81h/82h/83h/87h → channels 2/3/1/0 | ASSUMED: §1 DMA; hardware is a 4×4 74LS670 register file decoded on A0–A1, so future board wiring may replace this 16-latch compatibility model |
//! | Page bits 0–3 → A16–A19 | 20-bit bus READ: §1 DMA; upper latch bits discarded by ASSUMPTION |
//! | 90h–9Fh mirror 80h–8Fh; unused latches retained | ASSUMED partial decode within §1 I/O Address Map range; 87h supplies channel 0 page |
//! | Page readback | ASSUMED: deterministic latch readback, not electrical open bus |
//! | Refresh / transfer / arbitration timing | refresh not scheduled; inherits byte-level 8237 simplifications |
//! | Snapshot | shared controller state, all page nibbles validated; traces excluded |

use crate::Dma8237;
use device::{
    AccessDir, Device, DeviceError, DmaMemory, DmaTransfer, NextEvent, TraceEvent, VClock,
};

/// Single XT DMA controller, with 20-bit page wiring and no AT page aliases.
#[derive(Default)]
pub struct XtDma {
    core: Dma8237,
    now: VClock,
    trace: device::TraceBuffer,
}
impl XtDma {
    /// Construct the single byte controller with masked channels and zero pages.
    pub fn new() -> Self {
        Self::default()
    }
    /// Current 20-bit address of a local channel (0–3).
    pub fn physical_address(&self, channel: usize) -> u32 {
        self.core.physical_address(channel)
    }
    fn check(port: u16) -> Result<(), DeviceError> {
        if port <= 0x1F || (0x80..=0x9F).contains(&port) {
            Ok(())
        } else {
            Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: port.into(),
            })
        }
    }
}
impl Device for XtDma {
    fn name(&self) -> &'static str {
        "xt-dma"
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        Self::check(port)?;
        let register = if port >= 0x80 {
            0x80 | (port & 15)
        } else {
            port & 15
        };
        let value = self.core.port_read(register, size)?;
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Read,
            value,
        });
        Ok(value)
    }
    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        Self::check(port)?;
        let register = if port >= 0x80 {
            0x80 | (port & 15)
        } else {
            port & 15
        };
        self.core.port_write(
            register,
            size,
            if port >= 0x80 { value & 0x0F } else { value },
        )?;
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Write,
            value,
        });
        Ok(())
    }
    fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
        self.core.mmio_read(addr, size)
    }
    fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
        self.core.mmio_write(addr, size, value)
    }
    fn tick(&mut self, now: VClock) {
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now >= self.now {
            self.now = now;
            self.core.tick(now);
        }
    }
    fn next_event(&self, _now: VClock) -> NextEvent {
        NextEvent::Dormant
    }
    fn irq_pending(&self) -> bool {
        false
    }
    fn dma_service_write(
        &mut self,
        channel: u8,
        mem: &mut dyn DmaMemory,
        data: &[u8],
    ) -> DmaTransfer {
        self.core.dma_service_write(channel, mem, data)
    }
    fn dma_service_read(
        &mut self,
        channel: u8,
        mem: &mut dyn DmaMemory,
        out: &mut [u8],
    ) -> DmaTransfer {
        self.core.dma_service_read(channel, mem, out)
    }
    fn reset(&mut self) {
        self.core.reset();
        self.now = VClock::ZERO;
        self.trace.clear();
    }
    fn save(&self) -> Vec<u8> {
        let mut blob = vec![1];
        blob.extend(self.core.save());
        blob
    }
    fn restore(&mut self, blob: &[u8]) -> Result<(), DeviceError> {
        let invalid = || DeviceError::InvalidSnapshot {
            device: "xt-dma",
            reason: "invalid XT DMA snapshot",
        };
        if blob.first() != Some(&1) {
            return Err(invalid());
        }
        let mut core = Dma8237::new();
        core.restore(&blob[1..])?;
        for port in 0x80..=0x8F {
            let page = core.port_read(port, 1)?;
            if page > 15 {
                return Err(invalid());
            }
        }
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| {
            blob[blob.len() - 8 + i]
        })));
        self.core = core;
        Ok(())
    }
    fn set_trace_policy(&mut self, policy: device::TracePolicy) {
        self.trace.set_policy(policy);
    }
    fn trace_dropped(&self) -> u64 {
        self.trace.dropped()
    }
    fn take_trace(&mut self) -> Vec<TraceEvent> {
        self.trace.take()
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    #[test]
    fn xt_page_map_masks_to_twenty_bits_and_decodes_aliases() {
        let mut bus = MockBus::new(Box::new(XtDma::new()));
        for (port, channel, page) in [
            (0x81, 2, 0xFC),
            (0x82, 3, 0xAB),
            (0x83, 1, 0xE7),
            (0x87, 0, 0xD5),
        ] {
            bus.port_write(port, 1, page).unwrap();
            assert_eq!(bus.port_read(port, 1).unwrap(), page & 15);
            bus.port_write(0x0C, 1, 0).unwrap();
            bus.port_write(channel * 2, 1, 0x34).unwrap();
            bus.port_write(channel * 2, 1, 0x12).unwrap();
            let dma = bus
                .device_mut()
                .as_any_mut()
                .unwrap()
                .downcast_mut::<XtDma>()
                .unwrap();
            assert_eq!(
                dma.physical_address(channel as usize),
                ((page & 15) << 16) | 0x1234
            );
        }
        for port in [0x20, 0x7F, 0xA0, 0xC0] {
            assert!(bus.port_read(port, 1).is_err());
            assert!(bus.port_write(port, 1, 0xFF).is_err());
        }
    }

    #[test]
    fn controller_upper_ports_alias_all_sixteen_registers() {
        for register in 0..16 {
            let mut canonical = MockBus::new(Box::new(XtDma::new()));
            let mut aliased =
                MockBus::new(Box::new(crate::trace_policy_tests::traced(XtDma::new())));
            let value = u32::from(register) * 2 + 1;
            canonical.port_write(register, 1, value).unwrap();
            aliased.port_write(0x10 + register, 1, value).unwrap();
            assert_eq!(aliased.device().save(), canonical.device().save());
            assert_eq!(
                aliased.port_read(0x10 + register, 1),
                canonical.port_read(register, 1)
            );
            assert_eq!(
                aliased.device_mut().take_trace()[0],
                TraceEvent::Port {
                    at: VClock::ZERO,
                    port: 0x10 + register,
                    size: 1,
                    dir: AccessDir::Write,
                    value,
                }
            );
        }
        let mut bus = MockBus::new(Box::new(XtDma::new()));
        bus.port_write(0x1C, 1, 0).unwrap();
        bus.port_write(0x12, 1, 0x34).unwrap();
        bus.port_write(0x02, 1, 0x12).unwrap();
        bus.port_write(0x1C, 1, 0).unwrap();
        assert_eq!(bus.port_read(0x12, 1).unwrap(), 0x34);
        assert_eq!(bus.port_read(0x02, 1).unwrap(), 0x12);
    }

    #[test]
    fn page_window_decodes_every_port_and_preserves_original_trace_address() {
        let mut bus = MockBus::new(Box::new(crate::trace_policy_tests::traced(XtDma::new())));
        for offset in 0..16 {
            bus.port_write(0x80 + offset, 1, 0xA0 | u32::from(offset))
                .unwrap();
            assert_eq!(bus.port_read(0x90 + offset, 1).unwrap(), u32::from(offset));
            bus.port_write(0x90 + offset, 1, 15 - u32::from(offset))
                .unwrap();
            assert_eq!(
                bus.port_read(0x80 + offset, 1).unwrap(),
                15 - u32::from(offset)
            );
        }
        let saved = bus.device().save();
        bus.device_mut().reset();
        bus.device_mut().restore(&saved).unwrap();
        assert_eq!(bus.device().save(), saved);
        bus.port_write(0x97, 1, 0xAB).unwrap();
        assert_eq!(bus.port_read(0x87, 1).unwrap(), 0x0B);
        assert_eq!(
            bus.device_mut().take_trace()[0],
            TraceEvent::Port {
                at: VClock::ZERO,
                port: 0x97,
                size: 1,
                dir: AccessDir::Write,
                value: 0xAB
            }
        );
    }

    struct Memory(Vec<u8>);
    impl DmaMemory for Memory {
        fn dma_read(&self, addr: u32) -> u8 {
            self.0[addr as usize]
        }
        fn dma_write(&mut self, addr: u32, value: u8) {
            self.0[addr as usize] = value;
        }
    }

    #[test]
    fn xt_transfer_uses_page_and_preserves_snapshot() {
        let mut bus = MockBus::new(Box::new(XtDma::new()));
        for (port, byte) in [
            (0x81, 0xF2),
            (0x0C, 0),
            (4, 0xFF),
            (4, 0xFF),
            (5, 1),
            (5, 0),
            (0x0B, 0x46),
            (0x0A, 2),
        ] {
            bus.port_write(port, 1, byte).unwrap();
        }
        let mut mem = Memory(vec![0; 0x100000]);
        let result = bus
            .device_mut()
            .dma_service_write(2, &mut mem, &[0xA5, 0x5A]);
        assert_eq!(result.transferred, 2);
        assert!(result.terminal_count);
        assert_eq!(mem.0[0x2FFFF], 0xA5);
        assert_eq!(mem.0[0x20000], 0x5A);
        bus.tick(19);
        let saved = bus.device().save();
        bus.device_mut().reset();
        bus.device_mut().restore(&saved).unwrap();
        assert_eq!(bus.device().save(), saved);
        assert_eq!(bus.port_read(0x81, 1).unwrap(), 2);
        for invalid in [
            vec![],
            {
                let mut b = saved.clone();
                b[0] = 2;
                b
            },
            {
                let mut b = saved.clone();
                b[39] = 0xF0;
                b
            },
            {
                let mut b = saved.clone();
                b[45] = 0x10;
                b
            },
        ] {
            assert!(bus.device_mut().restore(&invalid).is_err());
            assert_eq!(bus.device().save(), saved);
        }
    }
}
