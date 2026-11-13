// SPDX-License-Identifier: AGPL-3.0-or-later
//! Lo-tech 2MB EMS Board schematic, revision r02: 8-bit ISA expanded memory.
//!
//! HCT688 compares A2–A9 with the port jumpers, gated by AEN; A10–A15
//! are ignored. HCT139 decodes A0/A1 and IOW into four HCT573 page latches.
//! IOR is unwired. Bits 0–4 drive SRAM A14–A18;
//! HCT138 selects one of four populated 512 KiB chips using bits 5–7.
//! The frame HCT688 enable is tied active; A16–A19 select the frame.
//! SMEMR/SMEMW gate SRAM OE/WE and the direction of the 74ACT245 data buffer.
//! Unbacked reads return FFh from data-bus pull-ups through that buffer.
//! See `backlog/docs/design/spec.md` §4.2 and
//! `backlog/docs/architecture.md` §x86jit integration.
//!
//! ## Coverage matrix
//! | Hardware | Model |
//! |---|---|
//! | Jumpered ports and aliases, frame and 1–4 SRAM chips | implemented |
//! | Unconnected chip selects and IOR | FFh reads, dropped writes |
//! | HCT573 without reset, SRAM retention | CPU/device reset preserves both |
//! | Undefined power-on latch values | model choice: FFh, matching driver unmap |
//! | SRAM power-on contents | model choice: zero for deterministic execution |
//! | AEN and SMEM strobes, 8-bit cycles | CPU bus cycles only; no electrical timing |
//! | Snapshot | versioned configuration, latches, SRAM and absolute clock; no traces |
//! | Instruction fetch from the frame | CPU fetch fault stops the run with a CPU error; tracked as open work |

use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};

const PAGE: usize = 0x4000;
const FRAME: u64 = 0x10000;

/// Invalid jumper or SRAM population selection.
#[derive(Debug, thiserror::Error)]
#[error("Lo-tech EMS requires port 260h/264h/268h/26Ch, frame C000h/D000h/E000h and 512/1024/1536/2048 KiB")]
pub struct LotechEmsConfigError;

/// Discrete-logic EMS board; backing SRAM never aliases ordinary guest RAM.
pub struct LotechEms {
    port: u16,
    frame: u16,
    pages: [u8; 4],
    ram: Vec<u8>,
    now: VClock,
    trace: device::TraceBuffer,
}

impl LotechEms {
    /// Lo-tech 2MB EMS Board schematic r02, HCT688 I/O jumper positions.
    pub const PORTS: &[u16] = &[0x260, 0x264, 0x268, 0x26C];
    /// Lo-tech 2MB EMS Board schematic r02, physical frame segment jumpers.
    pub const FRAMES: &[u16] = &[0xC000, 0xD000, 0xE000];
    /// Lo-tech 2MB EMS Board schematic r02, one to four 512 KiB SRAM chips.
    pub const SIZES_KIB: &[u32] = &[512, 1024, 1536, 2048];

    /// Check the physical jumper positions and chip populations without allocation.
    pub fn valid_configuration(port: u16, frame: u16, size_kib: u32) -> bool {
        Self::valid_port(port) && Self::valid_frame(frame) && Self::valid_size_kib(size_kib)
    }

    /// Whether the I/O base matches one of the HCT688 jumper positions.
    pub fn valid_port(port: u16) -> bool {
        Self::PORTS.contains(&port)
    }

    /// Whether the segment matches a physical frame jumper position.
    pub fn valid_frame(frame: u16) -> bool {
        Self::FRAMES.contains(&frame)
    }

    /// Whether the SRAM population uses one to four complete chips.
    pub fn valid_size_kib(size_kib: u32) -> bool {
        Self::SIZES_KIB.contains(&size_kib)
    }

    /// Power on with deterministic unbacked windows and zero-filled SRAM.
    pub fn new(port: u16, frame: u16, size_kib: u32) -> Result<Self, LotechEmsConfigError> {
        if !Self::valid_configuration(port, frame, size_kib) {
            return Err(LotechEmsConfigError);
        }
        Ok(Self {
            port,
            frame,
            pages: [0xFF; 4],
            ram: vec![0; size_kib as usize * 1024],
            now: VClock::ZERO,
            trace: device::TraceBuffer::default(),
        })
    }

    fn register(&self, port: u16, size: u8) -> Result<usize, DeviceError> {
        if size != 1 {
            return Err(DeviceError::UnmappedRegister {
                kind: "port width",
                offset: u64::from(port),
            });
        }
        if port & 0x3FC == self.port {
            Ok((port & 3) as usize)
        } else {
            Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: port.into(),
            })
        }
    }

    fn check_frame(&self, addr: u64, size: u8) -> Result<usize, DeviceError> {
        let base = u64::from(self.frame) << 4;
        if matches!(size, 1..=8)
            && addr >= base
            && addr
                .checked_add(u64::from(size))
                .is_some_and(|end| end <= base + FRAME)
        {
            Ok((addr - base) as usize)
        } else {
            Err(DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: addr,
            })
        }
    }

    fn backing(&self, offset: usize) -> usize {
        usize::from(self.pages[offset / PAGE]) * PAGE + offset % PAGE
    }
}

impl Device for LotechEms {
    fn name(&self) -> &'static str {
        "lotech-ems"
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        self.register(port, size)?;
        let value = 0xFF;
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
        let index = self.register(port, size)?;
        self.pages[index] = value as u8;
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
        let offset = self.check_frame(addr, size)?;
        let mut value = 0;
        for byte in 0..usize::from(size) {
            value |= u64::from(
                self.ram
                    .get(self.backing(offset + byte))
                    .copied()
                    .unwrap_or(0xFF),
            ) << (byte * 8);
        }
        self.trace.push(TraceEvent::Mmio {
            at: self.now,
            addr,
            size,
            dir: AccessDir::Read,
            value,
        });
        Ok(value)
    }
    fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
        let offset = self.check_frame(addr, size)?;
        for byte in 0..usize::from(size) {
            let index = self.backing(offset + byte);
            if let Some(cell) = self.ram.get_mut(index) {
                *cell = (value >> (byte * 8)) as u8;
            }
        }
        self.trace.push(TraceEvent::Mmio {
            at: self.now,
            addr,
            size,
            dir: AccessDir::Write,
            value,
        });
        Ok(())
    }
    fn tick(&mut self, now: VClock) {
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now >= self.now {
            self.now = now;
        }
    }
    fn next_event(&self, _: VClock) -> NextEvent {
        NextEvent::Dormant
    }
    fn irq_pending(&self) -> bool {
        false
    }
    fn reset(&mut self) {
        // ISA RESET is not connected to the latches or SRAM.
        self.now = VClock::ZERO;
        self.trace.clear();
    }
    fn save(&self) -> Vec<u8> {
        let mut blob = vec![1];
        blob.extend(self.port.to_le_bytes());
        blob.extend(self.frame.to_le_bytes());
        blob.extend(self.pages);
        blob.extend(self.now.0.to_le_bytes());
        blob.extend(&self.ram);
        blob
    }
    fn restore(&mut self, blob: &[u8]) -> Result<(), DeviceError> {
        if blob.len() != 17 + self.ram.len()
            || blob[0] != 1
            || blob[1..3] != self.port.to_le_bytes()
            || blob[3..5] != self.frame.to_le_bytes()
        {
            return Err(DeviceError::InvalidSnapshot {
                device: self.name(),
                reason: "version, jumpers or population differ",
            });
        }
        self.pages.copy_from_slice(&blob[5..9]);
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| blob[9 + i])));
        self.ram.copy_from_slice(&blob[17..]);
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
mod tests;
