// SPDX-License-Identifier: AGPL-3.0-or-later
//! Intel 8237A programmable DMA controller (§7.7).
//!
//! Models the first of the PC/AT's two 8237s — the byte-wide channels 0–3 at
//! ports `0x00..=0x0F`, plus the external page-register latches at `0x80..=0x8F`
//! that supply address bits 16–23. Each channel owns a 16-bit current/base
//! address and a 16-bit current/base word count; the chip owns a command,
//! status, request and mask register, a per-channel mode register, and the
//! first/last **flip-flop** that sequences the low/high byte of the 16-bit
//! channel registers.
//!
//! ## SMC-safe memory access
//!
//! A DMA transfer moves bytes between an I/O device and **guest RAM**. Guest RAM
//! must only be written through the path that runs the JIT's `note_write` hook,
//! or translated code stops being invalidated when DMA overwrites it (see the
//! memory model in `backlog/docs/architecture.md`, §4.3). This device therefore
//! never holds a reference into guest memory and never writes a raw buffer:
//! [`Dma8237::service_write`] / [`Dma8237::service_read`] take a
//! [`DmaMemory`] **handle** per call, and the real machine implements that trait
//! over the `Vm` API so every byte goes through `note_write`. Unit tests
//! implement the same trait over a plain buffer.
//!
//! ## Coverage matrix (datasheet vs modelled)
//!
//! | Feature                                   | Modelled | Notes                       |
//! |-------------------------------------------|----------|-----------------------------|
//! | Channels 0–3 address/count (base+current) | yes      | 16-bit, flip-flop sequenced |
//! | Page registers (bits 16–23)               | yes      | latched at `0x80..=0x8F`     |
//! | Mode register (type/dir/autoinit/mode)    | yes      | stored + decoded            |
//! | Command / status / request / mask         | yes      | status TC + request bits    |
//! | Flip-flop + master clear                   | yes      | `0x0C` / `0x0D`             |
//! | Write / read / verify transfers            | yes      | per mode bits 3-2           |
//! | Address increment / decrement + autoinit   | yes      | per mode bits 5 / 4         |
//! | Terminal count → TC status + autoinit/mask | yes      |                             |
//!
//! Deliberately **not** modelled:
//! - **The second 8237 (channels 4–7) and the cascade** (channel 4 cascades the
//!   slave controller into the master). This device is a single 8237; a machine
//!   that needs 16-bit DMA instantiates a second one. The cascade *mode* value is
//!   accepted and stored but performs no transfer.
//! - **Memory-to-memory transfers** (command bit 0, using channels 0/1): the bit
//!   is stored, but the block-copy path is not implemented.
//! - **Timing**: demand, single, and block transfer *modes* all move bytes
//!   through the same [`Dma8237::service_write`]/[`service_read`] primitives; the
//!   controller models the register effects, not per-cycle bus arbitration or the
//!   DREQ/DACK handshake latency.
//! - **Compressed timing / rotating priority / the temporary register readback**
//!   beyond returning the last byte.
//! - **Mode/direction mismatch**: the requested transfer direction
//!   ([`service_write`] for device→memory, [`service_read`] for memory→device) is
//!   trusted to match the channel's programmed [`TransferType`]. A guest that
//!   programs a write-transfer mode but drives a read-direction request (or vice
//!   versa) is not detected — parity with the other simplifications above, which
//!   model register effects rather than policing guest misconfiguration.

use device::{
    AccessDir, Device, DeviceError, DmaMemory, DmaTransfer, NextEvent, TraceEvent, VClock,
};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "8237-dma";

/// First controller-register port.
const REG_BASE: u16 = 0x00;
/// Last controller-register port.
const REG_END: u16 = 0x0F;
/// First page-latch port.
const PAGE_BASE: u16 = 0x80;
/// Last page-latch port.
const PAGE_END: u16 = 0x8F;

/// Number of DMA channels in one 8237.
const CHANNELS: usize = 4;

/// Number of page latches decoded at `0x80..=0x8F`.
const PAGE_LATCHES: usize = 16;

/// Snapshot layout version tag.
const SNAPSHOT_VERSION: u8 = 1;

/// Page-latch offset (within `0x80..=0x8F`) that supplies each channel's page:
/// channel 0 → `0x87`, 1 → `0x83`, 2 → `0x81`, 3 → `0x82`.
const CHANNEL_PAGE_OFFSET: [usize; CHANNELS] = [7, 3, 1, 2];

/// Command-register bit 2: controller disable (1 = the whole controller is off).
const CMD_DISABLE: u8 = 1 << 2;

/// Mode-register bit 4: autoinitialize (reload base→current at terminal count).
const MODE_AUTOINIT: u8 = 1 << 4;
/// Mode-register bit 5: address decrement (else increment).
const MODE_DECREMENT: u8 = 1 << 5;

/// The transfer type a channel's mode register selects (bits 3-2).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TransferType {
    /// Verify: address/count step with no bus transfer.
    Verify,
    /// Write transfer: the I/O device writes to memory (memory is written).
    Write,
    /// Read transfer: memory is read out to the I/O device.
    Read,
    /// Illegal encoding (bits 3-2 == `11`).
    Illegal,
}

impl TransferType {
    fn from_mode(mode: u8) -> TransferType {
        match (mode >> 2) & 0b11 {
            0 => TransferType::Verify,
            1 => TransferType::Write,
            2 => TransferType::Read,
            _ => TransferType::Illegal,
        }
    }
}

/// One DMA channel's address and count registers.
#[derive(Copy, Clone, Debug, Default)]
struct Channel {
    current_address: u16,
    base_address: u16,
    current_count: u16,
    base_count: u16,
    /// The channel's mode register byte (bits 1-0 are the channel select echo).
    mode: u8,
}

/// The 8237A DMA controller (§7.7).
///
/// Decodes `0x00..=0x0F` and `0x80..=0x8F`. Has no interrupt output — DMA does
/// not drive an IRQ line — so [`Device::irq_pending`] is always `false` and
/// [`Device::irq_vector`] is `None`.
pub struct Dma8237 {
    channels: [Channel; CHANNELS],
    /// External page latches at `0x80..=0x8F`.
    pages: [u8; PAGE_LATCHES],
    /// Command register (`0x08`).
    command: u8,
    /// Status register (`0x08` read): bits 0-3 TC, bits 4-7 request.
    status: u8,
    /// Software request register (`0x09`), reflected in status bits 4-7.
    request: u8,
    /// Mask register: bit per channel, 1 = masked (DREQ ignored).
    mask: u8,
    /// First/last flip-flop: `false` = next byte is low, `true` = high.
    flip_flop: bool,
    /// Temporary register (`0x0D` read).
    temp: u8,
    /// Trace events observed since the last [`Device::take_trace`].
    trace: device::TraceBuffer,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

impl Dma8237 {
    /// Create an 8237 in power-on state (all channels masked, registers cleared).
    #[must_use]
    pub fn new() -> Self {
        Self {
            channels: [Channel::default(); CHANNELS],
            pages: [0; PAGE_LATCHES],
            command: 0,
            status: 0,
            request: 0,
            // Master-clear leaves every channel masked until firmware unmasks it.
            mask: 0x0F,
            flip_flop: false,
            temp: 0,
            trace: device::TraceBuffer::default(),
            now: VClock::ZERO,
        }
    }

    /// The 24-bit physical address a channel currently points at.
    #[must_use]
    pub fn physical_address(&self, channel: usize) -> u32 {
        let page = u32::from(self.pages[CHANNEL_PAGE_OFFSET[channel]]);
        (page << 16) | u32::from(self.channels[channel].current_address)
    }

    /// The transfer type programmed for `channel`.
    #[must_use]
    pub fn transfer_type(&self, channel: usize) -> TransferType {
        TransferType::from_mode(self.channels[channel].mode)
    }

    /// The live word count remaining on `channel`.
    #[must_use]
    pub fn current_count(&self, channel: usize) -> u16 {
        self.channels[channel].current_count
    }

    /// Whether `channel` can currently be serviced (unmasked and controller on).
    #[must_use]
    pub fn channel_ready(&self, channel: usize) -> bool {
        self.command & CMD_DISABLE == 0 && self.mask & (1 << channel) == 0
    }

    /// Perform a device→memory ("write") transfer of `data` on `channel`,
    /// writing each byte through `mem` and stepping the channel address/count.
    /// Stops at terminal count or when `data` is exhausted. Returns the number of
    /// bytes transferred and whether TC was asserted.
    /// Intel 8237A datasheet, Current Word Count Register and End of Process.
    ///
    /// The caller selects this method when the channel's [`TransferType`] is
    /// `Write`; `Verify` steps the channel without touching memory.
    pub fn service_write(
        &mut self,
        channel: usize,
        mem: &mut dyn DmaMemory,
        data: &[u8],
    ) -> DmaTransfer {
        let verify = self.transfer_type(channel) == TransferType::Verify;
        let mut result = DmaTransfer::default();
        for &byte in data {
            if !self.channel_ready(channel) {
                break;
            }
            if !verify {
                let addr = self.physical_address(channel);
                mem.dma_write(addr, byte);
            }
            result.transferred += 1;
            if self.advance_channel(channel) {
                result.terminal_count = true;
                break;
            }
        }
        result
    }

    /// Perform a memory→device ("read") transfer on `channel`, filling `out` with
    /// bytes read through `mem` and stepping the channel. Stops at terminal count
    /// or when `out` is full. Reports the byte count and whether TC was asserted.
    /// Intel 8237A datasheet, Current Word Count Register and End of Process.
    pub fn service_read(
        &mut self,
        channel: usize,
        mem: &mut dyn DmaMemory,
        out: &mut [u8],
    ) -> DmaTransfer {
        let mut result = DmaTransfer::default();
        for slot in out.iter_mut() {
            if !self.channel_ready(channel) {
                break;
            }
            let addr = self.physical_address(channel);
            *slot = mem.dma_read(addr);
            result.transferred += 1;
            if self.advance_channel(channel) {
                result.terminal_count = true;
                break;
            }
        }
        result
    }

    /// Step a channel's address and count after one byte. Returns `true` when
    /// terminal count is reached (TC flagged; autoinit reload or self-mask).
    /// Intel 8237A datasheet, Current Word Count Register: TC is the count's
    /// underflow to 0xFFFF, which remains readable unless autoinitialize reloads it.
    fn advance_channel(&mut self, channel: usize) -> bool {
        let decrement = self.channels[channel].mode & MODE_DECREMENT != 0;
        let autoinit = self.channels[channel].mode & MODE_AUTOINIT != 0;
        {
            let ch = &mut self.channels[channel];
            ch.current_address = if decrement {
                ch.current_address.wrapping_sub(1)
            } else {
                ch.current_address.wrapping_add(1)
            };
            ch.current_count = ch.current_count.wrapping_sub(1);
        }
        // The 8237 transfers count+1 bytes: terminal count is the underflow from 0.
        if self.channels[channel].current_count == 0xFFFF {
            self.status |= 1 << channel; // TC flag for this channel
            if autoinit {
                let ch = &mut self.channels[channel];
                ch.current_address = ch.base_address;
                ch.current_count = ch.base_count;
            } else {
                self.mask |= 1 << channel;
            }
            true
        } else {
            false
        }
    }

    /// Byte written to a channel's address register, sequenced by the flip-flop.
    fn write_address(&mut self, channel: usize, byte: u8) {
        let ch = &mut self.channels[channel];
        if self.flip_flop {
            ch.base_address = (ch.base_address & 0x00FF) | (u16::from(byte) << 8);
            ch.current_address = (ch.current_address & 0x00FF) | (u16::from(byte) << 8);
        } else {
            ch.base_address = (ch.base_address & 0xFF00) | u16::from(byte);
            ch.current_address = (ch.current_address & 0xFF00) | u16::from(byte);
        }
        self.flip_flop = !self.flip_flop;
    }

    /// Byte written to a channel's count register, sequenced by the flip-flop.
    fn write_count(&mut self, channel: usize, byte: u8) {
        let ch = &mut self.channels[channel];
        if self.flip_flop {
            ch.base_count = (ch.base_count & 0x00FF) | (u16::from(byte) << 8);
            ch.current_count = (ch.current_count & 0x00FF) | (u16::from(byte) << 8);
        } else {
            ch.base_count = (ch.base_count & 0xFF00) | u16::from(byte);
            ch.current_count = (ch.current_count & 0xFF00) | u16::from(byte);
        }
        self.flip_flop = !self.flip_flop;
    }

    /// Byte read from a channel's address register, sequenced by the flip-flop.
    fn read_address(&mut self, channel: usize) -> u8 {
        let addr = self.channels[channel].current_address;
        let byte = if self.flip_flop {
            (addr >> 8) as u8
        } else {
            addr as u8
        };
        self.flip_flop = !self.flip_flop;
        byte
    }

    /// Byte read from a channel's count register, sequenced by the flip-flop.
    fn read_count(&mut self, channel: usize) -> u8 {
        let count = self.channels[channel].current_count;
        let byte = if self.flip_flop {
            (count >> 8) as u8
        } else {
            count as u8
        };
        self.flip_flop = !self.flip_flop;
        byte
    }

    /// Read the status register, clearing the TC flags as a side effect.
    fn read_status(&mut self) -> u8 {
        let value = self.status | (self.request << 4);
        self.status &= 0xF0; // reading clears the TC (low) bits
        value
    }

    /// Apply a single-channel mask write (`0x0A`).
    fn write_single_mask(&mut self, byte: u8) {
        let channel = usize::from(byte & 0b11);
        if byte & 0b100 != 0 {
            self.mask |= 1 << channel;
        } else {
            self.mask &= !(1 << channel);
        }
    }

    /// Apply a request-register write (`0x09`).
    fn write_request(&mut self, byte: u8) {
        let channel = byte & 0b11;
        if byte & 0b100 != 0 {
            self.request |= 1 << channel;
        } else {
            self.request &= !(1 << channel);
        }
    }

    /// Master clear (`0x0D` write): reset control state, mask every channel.
    fn master_clear(&mut self) {
        self.command = 0;
        self.status = 0;
        self.request = 0;
        self.temp = 0;
        self.flip_flop = false;
        self.mask = 0x0F;
    }

    /// Byte offset of a page-latch port within `0x80..=0x8F`.
    fn page_offset(port: u16) -> usize {
        usize::from(port - PAGE_BASE)
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

impl Default for Dma8237 {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for Dma8237 {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let value = match port {
            REG_BASE..=REG_END => {
                let off = port - REG_BASE;
                match off {
                    0x0 | 0x2 | 0x4 | 0x6 => u32::from(self.read_address(usize::from(off / 2))),
                    0x1 | 0x3 | 0x5 | 0x7 => u32::from(self.read_count(usize::from(off / 2))),
                    0x8 => u32::from(self.read_status()),
                    0xD => u32::from(self.temp),
                    // Command/request/mask/mode/clear ports are write-only.
                    _ => 0,
                }
            }
            PAGE_BASE..=PAGE_END => u32::from(self.pages[Self::page_offset(port)]),
            _ => {
                return Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(port),
                })
            }
        };
        self.trace_port(port, size, AccessDir::Read, value);
        Ok(value)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        let byte = value as u8;
        match port {
            REG_BASE..=REG_END => {
                let off = port - REG_BASE;
                match off {
                    0x0 | 0x2 | 0x4 | 0x6 => self.write_address(usize::from(off / 2), byte),
                    0x1 | 0x3 | 0x5 | 0x7 => self.write_count(usize::from(off / 2), byte),
                    0x8 => self.command = byte,
                    0x9 => self.write_request(byte),
                    0xA => self.write_single_mask(byte),
                    0xB => {
                        let channel = usize::from(byte & 0b11);
                        self.channels[channel].mode = byte;
                    }
                    0xC => self.flip_flop = false,
                    0xD => self.master_clear(),
                    0xE => self.mask = 0, // clear-mask: unmask all channels
                    0xF => self.mask = byte & 0x0F,
                    _ => unreachable!("offset bounded to 0..=0xF"),
                }
            }
            PAGE_BASE..=PAGE_END => self.pages[Self::page_offset(port)] = byte,
            _ => {
                return Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(port),
                })
            }
        }
        self.trace_port(port, size, AccessDir::Write, value);
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
        // The controller has no autonomous timed behaviour; transfers are driven
        // explicitly through service_write / service_read.
        self.now = now;
    }

    /// Intel 8237A datasheet, Single Transfer Mode: channel state advances on
    /// explicit DMA service. Bus arbitration timing is outside this model.
    fn next_event(&self, _: VClock) -> NextEvent {
        NextEvent::Dormant
    }

    fn irq_pending(&self) -> bool {
        // The 8237 has no interrupt output; devices that use DMA raise their own
        // IRQ (e.g. the floppy controller).
        false
    }

    fn irq_vector(&self) -> Option<u8> {
        None
    }

    fn reset(&mut self) {
        let trace_policy = self.trace.policy();
        *self = Dma8237::new();
        self.trace.set_policy(trace_policy);
    }

    fn save(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(1 + CHANNELS * 9 + PAGE_LATCHES + 5);
        blob.push(SNAPSHOT_VERSION);
        for ch in &self.channels {
            blob.extend_from_slice(&ch.current_address.to_le_bytes());
            blob.extend_from_slice(&ch.base_address.to_le_bytes());
            blob.extend_from_slice(&ch.current_count.to_le_bytes());
            blob.extend_from_slice(&ch.base_count.to_le_bytes());
            blob.push(ch.mode);
        }
        blob.extend_from_slice(&self.pages);
        blob.push(self.command);
        blob.push(self.status);
        blob.push(self.request);
        blob.push(self.mask);
        blob.push(u8::from(self.flip_flop));
        blob.push(self.temp);
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        const LEN: usize = 1 + CHANNELS * 9 + PAGE_LATCHES + 6;
        if snapshot.len() != LEN {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unexpected snapshot length",
            });
        }
        if snapshot[0] != SNAPSHOT_VERSION {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            });
        }
        let mut off = 1;
        let rd16 = |o: usize| u16::from_le_bytes([snapshot[o], snapshot[o + 1]]);
        for ch in &mut self.channels {
            ch.current_address = rd16(off);
            ch.base_address = rd16(off + 2);
            ch.current_count = rd16(off + 4);
            ch.base_count = rd16(off + 6);
            ch.mode = snapshot[off + 8];
            off += 9;
        }
        self.pages
            .copy_from_slice(&snapshot[off..off + PAGE_LATCHES]);
        off += PAGE_LATCHES;
        self.command = snapshot[off];
        self.status = snapshot[off + 1];
        self.request = snapshot[off + 2];
        self.mask = snapshot[off + 3];
        self.flip_flop = snapshot[off + 4] != 0;
        self.temp = snapshot[off + 5];
        Ok(())
    }

    fn dma_service_write(
        &mut self,
        channel: u8,
        mem: &mut dyn DmaMemory,
        data: &[u8],
    ) -> DmaTransfer {
        self.service_write(usize::from(channel), mem, data)
    }

    fn dma_service_read(
        &mut self,
        channel: u8,
        mem: &mut dyn DmaMemory,
        out: &mut [u8],
    ) -> DmaTransfer {
        self.service_read(usize::from(channel), mem, out)
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
    use std::collections::HashMap;
    use testkit::MockBus;

    /// Intel 8237A datasheet, Single Transfer Mode and Autoinitialization:
    /// channel registers advance on service, not on elapsed PIT clocks.
    #[test]
    fn dormant_dma_matches_per_clock_registers_memory_and_traces() {
        for mode in [0x04, 0x14, 0x24] {
            let mut reference = crate::trace_policy_tests::traced(Dma8237::new());
            let mut scheduled = crate::trace_policy_tests::traced(Dma8237::new());
            program(&mut reference, 2, mode, 1, 0x1234, 3);
            program(&mut scheduled, 2, mode, 1, 0x1234, 3);
            let mut reference = MockBus::new(Box::new(reference));
            let mut scheduled = MockBus::new(Box::new(scheduled));
            let mut reference_mem = MockMem::default();
            let mut scheduled_mem = MockMem::default();
            for target in [1, 31, 1000, 65537] {
                while reference.clock().0 < target {
                    reference.tick(1);
                }
                assert_eq!(
                    scheduled.device().next_event(VClock(target)),
                    device::NextEvent::Dormant
                );
                scheduled.device_mut().sync(VClock(target));
                let data = [1, 2, 3, 4];
                assert_eq!(
                    reference
                        .device_mut()
                        .dma_service_write(2, &mut reference_mem, &data),
                    scheduled
                        .device_mut()
                        .dma_service_write(2, &mut scheduled_mem, &data)
                );
                assert_eq!(
                    reference.port_read(8, 1).unwrap(),
                    scheduled.port_read(8, 1).unwrap()
                );
                assert_eq!(reference_mem.cells, scheduled_mem.cells);
                assert_eq!(reference_mem.writes, scheduled_mem.writes);
                assert_eq!(reference.device().save(), scheduled.device().save());
                assert_eq!(
                    reference.device_mut().take_trace(),
                    scheduled.device_mut().take_trace()
                );
            }
        }
    }

    /// A sparse mock guest memory implementing the SMC-safe write seam. Records
    /// every write so a test can prove DMA went through the handle, not a raw
    /// buffer.
    #[derive(Default)]
    struct MockMem {
        cells: HashMap<u32, u8>,
        writes: Vec<(u32, u8)>,
    }

    impl MockMem {
        fn seed(&mut self, addr: u32, bytes: &[u8]) {
            for (i, &b) in bytes.iter().enumerate() {
                self.cells.insert(addr + i as u32, b);
            }
        }
    }

    impl DmaMemory for MockMem {
        fn dma_read(&self, addr: u32) -> u8 {
            *self.cells.get(&addr).unwrap_or(&0)
        }

        fn dma_write(&mut self, addr: u32, value: u8) {
            self.cells.insert(addr, value);
            self.writes.push((addr, value));
        }
    }

    /// Program `channel` for a transfer: mode byte, page, address, count.
    fn program(dma: &mut Dma8237, channel: usize, mode: u8, page: u8, address: u16, count: u16) {
        let addr_port = (channel as u16) * 2;
        let count_port = addr_port + 1;
        // Mode register.
        dma.port_write(0x0B, 1, u32::from(mode | channel as u8))
            .unwrap();
        // Page latch.
        let page_port = PAGE_BASE + CHANNEL_PAGE_OFFSET[channel] as u16;
        dma.port_write(page_port, 1, u32::from(page)).unwrap();
        // Clear flip-flop, then low/high address and count.
        dma.port_write(0x0C, 1, 0).unwrap();
        dma.port_write(addr_port, 1, u32::from(address & 0xFF))
            .unwrap();
        dma.port_write(addr_port, 1, u32::from(address >> 8))
            .unwrap();
        dma.port_write(count_port, 1, u32::from(count & 0xFF))
            .unwrap();
        dma.port_write(count_port, 1, u32::from(count >> 8))
            .unwrap();
        // Unmask the channel.
        dma.port_write(0x0A, 1, u32::from(channel as u8)).unwrap();
    }

    #[test]
    fn power_on_masks_all_channels() {
        let dma = Dma8237::new();
        for ch in 0..CHANNELS {
            assert!(!dma.channel_ready(ch));
        }
        assert!(!dma.irq_pending());
        assert_eq!(dma.irq_vector(), None);
    }

    #[test]
    fn flip_flop_sequences_16bit_address() {
        let mut dma = Dma8237::new();
        dma.port_write(0x0C, 1, 0).unwrap(); // clear flip-flop
        dma.port_write(0x00, 1, 0x34).unwrap(); // low
        dma.port_write(0x00, 1, 0x12).unwrap(); // high
        dma.port_write(0x87, 1, 0x0A).unwrap(); // channel 0 page
        assert_eq!(dma.physical_address(0), 0x0A1234);
        // Read back through the flip-flop.
        dma.port_write(0x0C, 1, 0).unwrap();
        assert_eq!(dma.port_read(0x00, 1).unwrap(), 0x34);
        assert_eq!(dma.port_read(0x00, 1).unwrap(), 0x12);
    }

    #[test]
    fn clear_flip_flop_resets_sequence() {
        let mut dma = Dma8237::new();
        dma.port_write(0x00, 1, 0xAA).unwrap(); // consumes low half
        dma.port_write(0x0C, 1, 0).unwrap(); // reset flip-flop
        dma.port_write(0x00, 1, 0x11).unwrap(); // low again
        dma.port_write(0x00, 1, 0x22).unwrap(); // high
        assert_eq!(dma.physical_address(0) & 0xFFFF, 0x2211);
    }

    #[test]
    fn service_reports_tc_for_this_call_in_single_block_and_autoinit_modes() {
        for mode in [0x40, 0x80] {
            for autoinit in [0, MODE_AUTOINIT] {
                for from_memory in [false, true] {
                    let mut dma = Dma8237::new();
                    let mut mem = MockMem::default();
                    let direction = if from_memory { 0b1000 } else { 0b0100 };
                    program(&mut dma, 2, mode | autoinit | direction, 0, 0, 2);
                    let mut bus = MockBus::new(Box::new(dma));
                    let mut service = |bus: &mut MockBus, len| {
                        if from_memory {
                            bus.device_mut()
                                .dma_service_read(2, &mut mem, &mut vec![0; len])
                        } else {
                            bus.device_mut()
                                .dma_service_write(2, &mut mem, &vec![0x5A; len])
                        }
                    };
                    assert_eq!(service(&mut bus, 0), DmaTransfer::default());
                    assert_eq!(
                        service(&mut bus, 1),
                        DmaTransfer {
                            transferred: 1,
                            terminal_count: false
                        }
                    );
                    assert_eq!(
                        service(&mut bus, 4),
                        DmaTransfer {
                            transferred: 2,
                            terminal_count: true
                        }
                    );
                    // An unread TC status bit from the previous service is not
                    // a new TC pulse, even after autoinitialize reloads the count.
                    assert_eq!(
                        service(&mut bus, 1),
                        DmaTransfer {
                            transferred: usize::from(autoinit != 0),
                            terminal_count: false
                        }
                    );
                    assert_eq!(bus.port_read(0x08, 1).unwrap() & 4, 4);
                    assert_eq!(bus.port_read(0x08, 1).unwrap() & 4, 0);
                    if autoinit != 0 {
                        assert_eq!(
                            service(&mut bus, 2),
                            DmaTransfer {
                                transferred: 2,
                                terminal_count: true
                            }
                        );
                    }
                }
            }
        }
    }

    fn read_count_through_bus(bus: &mut MockBus, channel: u16) -> u16 {
        bus.port_write(0x0C, 1, 0).unwrap();
        let port = channel * 2 + 1;
        let low = bus.port_read(port, 1).unwrap() as u8;
        let high = bus.port_read(port, 1).unwrap() as u8;
        u16::from_le_bytes([low, high])
    }

    fn assert_count_readback_at_terminal_count(mode: u8) {
        for from_memory in [false, true] {
            for count in [0, 0x0102] {
                let mut dma = Dma8237::new();
                let mut mem = MockMem::default();
                let direction = if from_memory { 0b1000 } else { 0b0100 };
                program(&mut dma, 2, mode | direction, 0, 0, count);
                let mut bus = MockBus::new(Box::new(dma));
                let mut service = |bus: &mut MockBus, len| {
                    if from_memory {
                        bus.device_mut()
                            .dma_service_read(2, &mut mem, &mut vec![0; len])
                    } else {
                        bus.device_mut()
                            .dma_service_write(2, &mut mem, &vec![0x5A; len])
                    }
                };
                let partial = count / 2;
                assert_eq!(
                    service(&mut bus, usize::from(partial)),
                    DmaTransfer {
                        transferred: usize::from(partial),
                        terminal_count: false,
                    }
                );
                assert_eq!(read_count_through_bus(&mut bus, 2), count - partial);
                let remaining = usize::from(count - partial) + 1;
                assert_eq!(
                    service(&mut bus, remaining + 1),
                    DmaTransfer {
                        transferred: remaining,
                        terminal_count: true,
                    }
                );
                let expected = if mode & MODE_AUTOINIT != 0 {
                    count
                } else {
                    0xFFFF
                };
                assert_eq!(read_count_through_bus(&mut bus, 2), expected);
            }
        }
    }

    #[test]
    fn count_readback_after_terminal_count_in_single_mode() {
        assert_count_readback_at_terminal_count(0x40);
    }

    #[test]
    fn count_readback_after_terminal_count_in_block_mode() {
        assert_count_readback_at_terminal_count(0x80);
    }

    #[test]
    fn count_readback_after_terminal_count_with_autoinit() {
        for mode in [0x40, 0x80] {
            assert_count_readback_at_terminal_count(mode | MODE_AUTOINIT);
        }
    }

    #[test]
    fn write_transfer_writes_memory_through_handle() {
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();
        // Channel 1, write transfer (device -> memory), page 2, addr 0x0100, 4 bytes.
        program(&mut dma, 1, 0b0100_0000 | 0b0100, 0x02, 0x0100, 3);
        assert_eq!(dma.transfer_type(1), TransferType::Write);
        let data = [0xDE, 0xAD, 0xBE, 0xEF];
        let n = dma.service_write(1, &mut mem, &data);
        assert_eq!(n.transferred, 4);
        // All four bytes landed at 0x02_0100.. through the handle.
        assert_eq!(mem.writes.len(), 4);
        for (i, &b) in data.iter().enumerate() {
            assert_eq!(mem.dma_read(0x020100 + i as u32), b);
        }
        // count+1 == 4 bytes exhausts the channel; TC flagged, channel re-masked.
        assert_ne!(dma.read_status() & (1 << 1), 0);
        assert!(!dma.channel_ready(1));
    }

    #[test]
    fn read_transfer_reads_memory_through_handle() {
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();
        mem.seed(0x030200, &[1, 2, 3, 4]);
        // Channel 2, read transfer (memory -> device), page 3, addr 0x0200, 4 bytes.
        program(&mut dma, 2, 0b0100_0000 | 0b1000, 0x03, 0x0200, 3);
        assert_eq!(dma.transfer_type(2), TransferType::Read);
        let mut out = [0u8; 4];
        let n = dma.service_read(2, &mut mem, &mut out);
        assert_eq!(n.transferred, 4);
        assert_eq!(out, [1, 2, 3, 4]);
    }

    #[test]
    fn address_decrement_mode_steps_down() {
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();
        // Decrement (bit 5), write transfer, addr 0x0010, count 2 (3 bytes).
        program(&mut dma, 0, MODE_DECREMENT | 0b0100, 0x00, 0x0010, 2);
        dma.service_write(0, &mut mem, &[0xA, 0xB, 0xC]);
        let addrs: Vec<u32> = mem.writes.iter().map(|&(a, _)| a).collect();
        assert_eq!(addrs, vec![0x0010, 0x000F, 0x000E]);
    }

    #[test]
    fn autoinit_reloads_at_terminal_count() {
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();
        // Autoinit (bit 4) + write transfer, addr 0x0000, count 1 (2 bytes).
        program(&mut dma, 3, MODE_AUTOINIT | 0b0100, 0x00, 0x0000, 1);
        dma.service_write(3, &mut mem, &[0x1, 0x2]);
        // After TC the channel reloads base and stays unmasked.
        assert!(dma.channel_ready(3));
        assert_eq!(dma.current_count(3), 1);
        assert_eq!(dma.physical_address(3) & 0xFFFF, 0x0000);
    }

    #[test]
    fn service_stops_at_terminal_count_even_with_more_data() {
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();
        // count 1 => 2 bytes max; hand it 5.
        program(&mut dma, 0, 0b0100, 0x00, 0x0000, 1);
        let n = dma.service_write(0, &mut mem, &[1, 2, 3, 4, 5]);
        assert_eq!(n.transferred, 2, "must transfer only count+1 bytes");
    }

    #[test]
    fn verify_transfer_steps_without_writing_memory() {
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();
        // Verify transfer type (bits 3-2 == 00).
        program(&mut dma, 0, 0b0000_0000, 0x00, 0x0000, 3);
        assert_eq!(dma.transfer_type(0), TransferType::Verify);
        let n = dma.service_write(0, &mut mem, &[1, 2, 3, 4]);
        assert_eq!(n.transferred, 4);
        assert!(mem.writes.is_empty(), "verify must not touch memory");
    }

    #[test]
    fn masked_channel_is_not_serviced() {
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();
        program(&mut dma, 0, 0b0100, 0x00, 0x0000, 3);
        // Re-mask channel 0.
        dma.port_write(0x0A, 1, 0b100).unwrap();
        let n = dma.service_write(0, &mut mem, &[1, 2, 3]);
        assert_eq!(n.transferred, 0);
        assert!(mem.writes.is_empty());
    }

    #[test]
    fn controller_disable_masks_all() {
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();
        program(&mut dma, 0, 0b0100, 0x00, 0x0000, 3);
        dma.port_write(0x08, 1, u32::from(CMD_DISABLE)).unwrap();
        assert_eq!(dma.service_write(0, &mut mem, &[1, 2]).transferred, 0);
    }

    #[test]
    fn all_mask_and_clear_mask_registers() {
        let mut dma = Dma8237::new();
        dma.port_write(0x0F, 1, 0b0000).unwrap(); // unmask all
        for ch in 0..CHANNELS {
            assert!(dma.channel_ready(ch));
        }
        dma.port_write(0x0E, 1, 0).unwrap(); // clear-mask also unmasks all
        assert!(dma.channel_ready(0));
        dma.port_write(0x0F, 1, 0b1111).unwrap(); // mask all
        assert!(!dma.channel_ready(3));
    }

    #[test]
    fn status_request_bits_and_tc_clear_on_read() {
        let mut dma = Dma8237::new();
        let mut mem = MockMem::default();
        program(&mut dma, 0, 0b0100, 0x00, 0x0000, 0); // 1 byte -> immediate TC
        dma.port_write(0x09, 1, 0b0100).unwrap(); // software request channel 0
        dma.service_write(0, &mut mem, &[0xFF]);
        let st = dma.read_status();
        assert_ne!(st & (1 << 0), 0, "TC flag for channel 0");
        assert_ne!(st & (1 << 4), 0, "request flag for channel 0");
        // TC bits clear on read; request bits persist.
        let st2 = dma.read_status();
        assert_eq!(st2 & (1 << 0), 0);
        assert_ne!(st2 & (1 << 4), 0);
    }

    #[test]
    fn master_clear_masks_and_resets() {
        let mut dma = Dma8237::new();
        dma.port_write(0x0F, 1, 0).unwrap(); // unmask all
        dma.port_write(0x08, 1, 0x04).unwrap(); // set a command bit
        dma.port_write(0x0D, 1, 0).unwrap(); // master clear
        assert!(!dma.channel_ready(0));
        assert_eq!(dma.command, 0);
    }

    #[test]
    fn page_latches_round_trip() {
        let mut dma = Dma8237::new();
        for off in 0..PAGE_LATCHES as u16 {
            dma.port_write(PAGE_BASE + off, 1, u32::from(off + 1))
                .unwrap();
        }
        for off in 0..PAGE_LATCHES as u16 {
            assert_eq!(
                dma.port_read(PAGE_BASE + off, 1).unwrap(),
                u32::from(off + 1)
            );
        }
    }

    #[test]
    fn mmio_is_unmapped() {
        let mut dma = Dma8237::new();
        assert_eq!(
            dma.mmio_read(0x00, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: 0x00,
            }
        );
    }

    #[test]
    fn unmapped_port_errors() {
        let mut dma = Dma8237::new();
        assert_eq!(
            dma.port_read(0x10, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "port",
                offset: 0x10,
            }
        );
    }

    #[test]
    fn reset_returns_to_power_on() {
        let mut dma = Dma8237::new();
        program(&mut dma, 0, 0b0100, 0x05, 0x1234, 10);
        dma.reset();
        assert!(!dma.channel_ready(0));
        assert_eq!(dma.physical_address(0), 0);
        assert_eq!(dma.current_count(0), 0);
    }

    #[test]
    fn save_restore_round_trip() {
        let mut dma = Dma8237::new();
        program(&mut dma, 0, MODE_AUTOINIT | 0b0100, 0x07, 0x2000, 100);
        program(&mut dma, 2, 0b1000, 0x03, 0x4000, 50);
        dma.port_write(0x08, 1, 0x00).unwrap();

        let blob = dma.save();
        let mut restored = Dma8237::new();
        restored.restore(&blob).unwrap();
        assert_eq!(restored.physical_address(0), dma.physical_address(0));
        assert_eq!(restored.physical_address(2), dma.physical_address(2));
        assert_eq!(restored.current_count(0), dma.current_count(0));
        assert_eq!(restored.transfer_type(0), dma.transfer_type(0));
        assert_eq!(restored.channel_ready(0), dma.channel_ready(0));
    }

    #[test]
    fn restore_rejects_bad_snapshot() {
        let mut dma = Dma8237::new();
        assert_eq!(
            dma.restore(&[SNAPSHOT_VERSION, 0]).unwrap_err(),
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unexpected snapshot length",
            }
        );
        let mut blob = dma.save();
        blob[0] = 0xFF;
        assert_eq!(
            dma.restore(&blob).unwrap_err(),
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            }
        );
    }

    #[test]
    fn mock_bus_isolation_harness() {
        let mut bus = MockBus::new(Box::new(Dma8237::new()));
        bus.tick(1);
        // Program channel 0 through the bus and confirm the page/address decode.
        bus.port_write(0x0C, 1, 0).unwrap();
        bus.port_write(0x00, 1, 0x00).unwrap();
        bus.port_write(0x00, 1, 0x10).unwrap();
        bus.port_write(0x87, 1, 0x05).unwrap();
        // Read status back (write-only-adjacent port) to prove decode works.
        assert_eq!(bus.port_read(0x08, 1).unwrap() & 0x0F, 0);
    }
}
