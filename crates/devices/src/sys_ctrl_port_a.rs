// SPDX-License-Identifier: AGPL-3.0-or-later
//! System Control Port A at 92h, following IBM Personal System/2 Hardware
//! Interface Technical Reference, "System Control Port A (Hex 0092)".
//! See `backlog/docs/design/machine-config.md` §A20 line and System Control Port A.
//!
//! Coverage: bit 1 drives alternate A20; bit 0 requests CPU reset. IBM ThinkPad
//! 560 Technical Reference, "System Control Port A (Hex 0092)", reproduces the
//! PS/2 wording: a system reset or writing zero clears bit 0; a 0→1 write pulses
//! reset. The latch stays set during the reset so POST can read it. Bit 1 is
//! zero after a system reset. Intel386 EX User's Manual, PORT92, CPURST, confirms
//! that bit 0 remains one after CPU-RESET and must be cleared before another
//! reset. Accordingly, a CPU-only warm restart retains the latches; device
//! reset represents the system-reset clearing of these controls.
//!
//! The read/write mask is CBh. Bits 2 and 5 are reserved and read zero. Bit 4 is
//! read-only watchdog-timeout status (Bochs PORTS.LST, port 0092); this model
//! reports zero, no timeout, and ignores writes. No IRQ, MMIO or timed events
//! are generated. Reset uses the same board request line as the 8042.
//!
//! ASSUMED model behavior for bit 3 (security lock) and bits 6–7 (fixed-disk
//! activity light): ordinary read/write latches, initially zero. IBM ThinkPad
//! 560 Technical Reference, "System Control Port A (Hex 0092)", permits writing
//! bit 3 only while it is zero; that write-once restriction, password protection
//! and the physical light are not modelled. Reset pulse timing is ASSUMED:
//! duration is not modelled, only the request event.
//!
//! Versioned snapshots retain latches, the pending shared reset request and
//! absolute trace time; restoring drives only this device's A20 source and
//! does not synthesize a reset edge. The diagnostic trace is not snapshotted.

use std::{cell::Cell, rc::Rc};

use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};

use crate::A20Line;

/// Fixed byte-wide port decoded by System Control Port A.
pub const SYS_CTRL_PORT_A: u16 = 0x92;
const NAME: &str = "sys-ctrl-port-a";
const MASK: u8 = 0xCB;
const SNAPSHOT_VERSION: u8 = 1;
const SNAPSHOT_LEN: usize = 11;

/// Alternate A20 and CPU-reset controls, with inert security/light latches.
///
/// IBM ThinkPad 560 Technical Reference, "System Control Port A (Hex 0092)",
/// and Intel386 EX User's Manual, PORT92, CPURST, define the reset controls.
/// Assumptions for pulse timing and security/light bits are described in the
/// module documentation. Not fitted to an IBM 5170.
pub struct SysCtrlPortA {
    latch: u8,
    a20: A20Line,
    reset: Rc<Cell<bool>>,
    trace: device::TraceBuffer,
    now: VClock,
}

impl SysCtrlPortA {
    /// Power-on device with private board lines, suitable for isolation tests.
    #[must_use]
    pub fn new() -> Self {
        Self::with_board_lines(Rc::new(Cell::new(false)), A20Line::default())
    }

    /// Power-on device attached to the motherboard's shared reset and A20 lines.
    #[must_use]
    pub fn with_board_lines(reset: Rc<Cell<bool>>, a20: A20Line) -> Self {
        a20.set_fast(false);
        Self {
            latch: 0,
            a20,
            reset,
            trace: device::TraceBuffer::default(),
            now: VClock::ZERO,
        }
    }

    fn check(port: u16) -> Result<(), DeviceError> {
        if port == SYS_CTRL_PORT_A {
            Ok(())
        } else {
            Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            })
        }
    }

    fn trace_port(&mut self, size: u8, dir: AccessDir, value: u32) {
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port: SYS_CTRL_PORT_A,
            size,
            dir,
            value,
        });
    }
}

impl Default for SysCtrlPortA {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for SysCtrlPortA {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        Self::check(port)?;
        let value = u32::from(self.latch);
        self.trace_port(size, AccessDir::Read, value);
        Ok(value)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        Self::check(port)?;
        let next = value as u8 & MASK;
        if self.latch & 1 == 0 && next & 1 != 0 {
            self.reset.set(true);
        }
        self.latch = next;
        self.a20.set_fast(next & 2 != 0);
        self.trace_port(size, AccessDir::Write, value);
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
        self.latch = 0;
        self.a20.set_fast(false);
        self.reset.set(false);
        self.now = VClock::ZERO;
        self.trace.clear();
    }

    fn save(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(SNAPSHOT_LEN);
        blob.extend_from_slice(&[SNAPSHOT_VERSION, self.latch, u8::from(self.reset.get())]);
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        let reason = if snapshot.len() != SNAPSHOT_LEN {
            Some("snapshot length mismatch")
        } else if snapshot[0] != SNAPSHOT_VERSION {
            Some("unsupported snapshot version")
        } else if snapshot[1] & !MASK != 0 || snapshot[2] > 1 {
            Some("invalid control bits")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason,
            });
        }
        self.latch = snapshot[1];
        self.a20.set_fast(self.latch & 2 != 0);
        self.reset.set(snapshot[2] != 0);
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| snapshot[3 + i])));
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    fn rig() -> (MockBus, A20Line, Rc<Cell<bool>>) {
        let a20 = A20Line::default();
        let reset = Rc::new(Cell::new(false));
        let bus = MockBus::new(Box::new(SysCtrlPortA::with_board_lines(
            reset.clone(),
            a20.clone(),
        )));
        (bus, a20, reset)
    }

    #[test]
    fn port_a_readback_and_every_bit_follow_the_latch_mask() {
        let (mut bus, a20, reset) = rig();
        assert_eq!(bus.port_read(0x92, 1).unwrap(), 0);
        for value in 0..=255 {
            bus.port_write(0x92, 1, value).unwrap();
            assert_eq!(bus.port_read(0x92, 1).unwrap(), value & u32::from(MASK));
            assert_eq!(a20.enabled(), value & 2 != 0);
        }
        bus.port_write(0x92, 1, 0).unwrap();
        assert!(
            reset.get(),
            "clearing bit zero must not cancel a pending request"
        );
    }

    #[test]
    fn port_a_reset_requires_a_new_rising_edge() {
        let (mut bus, _, reset) = rig();
        for value in [0, 2, 0xCA] {
            bus.port_write(0x92, 1, value).unwrap();
            assert!(!reset.get());
        }
        bus.port_write(0x92, 1, 1).unwrap();
        assert!(reset.replace(false));
        assert_eq!(bus.port_read(0x92, 1).unwrap(), 1);
        bus.tick(1_000_000);
        assert_eq!(bus.port_read(0x92, 1).unwrap(), 1);
        bus.port_write(0x92, 1, 3).unwrap();
        assert!(!reset.get());
        bus.port_write(0x92, 1, 0).unwrap();
        bus.port_write(0x92, 1, 1).unwrap();
        assert!(reset.get());
    }

    #[test]
    fn port_a_and_keyboard_drive_all_a20_combinations_and_shared_reset() {
        let (mut fast, a20, reset) = rig();
        let mut keyboard = MockBus::new(Box::new(
            crate::I8042::new().with_board_lines(reset.clone(), a20.clone()),
        ));
        assert!(a20.enabled());
        assert_eq!(a20.change_count(), 1);
        for (kbc, port, effective, changes) in [
            (false, false, false, 2),
            (false, true, true, 3),
            (true, true, true, 3),
            (true, false, true, 3),
            (false, false, false, 4),
        ] {
            keyboard.port_write(0x64, 1, 0xD1).unwrap();
            keyboard
                .port_write(0x60, 1, if kbc { 3 } else { 1 })
                .unwrap();
            fast.port_write(0x92, 1, if port { 2 } else { 0 }).unwrap();
            assert_eq!(a20.enabled(), effective);
            assert_eq!(a20.change_count(), changes);
        }
        keyboard.port_write(0x64, 1, 0xFE).unwrap();
        assert!(reset.replace(false));
        fast.port_write(0x92, 1, 1).unwrap();
        assert!(reset.replace(false));
        fast.device_mut().reset();
        assert!(!a20.enabled());
        keyboard.device_mut().reset();
        assert!(a20.enabled(), "8042 power-on source is enabled");
        fast.device_mut().reset();
        assert!(
            a20.enabled(),
            "resetting port A must preserve the other source"
        );
    }

    #[test]
    fn port_a_snapshots_restore_sources_pending_reset_and_trace_time_without_an_edge() {
        let (mut bus, a20, reset) = rig();
        bus.device_mut()
            .set_trace_policy(device::TracePolicy::Ring { capacity: 8 });
        bus.tick(42);
        bus.port_write(0x92, 1, 0xCB).unwrap();
        let pending = bus.device().save();
        reset.set(false);
        let consumed = bus.device().save();
        bus.device_mut().reset();
        assert!(!a20.enabled());
        bus.device_mut().restore(&pending).unwrap();
        assert!(a20.enabled());
        assert!(reset.get());
        assert_eq!(bus.device().save(), pending);
        bus.device_mut().restore(&consumed).unwrap();
        assert!(!reset.get(), "restore must not synthesize a bit-zero edge");
        assert_eq!(bus.port_read(0x92, 1).unwrap(), 0xCB);
        assert_eq!(
            bus.device_mut().take_trace(),
            vec![TraceEvent::Port {
                at: VClock(42),
                port: 0x92,
                size: 1,
                dir: AccessDir::Read,
                value: 0xCB,
            }]
        );
    }

    #[test]
    fn port_a_invalid_snapshots_are_atomic_and_unmapped_accesses_fail() {
        let (mut bus, a20, reset) = rig();
        bus.port_write(0x92, 1, 3).unwrap();
        let good = bus.device().save();
        let mut bad_version = good.clone();
        bad_version[0] += 1;
        let mut reserved = good.clone();
        reserved[1] |= 4;
        let mut flag = good.clone();
        flag[2] = 2;
        for bad in [vec![], vec![1; 12], bad_version, reserved, flag] {
            assert!(bus.device_mut().restore(&bad).is_err());
            assert_eq!(bus.device().save(), good);
            assert!(a20.enabled());
            assert!(reset.get());
        }
        assert!(bus.port_read(0x91, 1).is_err());
        assert!(bus.port_write(0x93, 1, 0).is_err());
        assert!(bus.device_mut().mmio_read(0x92, 1).is_err());
        assert!(bus.device_mut().mmio_write(0x92, 1, 0).is_err());
        assert!(!bus.device().irq_pending());
        assert_eq!(bus.device().next_event(VClock::ZERO), NextEvent::Dormant);
        bus.device_mut().reset();
        assert_eq!(bus.device().save(), SysCtrlPortA::new().save());
        assert!(!a20.enabled());
        assert!(!reset.get());
    }

    #[test]
    fn port_a_traces_record_raw_writes_and_masked_reads_at_virtual_time() {
        let (mut bus, _, _) = rig();
        bus.device_mut()
            .set_trace_policy(device::TracePolicy::Ring { capacity: 8 });
        bus.tick(7);
        bus.port_write(0x92, 1, 0xFF).unwrap();
        assert_eq!(bus.port_read(0x92, 1).unwrap(), 0xCB);
        assert_eq!(
            bus.device_mut().take_trace(),
            vec![
                TraceEvent::Port {
                    at: VClock(7),
                    port: 0x92,
                    size: 1,
                    dir: AccessDir::Write,
                    value: 0xFF
                },
                TraceEvent::Port {
                    at: VClock(7),
                    port: 0x92,
                    size: 1,
                    dir: AccessDir::Read,
                    value: 0xCB
                },
            ]
        );
        assert!(bus.device_mut().take_trace().is_empty());
    }

    #[test]
    fn keyboard_snapshot_restore_updates_only_its_a20_source() {
        let (mut fast, a20, reset) = rig();
        let mut keyboard = MockBus::new(Box::new(
            crate::I8042::new().with_board_lines(reset, a20.clone()),
        ));
        let enabled = keyboard.device().save();
        keyboard.port_write(0x64, 1, 0xD1).unwrap();
        keyboard.port_write(0x60, 1, 1).unwrap();
        let disabled = keyboard.device().save();
        assert!(!a20.enabled());
        keyboard.device_mut().restore(&enabled).unwrap();
        assert!(a20.enabled());
        keyboard.device_mut().restore(&disabled).unwrap();
        assert!(!a20.enabled());
        fast.port_write(0x92, 1, 2).unwrap();
        keyboard.device_mut().restore(&disabled).unwrap();
        assert!(a20.enabled());
        fast.port_write(0x92, 1, 0).unwrap();
        assert!(!a20.enabled());
    }
}
