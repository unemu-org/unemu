// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM PC/XT NMI mask latch at A0h.
//!
//! IBM Personal Computer XT Technical Reference (6280089, March 1986),
//! §1 "System Board", "I/O Address Map" (p. 1-24): bit 7 enables NMI
//! (opposite to the AT CMOS mask), decoded throughout A0h–AFh.
//! See `backlog/docs/design/firmware-and-machine-classes.md` §4.1 and §9 item 14.
//!
//! ## Coverage matrix
//! | Feature | Coverage / evidence |
//! |---|---|
//! | A0h–AFh aliases, bit 7 enables, power-on disabled | READ: §1 I/O Address Map, 0AXh decode and 80h/00h enable/disable writes |
//! | Readback | ASSUMED: write-only hardware; model returns FFh open bus |
//! | NMI sources / CPU delivery | not modelled: enable state only, never IRQ |
//! | Snapshot | enable state and absolute clock; traces excluded |

use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};

/// XT NMI enable latch; no interrupt source or CPU delivery is attached.
#[derive(Default)]
pub struct XtNmiMask {
    enabled: bool,
    now: VClock,
    trace: device::TraceBuffer,
}

impl XtNmiMask {
    /// Power on with NMI disabled.
    pub fn new() -> Self {
        Self::default()
    }
    /// Whether port A0h bit 7 permits an NMI source to reach the CPU.
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    fn check(port: u16) -> Result<(), DeviceError> {
        if (0xA0..=0xAF).contains(&port) {
            Ok(())
        } else {
            Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: port.into(),
            })
        }
    }
}
impl Device for XtNmiMask {
    fn name(&self) -> &'static str {
        "xt-nmi-mask"
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        Self::check(port)?;
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Read,
            value: 0xFF,
        });
        Ok(0xFF)
    }
    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        Self::check(port)?;
        self.enabled = value & 0x80 != 0;
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Write,
            value,
        });
        Ok(())
    }
    fn mmio_read(&mut self, addr: u64, _size: u8) -> Result<u64, DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }
    fn mmio_write(&mut self, addr: u64, _size: u8, _value: u64) -> Result<(), DeviceError> {
        Err(DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: addr,
        })
    }
    fn tick(&mut self, now: VClock) {
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now >= self.now {
            self.now = now;
        }
    }
    fn next_event(&self, _now: VClock) -> NextEvent {
        NextEvent::Dormant
    }
    fn irq_pending(&self) -> bool {
        false
    }
    fn reset(&mut self) {
        self.enabled = false;
        self.now = VClock::ZERO;
        self.trace.clear();
    }
    fn save(&self) -> Vec<u8> {
        let mut blob = vec![1, self.enabled.into()];
        blob.extend(self.now.0.to_le_bytes());
        blob
    }
    fn restore(&mut self, blob: &[u8]) -> Result<(), DeviceError> {
        if blob.len() != 10 || blob[0] != 1 || blob[1] > 1 {
            return Err(DeviceError::InvalidSnapshot {
                device: self.name(),
                reason: "invalid NMI mask snapshot",
            });
        }
        self.enabled = blob[1] != 0;
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| blob[2 + i])));
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
    fn every_nmi_alias_updates_one_latch() {
        let mut bus = MockBus::new(Box::new(XtNmiMask::new()));
        for port in 0xA0..=0xAF {
            bus.port_write(port, 1, 0x80).unwrap();
            assert_eq!(bus.device().save()[1], 1);
            bus.port_write(port, 1, 0x7F).unwrap();
            assert_eq!(bus.device().save()[1], 0);
            assert_eq!(bus.port_read(port, 1).unwrap(), 0xFF);
            assert!(!bus.device().irq_pending());
        }
        for port in [0x9F, 0xB0] {
            assert!(bus.port_read(port, 1).is_err());
            assert!(bus.port_write(port, 1, 0).is_err());
        }
    }

    #[test]
    fn bit_seven_enables_and_snapshot_restores_atomically() {
        let mut bus = MockBus::new(Box::new(XtNmiMask::new()));
        for (value, enabled) in [(0x7F, false), (0x80, true), (0xFF, true), (0, false)] {
            bus.port_write(0xA0, 1, value).unwrap();
            assert_eq!(
                bus.device_mut()
                    .as_any_mut()
                    .unwrap()
                    .downcast_mut::<XtNmiMask>()
                    .unwrap()
                    .enabled(),
                enabled
            );
            assert_eq!(bus.port_read(0xA0, 1).unwrap(), 0xFF);
            assert!(!bus.device().irq_pending());
        }
        bus.port_write(0xA0, 1, 0x80).unwrap();
        bus.tick(29);
        let saved = bus.device().save();
        bus.device_mut().reset();
        bus.device_mut().restore(&saved).unwrap();
        assert_eq!(bus.device().save(), saved);
        for invalid in [
            vec![],
            {
                let mut b = saved.clone();
                b[1] = 2;
                b
            },
            {
                let mut b = saved.clone();
                b[0] = 2;
                b
            },
        ] {
            assert!(bus.device_mut().restore(&invalid).is_err());
            assert_eq!(bus.device().save(), saved);
        }
        assert!(bus.port_read(0xB0, 1).is_err());
        assert!(bus.port_write(0x70, 1, 0).is_err());
        assert!(bus.mmio_read(0, 1).is_err());
    }
}
