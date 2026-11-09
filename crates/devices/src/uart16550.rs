// SPDX-License-Identifier: AGPL-3.0-or-later
//! National Semiconductor 16550 UART (§7.2), the first emulated device.
//!
//! Models the 16550's eight consecutive port-mapped registers at
//! `base..=base+7` (COM1's `base` is `0x3F8`). This is the subset needed to
//! carry transmitted bytes to an optional output sink for golden tests; it faithfully
//! reproduces the register *map* and the DLAB banking, and documents every
//! behaviour it deliberately does not model yet.
//!
//! ## Deviations from the real chip (not yet modelled)
//! - No baud-rate modelling: the divisor latches (DLL/DLM) are stored but never
//!   affect timing. Transmission is instantaneous.
//! - No IRQs: [`Device::irq_pending`] is always `false` and no THRE/RX-driven
//!   interrupt is generated. On the real machine COM1 drives IRQ4; wiring that
//!   up comes later.
//! - No FIFOs: the FCR is stored but the RX/TX FIFOs are not modelled; the IIR
//!   always reports "no interrupt pending".
//! - No input source: the receiver holding register (RBR) always reads `0` and
//!   the LSR data-ready bit is always clear.
//! - **MCR loopback** is stored only: no internal TX-to-RX or modem-signal
//!   loopback (National Semiconductor NS16550A datasheet, Modem Control
//!   Register, bit 4). It needs the unmodelled receive/modem paths.
//! - **MSR** always reads zero: modem inputs and delta flags are absent
//!   (NS16550A datasheet, Modem Status Register); there is no modem endpoint.
//!
//! Transmitted bytes go directly to the optional sink; the UART retains no
//! output history. Without a sink, bytes are discarded without allocating.
//! Snapshots contain registers and the absolute trace time base; port wiring
//! and output sinks belong to the receiving instance. Snapshot v3 rejects the
//! earlier formats that included output history.
//! Transmission has no timed deadlines here (PC16550D datasheet, Transmitter
//! Holding Register). Restore does not emit historical bytes to a sink.
//! See `backlog/docs/design/observation.md`, §Sinks, bounds and lifecycle.

use device::{
    AccessDir, ByteOutput, ByteSink, ByteStreamKind, Device, DeviceError, NextEvent, TraceEvent,
    VClock,
};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "16550-uart";

/// Line Status Register value reported: THRE (`0x20`) and TEMT (`0x40`) are
/// always set because transmit is instantaneous, and the data-ready bit is clear
/// because there is no input source. Yields `0x60` on every read.
const LSR_TX_READY: u32 = 0x60;

/// IIR value reported: bit 0 set means "no interrupt pending"; the FIFO status
/// bits are `0` because FIFOs are not modelled.
const IIR_NO_IRQ: u32 = 0x01;

/// Modem Status Register value reported: no modem lines are modelled.
const MSR_IDLE: u32 = 0x00;

/// DLAB bit within the Line Control Register (bit 7): when set, offsets +0/+1
/// select the divisor-latch view instead of THR/RBR and IER.
const LCR_DLAB: u8 = 0x80;

/// Snapshot layout version tag, first byte of [`Device::save`] output.
const SNAPSHOT_VERSION: u8 = 3;

/// Fixed-size snapshot: version + 5 register bytes (IER, FCR, LCR, MCR,
/// scratch), DLL, DLM and absolute trace clock.
const SNAPSHOT_LEN: usize = 1 + 5 + 1 + 1 + 8;

/// The 16550 UART device (§7.2).
///
/// Constructed with the COM port base address; decodes the eight ports
/// `base..=base+7`. Accepted THR writes emit directly to a bound sink without
/// retaining a second history. Capture limits and drains belong to the host;
/// binding or draining a sink does not change snapshot bytes.
/// See `backlog/docs/design/observation.md`, §Sinks, bounds and lifecycle.
/// When enabled, the bounded trace records port accesses, including divisor-latch
/// writes, so it must not be treated as a transmitted byte stream: the National Semiconductor
/// PC16550D datasheet, Line Control Register (DLAB), assigns offsets +0/+1 to
/// the divisor latches when DLAB is set.
pub struct Uart16550 {
    base: u16,
    /// Interrupt Enable Register (+1, DLAB=0). Stored; raises no IRQ yet.
    ier: u8,
    /// FIFO Control Register (+2 write). Stored; FIFOs not modelled.
    fcr: u8,
    /// Line Control Register (+3). Bit 7 is DLAB.
    lcr: u8,
    /// Modem Control Register (+4). Stored only.
    mcr: u8,
    /// Scratch Register (+7). A byte of RAM; a device-presence probe target.
    scratch: u8,
    /// Divisor latch low (+0, DLAB=1). Stored; baud rate is not modelled.
    dll: u8,
    /// Divisor latch high (+1, DLAB=1). Stored; baud rate is not modelled.
    dlm: u8,
    /// Optional receiver; binding must not change saved state.
    sink: Option<Box<dyn ByteSink>>,
    /// Trace events observed since the last [`Device::take_trace`].
    trace: device::TraceBuffer,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

impl Uart16550 {
    /// Create a 16550 decoding the eight ports at `base..=base+7`.
    ///
    /// COM1 uses `base == 0x3F8`.
    ///
    /// # Panics
    /// Panics if `base > 0xFFF8`: eight registers must fit in the I/O space.
    /// This is a programmer-error guard; assembly validates user configuration
    /// before constructing the device.
    #[must_use]
    pub fn new(base: u16) -> Self {
        assert!(base <= 0xFFF8, "UART base must leave room for eight ports");
        Self {
            base,
            ier: 0,
            fcr: 0,
            lcr: 0,
            mcr: 0,
            scratch: 0,
            dll: 0,
            dlm: 0,
            sink: None,
            trace: device::TraceBuffer::default(),
            now: VClock::ZERO,
        }
    }

    /// Whether the DLAB bit is currently set in the LCR.
    fn dlab(&self) -> bool {
        self.lcr & LCR_DLAB != 0
    }

    /// Byte offset of `port` within this device's window, or an unmapped-port
    /// error if `port` is outside `base..=base+7`.
    fn offset(&self, port: u16) -> Result<u16, DeviceError> {
        if port < self.base || port > self.base + 7 {
            return Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port.wrapping_sub(self.base)),
            });
        }
        Ok(port - self.base)
    }

    /// Record a serviced port access on the tracing seam.
    fn trace_port(&mut self, port: u16, size: u8, dir: AccessDir, value: u32) {
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir,
            value,
        });
    }
}

impl ByteOutput for Uart16550 {
    fn kind(&self) -> ByteStreamKind {
        ByteStreamKind::SerialTx
    }

    fn bind_sink(&mut self, sink: Box<dyn ByteSink>) {
        self.sink = Some(sink);
    }
}

impl Device for Uart16550 {
    fn byte_output(&mut self) -> Option<&mut dyn ByteOutput> {
        Some(self)
    }

    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let offset = self.offset(port)?;
        let value = match offset {
            0 if self.dlab() => u32::from(self.dll),
            0 => 0, // RBR: no input source, so data-ready stays clear.
            1 if self.dlab() => u32::from(self.dlm),
            1 => u32::from(self.ier),
            2 => IIR_NO_IRQ,
            3 => u32::from(self.lcr),
            4 => u32::from(self.mcr),
            5 => LSR_TX_READY,
            6 => MSR_IDLE,
            7 => u32::from(self.scratch),
            _ => unreachable!("offset is bounded to 0..=7 by Self::offset"),
        };
        self.trace_port(port, size, AccessDir::Read, value);
        Ok(value)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        let offset = self.offset(port)?;
        // Registers are byte-wide; operate on the low byte for wider access.
        let byte = value as u8;
        match offset {
            0 if self.dlab() => self.dll = byte,
            0 => {
                if let Some(sink) = &mut self.sink {
                    sink.push(byte);
                }
            }
            1 if self.dlab() => self.dlm = byte,
            1 => self.ier = byte,
            2 => self.fcr = byte,
            3 => self.lcr = byte,
            4 => self.mcr = byte,
            5 | 6 => {} // LSR/MSR are read-only; writes are ignored on real hw.
            7 => self.scratch = byte,
            _ => unreachable!("offset is bounded to 0..=7 by Self::offset"),
        }
        self.trace_port(port, size, AccessDir::Write, value);
        Ok(())
    }

    fn mmio_read(&mut self, addr: u64, _size: u8) -> Result<u64, DeviceError> {
        // The 16550 is port-mapped on the PC/AT, never memory-mapped.
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
        self.now = now;
    }

    fn next_event(&self, _now: VClock) -> NextEvent {
        NextEvent::Dormant
    }

    fn irq_pending(&self) -> bool {
        // No THRE/RX interrupts are modelled yet; COM1's IRQ4 line comes later.
        false
    }

    fn irq_vector(&self) -> Option<u8> {
        None
    }

    fn reset(&mut self) {
        let trace_policy = self.trace.policy();
        // Capture epochs belong to the host; a hardware reset keeps the route.
        let base = self.base;
        let sink = self.sink.take();
        *self = Uart16550::new(base);
        self.sink = sink;
        self.trace.set_policy(trace_policy);
    }

    fn save(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(SNAPSHOT_LEN);
        blob.push(SNAPSHOT_VERSION);
        blob.push(self.ier);
        blob.push(self.fcr);
        blob.push(self.lcr);
        blob.push(self.mcr);
        blob.push(self.scratch);
        blob.push(self.dll);
        blob.push(self.dlm);
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        if snapshot.len() < SNAPSHOT_LEN {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "snapshot shorter than fixed layout",
            });
        }
        if snapshot[0] != SNAPSHOT_VERSION {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            });
        }
        let ier = snapshot[1];
        let fcr = snapshot[2];
        let lcr = snapshot[3];
        let mcr = snapshot[4];
        let scratch = snapshot[5];
        let dll = snapshot[6];
        let dlm = snapshot[7];
        if snapshot.len() != SNAPSHOT_LEN {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "snapshot has trailing bytes",
            });
        }
        self.ier = ier;
        self.fcr = fcr;
        self.lcr = lcr;
        self.mcr = mcr;
        self.scratch = scratch;
        self.dll = dll;
        self.dlm = dlm;
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| snapshot[8 + i])));
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

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    #[test]
    #[should_panic(expected = "UART base must leave room for eight ports")]
    fn fidelity_rejects_wrapping_port_window() {
        let _bus = MockBus::new(Box::new(Uart16550::new(0xFFF9)));
    }

    #[test]
    fn fidelity_last_valid_port_window_includes_scratch() {
        let mut bus = MockBus::new(Box::new(Uart16550::new(0xFFF8)));
        bus.port_write(0xFFFF, 1, 0x5A).unwrap();
        assert_eq!(bus.port_read(0xFFFF, 1).unwrap(), 0x5A);
    }

    const COM1: u16 = 0x3F8;

    fn capture_output(device: &mut dyn Device) -> device::ByteCapture {
        let capture = device::ByteCapture::new(16);
        device
            .byte_output()
            .unwrap()
            .bind_sink(Box::new(capture.clone()));
        capture
    }

    #[test]
    fn snapshot_is_independent_of_sink_binding_and_capture_history() {
        let mut plain = MockBus::new(Box::new(Uart16550::new(COM1)));
        let mut routed = MockBus::new(Box::new(Uart16550::new(COM1)));
        let capture = device::ByteCapture::new(3);
        routed
            .device_mut()
            .byte_output()
            .unwrap()
            .bind_sink(Box::new(capture.clone()));
        for bus in [&mut plain, &mut routed] {
            bus.port_write(COM1 + 3, 1, 0x80).unwrap();
            bus.port_write(COM1, 1, 12).unwrap();
            bus.port_write(COM1 + 1, 1, 1).unwrap();
            bus.port_write(COM1 + 3, 1, 3).unwrap();
            for byte in [0, 0xFF, 0xFF, 42] {
                bus.port_write(COM1, 1, byte).unwrap();
            }
        }
        let snapshot = plain.device().save();
        assert_eq!(routed.device().save(), snapshot);
        assert_eq!(snapshot[0], SNAPSHOT_VERSION);
        assert_eq!(snapshot.len(), SNAPSHOT_LEN);
        assert_eq!(capture.drain(), Err(device::ByteOverflow { limit: 3 }));
        assert_eq!(routed.device().save(), snapshot);
        capture.start_epoch();
        for bus in [&mut plain, &mut routed] {
            bus.device_mut().reset();
            bus.device_mut().restore(&snapshot).unwrap();
            assert_eq!(bus.device().save(), snapshot);
        }
        assert!(
            capture.drain().unwrap().is_empty(),
            "restore must not emit history"
        );
        for bus in [&mut plain, &mut routed] {
            bus.port_write(COM1, 1, 7).unwrap();
        }
        assert_eq!(capture.drain().unwrap(), [7]);
        assert_eq!(routed.device().save(), plain.device().save());
    }

    #[test]
    fn observation_sink_receives_only_accepted_thr_bytes_and_survives_reset() {
        let capture = device::ByteCapture::new(4);
        let mut bus = MockBus::new(Box::new(crate::trace_policy_tests::traced(Uart16550::new(
            COM1,
        ))));
        let output = bus.device_mut().byte_output().unwrap();
        assert_eq!(output.kind(), device::ByteStreamKind::SerialTx);
        output.bind_sink(Box::new(capture.clone()));
        bus.port_write(COM1, 2, 0x1241).unwrap();
        bus.port_write(COM1 + 3, 1, 0x80).unwrap();
        bus.port_write(COM1, 1, 0xFF).unwrap();
        bus.port_write(COM1 + 1, 1, 0x12).unwrap();
        bus.port_read(COM1, 1).unwrap();
        assert!(bus.port_write(COM1 - 1, 1, 0x42).is_err());
        let snapshot = bus.device().save();
        assert_eq!(snapshot.len(), SNAPSHOT_LEN);
        bus.device_mut().restore(&snapshot).unwrap();
        bus.device_mut().take_trace();
        assert_eq!(capture.drain().unwrap(), b"A");
        bus.device_mut().reset();
        for byte in [0, 0xFF, 0xFF] {
            bus.port_write(COM1, 1, byte).unwrap();
        }
        assert_eq!(capture.drain().unwrap(), [0, 0xFF, 0xFF]);
        assert!(capture.drain().unwrap().is_empty());
    }

    #[test]
    fn dormant_matches_per_clock_seeded_accesses_and_long_jumps() {
        let mut reference = MockBus::new(Box::new(crate::trace_policy_tests::traced(
            Uart16550::new(0x3F8),
        )));
        let mut scheduled = MockBus::new(Box::new(crate::trace_policy_tests::traced(
            Uart16550::new(0x3F8),
        )));
        let mut seed = 0x8042_1468_1816_5501u64;
        for step in 0..128 {
            assert_eq!(
                scheduled.device().next_event(reference.clock()),
                device::NextEvent::Dormant
            );
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let clocks = if step % 31 == 0 {
                100_003
            } else {
                (seed >> 32) % 17
            };
            for _ in 0..clocks {
                reference.tick(1);
                assert!(!reference.device().irq_pending());
            }
            scheduled.device_mut().sync(reference.clock());
            assert_eq!(reference.device().save(), scheduled.device().save());
            assert_eq!(
                reference.device().irq_pending(),
                scheduled.device().irq_pending()
            );
            let port = 0x3F8 + ((seed >> 8) % 8) as u16;
            if seed & 1 == 0 {
                reference.port_write(port, 1, seed as u32).unwrap();
                scheduled.port_write(port, 1, seed as u32).unwrap();
            } else {
                assert_eq!(
                    reference.port_read(port, 1).unwrap(),
                    scheduled.port_read(port, 1).unwrap()
                );
            }
            assert_eq!(reference.device().save(), scheduled.device().save());
            assert_eq!(
                reference.device().irq_pending(),
                scheduled.device().irq_pending()
            );
            assert_eq!(
                reference.device_mut().take_trace(),
                scheduled.device_mut().take_trace()
            );
        }
    }

    #[test]
    fn thr_write_emits_to_sink() {
        let mut uart = Uart16550::new(COM1);
        let capture = capture_output(&mut uart);
        uart.port_write(COM1, 1, u32::from(b'A')).unwrap();
        uart.port_write(COM1, 1, u32::from(b'B')).unwrap();
        assert_eq!(capture.drain().unwrap(), b"AB");
        assert!(capture.drain().unwrap().is_empty());
    }

    #[test]
    fn lsr_reports_thr_empty_no_data() {
        let mut uart = Uart16550::new(COM1);
        assert_eq!(uart.port_read(COM1 + 5, 1).unwrap(), 0x60);
    }

    #[test]
    fn dlab_banks_divisor_latch_then_restores_thr() {
        let mut uart = Uart16550::new(COM1);
        let capture = capture_output(&mut uart);
        // Set DLAB (LCR bit 7) at +3.
        uart.port_write(COM1 + 3, 1, u32::from(LCR_DLAB)).unwrap();
        // With DLAB set, +0/+1 are DLL/DLM.
        uart.port_write(COM1, 1, 0x0C).unwrap();
        uart.port_write(COM1 + 1, 1, 0x01).unwrap();
        assert_eq!(uart.port_read(COM1, 1).unwrap(), 0x0C);
        assert_eq!(uart.port_read(COM1 + 1, 1).unwrap(), 0x01);
        // Clear DLAB: +0 is THR again, so a write transmits rather than banking.
        uart.port_write(COM1 + 3, 1, 0).unwrap();
        uart.port_write(COM1, 1, u32::from(b'Z')).unwrap();
        assert_eq!(capture.drain().unwrap(), b"Z");
        // Divisor latch retained its value.
        assert_eq!(uart.port_read(COM1 + 3, 1).unwrap(), 0);
    }

    #[test]
    fn mock_bus_output_excludes_divisor_writes() {
        let mut bus = MockBus::new(Box::new(crate::trace_policy_tests::traced(Uart16550::new(
            COM1,
        ))));
        let capture = capture_output(bus.device_mut());
        bus.port_write(COM1 + 3, 1, 0x83).unwrap();
        bus.port_write(COM1, 1, 0x42).unwrap();
        bus.port_write(COM1 + 1, 1, 0x01).unwrap();
        assert_eq!(bus.port_read(COM1, 1).unwrap(), 0x42);
        assert_eq!(bus.port_read(COM1 + 1, 1).unwrap(), 0x01);
        bus.port_write(COM1 + 3, 1, 0x03).unwrap();
        for &byte in b"OK\0\xff" {
            bus.port_write(COM1, 1, u32::from(byte)).unwrap();
        }

        assert_eq!(capture.drain().unwrap(), b"OK\0\xff");
        assert!(capture.drain().unwrap().is_empty());
        assert!(bus.device_mut().take_trace().iter().any(|event| matches!(
            event,
            TraceEvent::Port {
                port: COM1,
                dir: AccessDir::Write,
                value: 0x42,
                ..
            }
        )));
    }

    #[test]
    fn scratch_register_round_trips() {
        let mut uart = Uart16550::new(COM1);
        uart.port_write(COM1 + 7, 1, 0x5A).unwrap();
        assert_eq!(uart.port_read(COM1 + 7, 1).unwrap(), 0x5A);
    }

    #[test]
    fn save_restore_round_trip_preserves_state() {
        let mut uart = Uart16550::new(COM1);
        let capture = capture_output(&mut uart);
        uart.port_write(COM1 + 3, 1, u32::from(LCR_DLAB)).unwrap();
        uart.port_write(COM1, 1, 0x03).unwrap(); // DLL
        uart.port_write(COM1 + 1, 1, 0x00).unwrap(); // DLM
        uart.port_write(COM1 + 3, 1, 0).unwrap();
        uart.port_write(COM1 + 4, 1, 0x0B).unwrap(); // MCR
        uart.port_write(COM1 + 7, 1, 0x7E).unwrap(); // scratch
        uart.port_write(COM1, 1, u32::from(b'h')).unwrap();
        uart.port_write(COM1, 1, u32::from(b'i')).unwrap();

        let blob = uart.save();
        let mut restored = Uart16550::new(COM1);
        let restored_capture = capture_output(&mut restored);
        restored.restore(&blob).unwrap();

        assert_eq!(capture.drain().unwrap(), b"hi");
        assert!(restored_capture.drain().unwrap().is_empty());
        assert_eq!(restored.save(), blob);
        restored.port_write(COM1, 1, u32::from(b'!')).unwrap();
        assert_eq!(restored_capture.drain().unwrap(), b"!");
        assert_eq!(restored.port_read(COM1 + 4, 1).unwrap(), 0x0B);
        assert_eq!(restored.port_read(COM1 + 7, 1).unwrap(), 0x7E);
        // DLL survives (read it back through the DLAB view).
        restored
            .port_write(COM1 + 3, 1, u32::from(LCR_DLAB))
            .unwrap();
        assert_eq!(restored.port_read(COM1, 1).unwrap(), 0x03);
    }

    #[test]
    fn restore_truncated_snapshot_is_rejected() {
        let mut uart = Uart16550::new(COM1);
        let err = uart.restore(&[SNAPSHOT_VERSION, 0, 0]).unwrap_err();
        assert_eq!(
            err,
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "snapshot shorter than fixed layout",
            }
        );
    }

    #[test]
    fn mmio_is_unmapped() {
        let mut uart = Uart16550::new(COM1);
        assert_eq!(
            uart.mmio_read(0x1000, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: 0x1000,
            }
        );
        assert_eq!(
            uart.mmio_write(0x1000, 1, 0).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: 0x1000,
            }
        );
    }

    #[test]
    fn port_outside_window_is_unmapped() {
        let mut uart = Uart16550::new(COM1);
        let err = uart.port_read(COM1 + 8, 1).unwrap_err();
        assert_eq!(
            err,
            DeviceError::UnmappedRegister {
                kind: "port",
                offset: 8,
            }
        );
    }

    #[test]
    fn no_irq_is_asserted() {
        let uart = Uart16550::new(COM1);
        assert!(!uart.irq_pending());
        assert_eq!(uart.irq_vector(), None);
    }

    #[test]
    fn reset_clears_registers_and_preserves_host_output() {
        let mut uart = Uart16550::new(COM1);
        let capture = capture_output(&mut uart);
        uart.port_write(COM1, 1, u32::from(b'x')).unwrap();
        uart.port_write(COM1 + 7, 1, 0x42).unwrap();
        uart.reset();
        assert_eq!(capture.drain().unwrap(), b"x");
        assert_eq!(uart.port_read(COM1 + 7, 1).unwrap(), 0);
    }

    #[test]
    fn hello_via_output_and_trace() {
        let mut uart = crate::trace_policy_tests::traced(Uart16550::new(COM1));
        let capture = capture_output(&mut uart);
        for &byte in b"hello" {
            uart.port_write(COM1, 1, u32::from(byte)).unwrap();
        }
        assert_eq!(capture.drain().unwrap(), b"hello");

        // The tracing seam must yield one Port write to `base` per THR byte.
        let writes: Vec<u32> = uart
            .take_trace()
            .into_iter()
            .filter_map(|ev| match ev {
                TraceEvent::Port {
                    port,
                    dir: AccessDir::Write,
                    value,
                    ..
                } if port == COM1 => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(
            writes,
            b"hello".iter().map(|&b| u32::from(b)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mock_bus_isolation_harness() {
        // Prove the device runs under the isolation harness and the trace seam
        // survives a round trip through `&mut dyn Device`.
        let mut bus = MockBus::new(Box::new(crate::trace_policy_tests::traced(Uart16550::new(
            COM1,
        ))));
        bus.tick(3); // stamp a non-zero virtual time onto subsequent traces.
        for &byte in b"hello" {
            bus.port_write(COM1, 1, u32::from(byte)).unwrap();
        }
        assert_eq!(bus.port_read(COM1 + 5, 1).unwrap(), 0x60);

        let reconstructed: Vec<u8> = bus
            .device_mut()
            .take_trace()
            .into_iter()
            .filter_map(|ev| match ev {
                TraceEvent::Port {
                    port,
                    dir: AccessDir::Write,
                    value,
                    at,
                    ..
                } if port == COM1 => {
                    assert_eq!(at, VClock(3));
                    Some(value as u8)
                }
                _ => None,
            })
            .collect();
        assert_eq!(reconstructed, b"hello");
    }
}
