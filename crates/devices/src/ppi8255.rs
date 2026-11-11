// SPDX-License-Identifier: AGPL-3.0-or-later
//! Intel 8255A programmable peripheral interface, Mode 0.
//!
//! Intel 8255A datasheet, "Mode 0 (Basic Input/Output)", "Mode Definition
//! Format" and "Bit Set/Reset": independent A/B and split C directions,
//! output latches cleared by mode set, and C bit set/reset without mode change.
//! See `backlog/docs/design/firmware-and-machine-classes.md` §4.1 and §9 item 14.
//!
//! ## Coverage matrix
//! | Feature | Coverage / evidence |
//! |---|---|
//! | Mode 0, reset all inputs (9Bh), BSR, direction masks | READ: Intel 8255A datasheet, Mode Selection, Mode 0, Bit Set/Reset |
//! | Mode-set clears outputs; output reads return latch | READ: Intel 8255A datasheet, Mode Selection and Mode 0 |
//! | Writes while input | latch retained; visible only on output pins |
//! | Control-register read illegal | READ: Intel 8255A datasheet, Basic Operation table; returning FFh is ASSUMED open bus |
//! | Modes 1/2 | unsupported mode-set words ignored atomically; no handshake |
//! | Electrical levels and bus timing | not modelled; input pins supplied by embedding board |
//! | Snapshot | control, latches, input pins, absolute clock; traces excluded |

use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};

/// Intel 8255A in Mode 0 at four consecutive I/O ports.
pub struct Ppi8255 {
    base: u16,
    control: u8,
    latches: [u8; 3],
    inputs: [u8; 3],
    now: VClock,
    trace: device::TraceBuffer,
}

impl Ppi8255 {
    /// Construct with all ports configured as inputs. Base must be at most FFFCh.
    pub fn new(base: u16) -> Self {
        assert!(base <= 0xFFFC, "PPI port window exceeds port space");
        Self {
            base,
            control: 0x9B,
            latches: [0; 3],
            inputs: [0xFF; 3],
            now: VClock::ZERO,
            trace: device::TraceBuffer::default(),
        }
    }

    /// Drive external A/B/C input pins; output-configured bits ignore these.
    pub fn set_inputs(&mut self, inputs: [u8; 3]) {
        self.inputs = inputs;
    }

    /// Output-enable masks for A, B, and the two independently directed C halves.
    pub fn output_masks(&self) -> [u8; 3] {
        [
            if self.control & 0x10 == 0 { 0xFF } else { 0 },
            if self.control & 0x02 == 0 { 0xFF } else { 0 },
            (if self.control & 0x08 == 0 { 0xF0 } else { 0 })
                | (if self.control & 0x01 == 0 { 0x0F } else { 0 }),
        ]
    }

    /// Driven output bits; undriven pins return zero, not an electrical level.
    pub fn outputs(&self) -> [u8; 3] {
        let masks = self.output_masks();
        std::array::from_fn(|i| self.latches[i] & masks[i])
    }

    fn offset(&self, port: u16) -> Result<usize, DeviceError> {
        port.checked_sub(self.base)
            .filter(|&x| x < 4)
            .map(usize::from)
            .ok_or(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            })
    }
}

impl Device for Ppi8255 {
    fn name(&self) -> &'static str {
        "8255-ppi"
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let i = self.offset(port)?;
        let value = if i == 3 {
            0xFF
        } else {
            let mask = self.output_masks()[i];
            (self.latches[i] & mask) | (self.inputs[i] & !mask)
        };
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir: AccessDir::Read,
            value: value.into(),
        });
        Ok(value.into())
    }
    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        let i = self.offset(port)?;
        let byte = value as u8;
        if i < 3 {
            self.latches[i] = byte;
        } else if byte & 0x80 == 0 {
            let mask = 1 << ((byte >> 1) & 7);
            if byte & 1 != 0 {
                self.latches[2] |= mask;
            } else {
                self.latches[2] &= !mask;
            }
        } else if byte & 0x64 == 0 {
            self.control = byte;
            self.latches = [0; 3];
        }
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
        self.control = 0x9B;
        self.latches = [0; 3];
        self.inputs = [0xFF; 3];
        self.now = VClock::ZERO;
        self.trace.clear();
    }
    fn save(&self) -> Vec<u8> {
        let mut blob = vec![1, self.control];
        blob.extend(self.latches);
        blob.extend(self.inputs);
        blob.extend(self.now.0.to_le_bytes());
        blob
    }
    fn restore(&mut self, blob: &[u8]) -> Result<(), DeviceError> {
        if blob.len() != 16 || blob[0] != 1 || blob[1] & 0xE4 != 0x80 {
            return Err(DeviceError::InvalidSnapshot {
                device: self.name(),
                reason: "invalid Mode 0 snapshot",
            });
        }
        self.control = blob[1];
        self.latches.copy_from_slice(&blob[2..5]);
        self.inputs.copy_from_slice(&blob[5..8]);
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| blob[8 + i])));
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
    fn mode_words_directions_and_latch_reset() {
        let mut ppi = Ppi8255::new(0x60);
        ppi.set_inputs([0x12, 0x34, 0xA5]);
        let mut bus = MockBus::new(Box::new(ppi));
        for (port, value) in [(0x60, 0x12), (0x61, 0x34), (0x62, 0xA5), (0x63, 0xFF)] {
            assert_eq!(bus.port_read(port, 1).unwrap(), value);
        }
        bus.port_write(0x63, 1, 0x88).unwrap();
        for port in 0x60..=0x62 {
            bus.port_write(port, 1, 0x5A).unwrap();
        }
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0x5A);
        assert_eq!(bus.port_read(0x61, 1).unwrap(), 0x5A);
        assert_eq!(bus.port_read(0x62, 1).unwrap(), 0xAA);
        bus.port_write(0x63, 1, 0x81).unwrap();
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0);
        assert_eq!(bus.port_read(0x62, 1).unwrap(), 5);
        bus.port_write(0x62, 1, 0xFF).unwrap();
        assert_eq!(bus.port_read(0x62, 1).unwrap(), 0xF5);
        bus.port_write(0x63, 1, 0x99).unwrap();
        assert_eq!(bus.port_read(0x60, 1).unwrap(), 0x12);
        assert_eq!(bus.port_read(0x61, 1).unwrap(), 0);
        assert_eq!(bus.port_read(0x62, 1).unwrap(), 0xA5);
    }

    #[test]
    fn bsr_changes_each_c_bit_without_changing_directions() {
        let mut bus = MockBus::new(Box::new(Ppi8255::new(0x60)));
        bus.port_write(0x63, 1, 0x80).unwrap();
        for bit in 0..8 {
            bus.port_write(0x63, 1, bit * 2 + 1).unwrap();
            assert_eq!(bus.port_read(0x62, 1).unwrap(), 1 << bit);
            bus.port_write(0x63, 1, bit * 2).unwrap();
            assert_eq!(bus.port_read(0x62, 1).unwrap(), 0);
        }
        bus.port_write(0x60, 1, 0x6A).unwrap();
        for mode in [0xA0, 0xC0, 0x84] {
            bus.port_write(0x63, 1, mode).unwrap();
            assert_eq!(bus.port_read(0x60, 1).unwrap(), 0x6A);
        }
    }

    #[test]
    fn snapshot_roundtrip_and_atomic_rejection() {
        let mut bus = MockBus::new(Box::new(Ppi8255::new(0x100)));
        bus.port_write(0x103, 1, 0x80).unwrap();
        bus.port_write(0x100, 1, 0xC3).unwrap();
        bus.tick(17);
        let saved = bus.device().save();
        bus.device_mut().reset();
        bus.device_mut().restore(&saved).unwrap();
        assert_eq!(bus.port_read(0x100, 1).unwrap(), 0xC3);
        assert_eq!(bus.device().save(), saved);
        for invalid in [
            vec![],
            {
                let mut b = saved.clone();
                b[1] = 0xA0;
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
        assert!(bus.port_read(0x104, 1).is_err());
        assert!(bus.mmio_read(0, 1).is_err());
    }
}
