// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM VGA in colour text mode 3, 80×25 (§7, §9.2).
//!
//! Models the two hardware surfaces a text-mode VGA presents to the guest:
//!
//! - The **text buffer**: 32 KiB of video memory mapped at MMIO `0xB8000`. Each
//!   character cell is two bytes — glyph then attribute — so cell `n` lives at
//!   `0xB8000 + n*2`. The 80×25 display is the first `80*25 = 2000` cells
//!   (`4000` bytes) of the page selected by the CRTC start address.
//! - The **CRTC** (CRT controller): an indexed register file behind two ports,
//!   `0x3D4` (index) and `0x3D5` (data). Writing the index port selects a
//!   register; reads/writes of the data port hit the selected register.
//!
//! The card is an ordinary [`Device`]: it owns registers and memory and nothing
//! else. It does **not** implement the presentation seam — turning its memory
//! into a frame is the `video` crate's renderer, and presenting that frame is a
//! `FrameSink`. The card exposes just enough getters ([`VgaText::vram`],
//! [`VgaText::start_cell`], [`VgaText::cursor_cell`], [`VgaText::cursor_visible`])
//! for a renderer to read it without depending on this type.
//!
//! ## What is modelled
//! - The text buffer as flat MMIO storage (byte and word access).
//! - The full CRTC index/data access mechanism (all 25 registers store).
//! - The registers a text renderer interprets: cursor location high/low
//!   (`0x0E`/`0x0F`), display start address high/low (`0x0C`/`0x0D`), and the
//!   cursor-disable bit of cursor-start (`0x0A`, bit 5).
//!
//! ## Deviations from real hardware (not modelled)
//! - No attribute controller, sequencer, graphics controller, or DAC — only the
//!   CRTC and the text buffer. Mode setting (INT 10h / register programming to
//!   *enter* mode 3) is out of scope; the device is a text buffer + CRTC.
//! - No planar/graphics memory: the buffer is a plain byte array, correct for
//!   text mode but not for the 0xA0000 graphics window.
//! - No display timing / retrace: [`Device::tick`] only stamps virtual time onto
//!   traces. The input status register (`0x3DA`) and its retrace bits are not
//!   decoded, and the cursor does not blink on its own.
//! - Cursor scanline shape (`0x0A`/`0x0B` low bits) is stored but only the
//!   cursor-disable bit is interpreted.

use device::{AccessDir, Device, DeviceError, TraceEvent, VClock};

/// Stable device name reported by [`Device::name`].
const NAME: &str = "vga-text";

/// Visible columns in mode 3.
pub const COLS: u16 = 80;
/// Visible rows in mode 3.
pub const ROWS: u16 = 25;

/// Base MMIO address of the colour text buffer.
pub const TEXT_BUFFER_BASE: u64 = 0xB_8000;
/// Text buffer window length: 32 KiB, the colour text page window.
pub const TEXT_BUFFER_LEN: usize = 0x8000;

/// CRTC index port (colour). Selects the register the data port addresses.
pub const CRTC_INDEX_PORT: u16 = 0x3D4;
/// CRTC data port (colour). Reads/writes the selected register.
pub const CRTC_DATA_PORT: u16 = 0x3D5;

/// Number of CRTC registers modelled (indices `0x00..=0x18`).
const CRTC_REG_COUNT: usize = 0x19;

/// CRTC index of the cursor-start register (`0x0A`). Bit 5 disables the cursor.
pub const CRTC_CURSOR_START_INDEX: u8 = 0x0A;
/// CRTC index of the display start-address high byte (`0x0C`).
pub const CRTC_START_HIGH_INDEX: u8 = 0x0C;
/// CRTC index of the display start-address low byte (`0x0D`).
pub const CRTC_START_LOW_INDEX: u8 = 0x0D;
/// CRTC index of the cursor-location high byte (`0x0E`).
pub const CRTC_CURSOR_HIGH_INDEX: u8 = 0x0E;
/// CRTC index of the cursor-location low byte (`0x0F`).
pub const CRTC_CURSOR_LOW_INDEX: u8 = 0x0F;

/// Cursor-disable bit within the cursor-start register (`0x0A`).
const CURSOR_DISABLE: u8 = 0x20;

/// Snapshot layout version tag, first byte of [`Device::save`] output.
const SNAPSHOT_VERSION: u8 = 1;

/// Fixed-size snapshot header: version + CRTC index + the CRTC register file.
const SNAPSHOT_HEADER_LEN: usize = 1 + 1 + CRTC_REG_COUNT;

/// The VGA text-mode device (§9.2).
///
/// Decodes the 32 KiB MMIO text buffer at [`TEXT_BUFFER_BASE`] and the two CRTC
/// ports [`CRTC_INDEX_PORT`]/[`CRTC_DATA_PORT`].
pub struct VgaText {
    /// Text video memory: two bytes per cell (glyph, attribute).
    vram: Vec<u8>,
    /// Currently selected CRTC register (set by writes to the index port).
    crtc_index: u8,
    /// The CRTC register file. All 25 registers store; a subset is interpreted.
    crtc: [u8; CRTC_REG_COUNT],
    /// Trace events observed since the last [`Device::take_trace`].
    trace: Vec<TraceEvent>,
    /// Virtual time of the most recent [`Device::tick`], stamped onto traces.
    now: VClock,
}

impl VgaText {
    /// Create a VGA text device with a cleared buffer and zeroed CRTC registers.
    #[must_use]
    pub fn new() -> Self {
        Self {
            vram: vec![0; TEXT_BUFFER_LEN],
            crtc_index: 0,
            crtc: [0; CRTC_REG_COUNT],
            trace: Vec::new(),
            now: VClock::ZERO,
        }
    }

    /// Borrow the text video memory (two bytes per cell).
    #[must_use]
    pub fn vram(&self) -> &[u8] {
        &self.vram
    }

    /// Display start address in cells, from CRTC `0x0C`/`0x0D`.
    #[must_use]
    pub fn start_cell(&self) -> u16 {
        u16::from(self.crtc[usize::from(CRTC_START_HIGH_INDEX)]) << 8
            | u16::from(self.crtc[usize::from(CRTC_START_LOW_INDEX)])
    }

    /// Absolute cursor location in cells, from CRTC `0x0E`/`0x0F`.
    #[must_use]
    pub fn cursor_cell(&self) -> u16 {
        u16::from(self.crtc[usize::from(CRTC_CURSOR_HIGH_INDEX)]) << 8
            | u16::from(self.crtc[usize::from(CRTC_CURSOR_LOW_INDEX)])
    }

    /// Whether the cursor is displayed (cursor-disable bit of CRTC `0x0A` clear).
    #[must_use]
    pub fn cursor_visible(&self) -> bool {
        self.crtc[usize::from(CRTC_CURSOR_START_INDEX)] & CURSOR_DISABLE == 0
    }

    /// Byte offset of `addr` within the text buffer window, or an unmapped-MMIO
    /// error if `addr` is outside `TEXT_BUFFER_BASE..+TEXT_BUFFER_LEN`.
    fn mmio_offset(&self, addr: u64) -> Result<usize, DeviceError> {
        if addr < TEXT_BUFFER_BASE || addr >= TEXT_BUFFER_BASE + TEXT_BUFFER_LEN as u64 {
            return Err(DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: addr,
            });
        }
        Ok((addr - TEXT_BUFFER_BASE) as usize)
    }

    fn trace_mmio(&mut self, addr: u64, size: u8, dir: AccessDir, value: u64) {
        self.trace.push(TraceEvent::Mmio {
            at: self.now,
            addr,
            size,
            dir,
            value,
        });
    }

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

impl Default for VgaText {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for VgaText {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let value = match port {
            CRTC_INDEX_PORT => u32::from(self.crtc_index),
            CRTC_DATA_PORT => u32::from(self.crtc[usize::from(self.crtc_index) % CRTC_REG_COUNT]),
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
            CRTC_INDEX_PORT => self.crtc_index = byte,
            CRTC_DATA_PORT => {
                let index = usize::from(self.crtc_index) % CRTC_REG_COUNT;
                self.crtc[index] = byte;
            }
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

    fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
        let offset = self.mmio_offset(addr)?;
        // Registers are byte storage; read `size` little-endian bytes, blank past
        // the end of the window.
        let mut value: u64 = 0;
        for i in 0..usize::from(size) {
            let byte = self.vram.get(offset + i).copied().unwrap_or(0);
            value |= u64::from(byte) << (8 * i);
        }
        self.trace_mmio(addr, size, AccessDir::Read, value);
        Ok(value)
    }

    fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
        let offset = self.mmio_offset(addr)?;
        for i in 0..usize::from(size) {
            if let Some(slot) = self.vram.get_mut(offset + i) {
                *slot = (value >> (8 * i)) as u8;
            }
        }
        self.trace_mmio(addr, size, AccessDir::Write, value);
        Ok(())
    }

    fn tick(&mut self, now: VClock) {
        self.now = now;
    }

    fn irq_pending(&self) -> bool {
        // A text-mode VGA drives no IRQ line in this model (no vertical-retrace
        // interrupt wired).
        false
    }

    fn irq_vector(&self) -> Option<u8> {
        None
    }

    fn reset(&mut self) {
        *self = VgaText::new();
    }

    fn save(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(SNAPSHOT_HEADER_LEN + self.vram.len());
        blob.push(SNAPSHOT_VERSION);
        blob.push(self.crtc_index);
        blob.extend_from_slice(&self.crtc);
        blob.extend_from_slice(&self.vram);
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        if snapshot.len() != SNAPSHOT_HEADER_LEN + TEXT_BUFFER_LEN {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "snapshot length mismatch",
            });
        }
        if snapshot[0] != SNAPSHOT_VERSION {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "unsupported snapshot version",
            });
        }
        self.crtc_index = snapshot[1];
        self.crtc.copy_from_slice(&snapshot[2..2 + CRTC_REG_COUNT]);
        self.vram.copy_from_slice(&snapshot[SNAPSHOT_HEADER_LEN..]);
        Ok(())
    }

    fn take_trace(&mut self) -> Vec<TraceEvent> {
        std::mem::take(&mut self.trace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::MockBus;

    /// MMIO address of text cell `n`.
    fn cell_addr(n: u64) -> u64 {
        TEXT_BUFFER_BASE + n * 2
    }

    #[test]
    fn mmio_word_write_stores_glyph_and_attribute() {
        let mut vga = VgaText::new();
        // 'A' with attribute 0x1F, as a 16-bit little-endian cell.
        let cell = u64::from(b'A') | (0x1F << 8);
        vga.mmio_write(cell_addr(0), 2, cell).unwrap();
        assert_eq!(vga.vram()[0], b'A');
        assert_eq!(vga.vram()[1], 0x1F);
        assert_eq!(vga.mmio_read(cell_addr(0), 2).unwrap(), cell);
    }

    #[test]
    fn mmio_byte_writes_hit_glyph_then_attribute() {
        let mut vga = VgaText::new();
        vga.mmio_write(cell_addr(3), 1, u64::from(b'Z')).unwrap();
        vga.mmio_write(cell_addr(3) + 1, 1, 0x70).unwrap();
        assert_eq!(vga.vram()[6], b'Z');
        assert_eq!(vga.vram()[7], 0x70);
    }

    #[test]
    fn mmio_outside_window_is_unmapped() {
        let mut vga = VgaText::new();
        let below = TEXT_BUFFER_BASE - 1;
        assert_eq!(
            vga.mmio_write(below, 1, 0).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: below,
            }
        );
        let above = TEXT_BUFFER_BASE + TEXT_BUFFER_LEN as u64;
        assert_eq!(
            vga.mmio_read(above, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: above,
            }
        );
    }

    #[test]
    fn crtc_cursor_location_round_trips() {
        let mut vga = VgaText::new();
        // Cursor at cell 5: high = 0, low = 5.
        vga.port_write(CRTC_INDEX_PORT, 1, u32::from(CRTC_CURSOR_HIGH_INDEX))
            .unwrap();
        vga.port_write(CRTC_DATA_PORT, 1, 0).unwrap();
        vga.port_write(CRTC_INDEX_PORT, 1, u32::from(CRTC_CURSOR_LOW_INDEX))
            .unwrap();
        vga.port_write(CRTC_DATA_PORT, 1, 5).unwrap();
        assert_eq!(vga.cursor_cell(), 5);
        // Data port reads back the selected register (index still cursor-low).
        assert_eq!(vga.port_read(CRTC_DATA_PORT, 1).unwrap(), 5);
        // Index port reads back the current index.
        assert_eq!(
            vga.port_read(CRTC_INDEX_PORT, 1).unwrap(),
            u32::from(CRTC_CURSOR_LOW_INDEX)
        );
    }

    #[test]
    fn crtc_start_address_round_trips() {
        let mut vga = VgaText::new();
        // Start at cell 0x0140 (page-scrolled): high = 0x01, low = 0x40.
        vga.port_write(CRTC_INDEX_PORT, 1, u32::from(CRTC_START_HIGH_INDEX))
            .unwrap();
        vga.port_write(CRTC_DATA_PORT, 1, 0x01).unwrap();
        vga.port_write(CRTC_INDEX_PORT, 1, u32::from(CRTC_START_LOW_INDEX))
            .unwrap();
        vga.port_write(CRTC_DATA_PORT, 1, 0x40).unwrap();
        assert_eq!(vga.start_cell(), 0x0140);
    }

    #[test]
    fn cursor_disable_bit_hides_cursor() {
        let mut vga = VgaText::new();
        assert!(vga.cursor_visible());
        vga.port_write(CRTC_INDEX_PORT, 1, u32::from(CRTC_CURSOR_START_INDEX))
            .unwrap();
        vga.port_write(CRTC_DATA_PORT, 1, u32::from(CURSOR_DISABLE))
            .unwrap();
        assert!(!vga.cursor_visible());
    }

    #[test]
    fn unmapped_port_is_rejected() {
        let mut vga = VgaText::new();
        assert_eq!(
            vga.port_read(0x3DA, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "port",
                offset: 0x3DA,
            }
        );
    }

    #[test]
    fn no_irq_is_asserted() {
        let vga = VgaText::new();
        assert!(!vga.irq_pending());
        assert_eq!(vga.irq_vector(), None);
    }

    #[test]
    fn reset_clears_buffer_and_registers() {
        let mut vga = VgaText::new();
        vga.mmio_write(cell_addr(0), 2, u64::from(b'X') | (0x07 << 8))
            .unwrap();
        vga.port_write(CRTC_INDEX_PORT, 1, u32::from(CRTC_CURSOR_LOW_INDEX))
            .unwrap();
        vga.port_write(CRTC_DATA_PORT, 1, 9).unwrap();
        vga.reset();
        assert_eq!(vga.vram()[0], 0);
        assert_eq!(vga.cursor_cell(), 0);
    }

    #[test]
    fn save_restore_round_trip_preserves_state() {
        let mut vga = VgaText::new();
        vga.mmio_write(cell_addr(0), 2, u64::from(b'Q') | (0x4E << 8))
            .unwrap();
        vga.port_write(CRTC_INDEX_PORT, 1, u32::from(CRTC_CURSOR_LOW_INDEX))
            .unwrap();
        vga.port_write(CRTC_DATA_PORT, 1, 7).unwrap();

        let blob = vga.save();
        let mut restored = VgaText::new();
        restored.restore(&blob).unwrap();

        assert_eq!(restored.vram()[0], b'Q');
        assert_eq!(restored.vram()[1], 0x4E);
        assert_eq!(restored.cursor_cell(), 7);
        // The selected CRTC index survives too.
        assert_eq!(
            restored.port_read(CRTC_INDEX_PORT, 1).unwrap(),
            u32::from(CRTC_CURSOR_LOW_INDEX)
        );
    }

    #[test]
    fn restore_wrong_length_is_rejected() {
        let mut vga = VgaText::new();
        let err = vga.restore(&[SNAPSHOT_VERSION, 0, 0]).unwrap_err();
        assert_eq!(
            err,
            DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "snapshot length mismatch",
            }
        );
    }

    #[test]
    fn mmio_write_is_traced_with_virtual_time() {
        // Prove the card runs under the isolation harness and its MMIO writes
        // surface on the tracing seam with the current virtual time.
        let mut bus = MockBus::new(Box::new(VgaText::new()));
        bus.tick(4);
        let cell = u64::from(b'H') | (0x1F << 8);
        bus.mmio_write(cell_addr(0), 2, cell).unwrap();

        let events = bus.device_mut().take_trace();
        assert_eq!(
            events,
            vec![TraceEvent::Mmio {
                at: VClock(4),
                addr: cell_addr(0),
                size: 2,
                dir: AccessDir::Write,
                value: cell,
            }]
        );
    }
}
