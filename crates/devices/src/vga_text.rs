// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM VGA in colour text mode 3, 80×25 (§7, §9.2).
//!
//! IBM VGA Technical Reference, Alphanumeric Modes and CRT Controller
//! Registers (Start Address, Cursor Location, Cursor Start). Passive scanout is
//! described in `backlog/docs/design/observation.md`, §Typed capabilities and
//! route ownership.
//!
//! Models the hardware surfaces a text-mode VGA presents to the guest:
//!
//! - The **text buffer**: 32 KiB of video memory mapped at MMIO `0xB8000`. Each
//!   character cell is two bytes — glyph then attribute — so cell `n` lives at
//!   `0xB8000 + n*2`. The 80×25 display is the first `80*25 = 2000` cells
//!   (`4000` bytes) of the page selected by the CRTC start address.
//! - The **CRTC** (CRT controller): an indexed register file behind two ports,
//!   `0x3D4` (index) and `0x3D5` (data). Writing the index port selects a
//!   register; reads/writes of the data port hit the selected register.
//! - A small **CGA-compatible register surface** a stock PC BIOS drives when it
//!   treats this card as a 6845 CGA: the mode-control register (`0x3D8`), the
//!   colour-select register (`0x3D9`), and the input status register (`0x3DA`).
//!   Modelling these is what lets an off-the-shelf BIOS's INT 10h run: it writes
//!   mode/colour and, before touching video memory, busy-waits on the status
//!   register's vertical-retrace bit.
//!
//! The BIOS profile's monochrome console is the separate MDA card.
//!
//! The card is an ordinary [`Device`]: it owns registers and memory and nothing
//! else. It does **not** implement the presentation seam — turning its memory
//! into a frame is the `video` crate's renderer, and presenting that frame is a
//! `FrameSink`. The card exposes just enough getters ([`VgaText::vram`],
//! [`VgaText::start_cell`], [`VgaText::cursor_cell`], [`VgaText::cursor_visible`])
//! for a renderer to read it without depending on this type.
//!
//! ## What is modelled
//! - The text buffer as flat MMIO storage, with 1–8 byte little-endian accesses.
//!   Width 0 or above 8 is rejected before memory or traces change. Direct
//!   accesses past the window return zero for trailing read bytes and discard
//!   trailing write bytes; the bus splits accesses at decode-window boundaries.
//! - The full CRTC index/data access mechanism (all 25 registers store).
//! - The registers a text renderer interprets: cursor location high/low
//!   (`0x0E`/`0x0F`), display start address high/low (`0x0C`/`0x0D`), and the
//!   cursor-disable bit of cursor-start (`0x0A`, bit 5).
//! - The CGA mode-control (`0x3D8`) and colour-select (`0x3D9`) registers:
//!   writes are stored and read back. Mode-control bit 5 selects attribute bit 7
//!   as blink when set or background intensity when clear, including at reset.
//!   Source: IBM PC Technical Reference, Options and Adapters, IBM Color/Graphics
//!   Monitor Adapter, Programming Considerations, Mode Control Register.
//!   No graphics mode is entered — the card always renders text mode 3.
//! - The CGA input status register (`0x3DA`): its display-enable (bit 0) and
//!   vertical-retrace (bit 3) bits, cycled deterministically so a BIOS
//!   both-edges retrace wait terminates (see `VgaText::status_register`).
//!
//! IBM VGA Technical Reference, CRT Controller Registers, Address Register:
//! only index bits 4–0 select a register; indices 19h–1Fh are reserved.
//! Assumption for reserved accesses: ignore writes and return zero on reads,
//! since the reference specifies no data value. They never alias valid registers.
//!
//! ## Deviations from real hardware (not modelled)
//! - No attribute controller, sequencer, graphics controller, or DAC — only the
//!   CRTC, the CGA register surface above, and the text buffer. The card never
//!   leaves text mode: the graphics-mode bits of the CGA mode-control register
//!   are stored but not acted on, and its video-enable bit (bit 3) does not
//!   blank the screen, so cells written before a mode set are shown (with
//!   attribute bit 7 as background intensity, since reset clears bit 5).
//! - No planar/graphics memory: the buffer is a plain byte array, correct for
//!   text mode but not for the 0xA0000 graphics window.
//! - No real display timing: the status register's retrace/display bits are a
//!   deterministic per-read phase, not derived from a pixel clock, and the card
//!   drives no vertical-retrace interrupt. The colour-select register's border/
//!   background/palette choice is stored but not rendered. The cursor does not
//!   blink on its own.
//! - Cursor scanline shape (`0x0A`/`0x0B` low bits) is stored but only the
//!   cursor-disable bit is interpreted.
//! - The I/O address select of the Miscellaneous Output register (bit 0) is
//!   not modelled: the CRTC and status registers always decode at the colour
//!   addresses, and the monochrome addresses `0x3B4`/`0x3B5`/`0x3BA` are
//!   unmapped on this card. A machine with a monochrome console uses the MDA
//!   card for them.
//!
//! Snapshots contain CRTC/CGA registers, VRAM, the per-read retrace phase
//! and the absolute trace time base. There are no pixel-clock deadlines in this
//! model (IBM VGA Technical Reference, CRT Controller Registers).

use device::{AccessDir, Device, DeviceError, NextEvent, TextModeState, TraceEvent, VClock};

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

/// CGA mode-control register (`0x3D8`). Write-only on real CGA; a BIOS uses it to
/// select the video mode. We store the byte and keep rendering text mode 3.
pub const CGA_MODE_CONTROL_PORT: u16 = 0x3D8;
/// CGA colour-select register (`0x3D9`). Selects border/background/palette; we
/// store the byte but do not render its colours yet.
pub const CGA_COLOR_SELECT_PORT: u16 = 0x3D9;
/// CGA input status register (`0x3DA`, read-only). A BIOS polls its
/// vertical-retrace bit before writing video memory.
pub const CGA_STATUS_PORT: u16 = 0x3DA;

/// Display / horizontal-drive bit of the status register (bit 0).
const STATUS_DISPLAY_ENABLE: u8 = 0x01;
/// Retrace / video bit of the status register (bit 3).
const STATUS_VERTICAL_RETRACE: u8 = 0x08;

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
const SNAPSHOT_VERSION: u8 = 3;

const SNAPSHOT_CRTC_INDEX: usize = 1;
const SNAPSHOT_CRTC_REGS: usize = SNAPSHOT_CRTC_INDEX + 1;
const SNAPSHOT_MODE_CONTROL: usize = SNAPSHOT_CRTC_REGS + CRTC_REG_COUNT;
const SNAPSHOT_COLOR_SELECT: usize = SNAPSHOT_MODE_CONTROL + 1;
const SNAPSHOT_STATUS_READS: usize = SNAPSHOT_COLOR_SELECT + 1;
const SNAPSHOT_CLOCK: usize = SNAPSHOT_STATUS_READS + 8;
/// Header preceding VRAM: registers, status-read phase and absolute trace clock.
const SNAPSHOT_HEADER_LEN: usize = SNAPSHOT_CLOCK + 8;

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
    /// CGA mode-control register (`0x3D8`). Stored so a BIOS mode set does not
    /// fault; graphics-mode bits are recorded but not acted on (text mode 3 only).
    mode_control: u8,
    /// CGA colour-select register (`0x3D9`). Stored; the border/background/palette
    /// it selects is not rendered yet.
    color_select: u8,
    /// Free-running counter bumped on every read of the input status register
    /// (`0x3DA`), driving its retrace/display bits
    /// through their phases. See `VgaText::status_register` for why the phase
    /// advances per read, not per tick.
    status_reads: u64,
    /// Trace events observed since the last [`Device::take_trace`].
    trace: device::TraceBuffer,
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
            mode_control: 0,
            color_select: 0,
            status_reads: 0,
            trace: device::TraceBuffer::default(),
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

    /// Current colour input-status byte (`0x3DA`), advancing the retrace phase.
    ///
    /// A stock BIOS busy-waits on this register before touching video memory: it
    /// spins until the retrace bit goes high, then spins until it goes low again
    /// (both edges), to write only during retrace and avoid CGA "snow". This
    /// compatibility model advances the phase on reads, independently of virtual
    /// time, so both edges remain observable even at a single clock. Scheduling
    /// preserves that approximation; a real retrace clock is a separate model
    /// change (see `backlog/docs/design/scheduler.md` § Device migration inventory).
    ///
    /// Phase, in the low bits of the read count: bit 0 toggles every read (both
    /// edges seen within 2 reads); bit 3 toggles every second read (both edges
    /// seen within 4 reads), so polling either bit observes both edges.
    ///
    fn status_register(&mut self) -> u8 {
        let count = self.status_reads;
        self.status_reads = count.wrapping_add(1);
        let mut status = 0;
        if count & 0b01 != 0 {
            status |= STATUS_DISPLAY_ENABLE;
        }
        if count & 0b10 != 0 {
            status |= STATUS_VERTICAL_RETRACE;
        }
        status
    }

    /// Byte offset of `addr` within the text buffer window, or an unmapped-MMIO
    /// error if `addr` is outside `TEXT_BUFFER_BASE..+TEXT_BUFFER_LEN` or the
    /// width cannot fit in the MMIO value type.
    fn mmio_offset(&self, addr: u64, size: u8) -> Result<usize, DeviceError> {
        if !(1..=8).contains(&size)
            || !(TEXT_BUFFER_BASE..TEXT_BUFFER_BASE + TEXT_BUFFER_LEN as u64).contains(&addr)
        {
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
    fn text_scanout(&self) -> Option<TextModeState<'_>> {
        Some(TextModeState {
            vram: self.vram(),
            cols: COLS,
            rows: ROWS,
            start_cell: self.start_cell(),
            cursor_cell: self.cursor_cell(),
            cursor_visible: self.cursor_visible(),
            attributes: device::TextAttributeSet::Colour {
                blink_enabled: self.mode_control & 0x20 != 0,
            },
        })
    }

    fn name(&self) -> &'static str {
        NAME
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let value = match port {
            CRTC_INDEX_PORT => u32::from(self.crtc_index),
            CRTC_DATA_PORT => u32::from(
                self.crtc
                    .get(usize::from(self.crtc_index))
                    .copied()
                    .unwrap_or(0),
            ),
            // Real CGA mode-control/colour-select are write-only, but returning
            // the last value written is simpler than modelling a floating bus and
            // is harmless — a BIOS writes these, it does not read them back.
            CGA_MODE_CONTROL_PORT => u32::from(self.mode_control),
            CGA_COLOR_SELECT_PORT => u32::from(self.color_select),
            CGA_STATUS_PORT => u32::from(self.status_register()),
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
            CRTC_INDEX_PORT => self.crtc_index = byte & 0x1F,
            CRTC_DATA_PORT => {
                if let Some(register) = self.crtc.get_mut(usize::from(self.crtc_index)) {
                    *register = byte;
                }
            }
            CGA_MODE_CONTROL_PORT => self.mode_control = byte,
            CGA_COLOR_SELECT_PORT => self.color_select = byte,
            // The status register is read-only; ignore a write rather than fault
            // the machine on a stray one.
            CGA_STATUS_PORT => {}
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
        let offset = self.mmio_offset(addr, size)?;
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
        let offset = self.mmio_offset(addr, size)?;
        for i in 0..usize::from(size) {
            if let Some(slot) = self.vram.get_mut(offset + i) {
                *slot = (value >> (8 * i)) as u8;
            }
        }
        self.trace_mmio(addr, size, AccessDir::Write, value);
        Ok(())
    }

    fn tick(&mut self, now: VClock) {
        debug_assert!(now >= self.now, "device clock moved backwards");
        if now < self.now {
            return;
        }
        self.now = now;
    }

    fn next_event(&self, _now: VClock) -> NextEvent {
        NextEvent::Dormant
    }

    fn irq_pending(&self) -> bool {
        // A text-mode VGA drives no IRQ line in this model (no vertical-retrace
        // interrupt wired).
        false
    }

    fn reset(&mut self) {
        let trace_policy = self.trace.policy();
        *self = VgaText::new();
        self.trace.set_policy(trace_policy);
    }

    fn save(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(SNAPSHOT_HEADER_LEN + self.vram.len());
        blob.push(SNAPSHOT_VERSION);
        blob.push(self.crtc_index);
        blob.extend_from_slice(&self.crtc);
        blob.push(self.mode_control);
        blob.push(self.color_select);
        blob.extend_from_slice(&self.status_reads.to_le_bytes());
        blob.extend_from_slice(&self.now.0.to_le_bytes());
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
        if snapshot[SNAPSHOT_CRTC_INDEX] > 0x1F {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "invalid CRTC index",
            });
        }
        self.status_reads =
            u64::from_le_bytes(std::array::from_fn(|i| snapshot[SNAPSHOT_STATUS_READS + i]));
        self.crtc_index = snapshot[SNAPSHOT_CRTC_INDEX];
        self.crtc
            .copy_from_slice(&snapshot[SNAPSHOT_CRTC_REGS..SNAPSHOT_MODE_CONTROL]);
        self.mode_control = snapshot[SNAPSHOT_MODE_CONTROL];
        self.color_select = snapshot[SNAPSHOT_COLOR_SELECT];
        self.vram.copy_from_slice(&snapshot[SNAPSHOT_HEADER_LEN..]);
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| {
            snapshot[SNAPSHOT_CLOCK + i]
        })));
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

    #[test]
    fn zero_mmio_width_is_rejected_without_state_changes_or_trace() {
        assert_invalid_mmio_width(0);
    }

    #[test]
    fn oversized_mmio_widths_are_rejected_without_state_changes_or_trace() {
        for size in [9, 16, u8::MAX] {
            assert_invalid_mmio_width(size);
        }
    }

    fn assert_invalid_mmio_width(size: u8) {
        let mut bus = MockBus::new(Box::new(crate::trace_policy_tests::traced(VgaText::new())));
        bus.tick(4);
        bus.mmio_write(TEXT_BUFFER_BASE, 8, 0x0123_4567_89AB_CDEF)
            .unwrap();
        bus.device_mut().take_trace();
        let saved = bus.device().save();
        let error = || DeviceError::UnmappedRegister {
            kind: "mmio",
            offset: TEXT_BUFFER_BASE,
        };
        assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE, size), Err(error()));
        assert_eq!(bus.device().save(), saved);
        assert!(bus.device_mut().take_trace().is_empty());
        assert_eq!(
            bus.mmio_write(TEXT_BUFFER_BASE, size, u64::MAX),
            Err(error())
        );
        assert_eq!(bus.device().save(), saved);
        assert!(bus.device_mut().take_trace().is_empty());
    }

    #[test]
    fn monochrome_ports_are_unmapped_without_changing_card_state() {
        let mut bus = MockBus::new(Box::new(crate::trace_policy_tests::traced(VgaText::new())));
        let saved = bus.device().save();
        for port in [0x3B4, 0x3B5, 0x3BA] {
            let error = || DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            };
            assert_eq!(bus.port_read(port, 1), Err(error()));
            assert_eq!(bus.port_write(port, 1, 0xFF), Err(error()));
        }
        assert_eq!(bus.device().save(), saved);
        assert!(bus.device_mut().take_trace().is_empty());
    }

    #[test]
    fn scanout_reports_colour_attributes_and_mode_control_blink_selection() {
        let mut bus = MockBus::new(Box::new(VgaText::new()));
        for value in [0x00, 0x29, 0x09, 0xFF] {
            bus.port_write(CGA_MODE_CONTROL_PORT, 1, value).unwrap();
            assert_eq!(
                bus.device().text_scanout().unwrap().attributes,
                device::TextAttributeSet::Colour {
                    blink_enabled: value & 0x20 != 0
                }
            );
        }
    }

    #[test]
    fn fidelity_reserved_crtc_indices_do_not_alias_registers() {
        let mut bus = MockBus::new(Box::new(VgaText::new()));
        for index in 25..32 {
            bus.port_write(CRTC_INDEX_PORT, 1, index - 25).unwrap();
            bus.port_write(CRTC_DATA_PORT, 1, 0x5A).unwrap();
            bus.port_write(CRTC_INDEX_PORT, 1, index).unwrap();
            bus.port_write(CRTC_DATA_PORT, 1, 0xEE).unwrap();
            assert_eq!(bus.port_read(CRTC_DATA_PORT, 1).unwrap(), 0);
            bus.port_write(CRTC_INDEX_PORT, 1, index - 25).unwrap();
            assert_eq!(bus.port_read(CRTC_DATA_PORT, 1).unwrap(), 0x5A);
        }
        bus.port_write(CRTC_INDEX_PORT, 1, 0xEC).unwrap();
        assert_eq!(bus.port_read(CRTC_INDEX_PORT, 1).unwrap(), 0x0C);
    }

    #[test]
    fn observation_scanout_is_passive_repeatable_and_tracks_crtc_state() {
        let mut bus = MockBus::new(Box::new(crate::trace_policy_tests::traced(VgaText::new())));
        bus.tick(37);
        bus.mmio_write(TEXT_BUFFER_BASE + 0x240, 2, 0x1F41).unwrap();
        for (index, value) in [(0x0C, 1), (0x0D, 0x20), (0x0E, 1), (0x0F, 0x22)] {
            bus.port_write(CRTC_INDEX_PORT, 1, index).unwrap();
            bus.port_write(CRTC_DATA_PORT, 1, value).unwrap();
        }
        let saved = bus.device().save();
        let clock = bus.clock();
        assert!(bus.device().text_scanout().is_some());
        let expected_trace = bus.device_mut().take_trace();
        let first = bus.device().text_scanout().unwrap();
        assert_eq!((first.cols, first.rows), (COLS, ROWS));
        assert_eq!((first.start_cell, first.cursor_cell), (0x120, 0x122));
        assert_eq!(&first.vram[0x240..0x242], &[0x41, 0x1F]);
        assert!(first.cursor_visible);
        assert_eq!(first, bus.device().text_scanout().unwrap());
        assert_eq!(bus.device().save(), saved);
        assert_eq!(bus.clock(), clock);
        assert!(bus.device_mut().take_trace().is_empty());
        assert_eq!(expected_trace.len(), 9);
        // A scanout must not advance the per-read status phase.
        assert_eq!(bus.port_read(CGA_STATUS_PORT, 1).unwrap(), 0);
        bus.port_write(CRTC_INDEX_PORT, 1, 0x0A | 0xE0).unwrap();
        bus.port_write(CRTC_DATA_PORT, 1, 0x20).unwrap();
        assert!(!bus.device().text_scanout().unwrap().cursor_visible);
        bus.device_mut().restore(&saved).unwrap();
        assert!(bus.device().text_scanout().unwrap().cursor_visible);
    }

    /// MMIO address of text cell `n`.
    fn cell_addr(n: u64) -> u64 {
        TEXT_BUFFER_BASE + n * 2
    }

    #[test]
    fn dormant_matches_per_clock_seeded_accesses_and_long_jumps() {
        let mut reference =
            MockBus::new(Box::new(crate::trace_policy_tests::traced(VgaText::new())));
        let mut scheduled =
            MockBus::new(Box::new(crate::trace_policy_tests::traced(VgaText::new())));
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
            match seed % 4 {
                0 => {
                    let addr = TEXT_BUFFER_BASE + ((seed >> 8) % TEXT_BUFFER_LEN as u64);
                    reference.mmio_write(addr, 2, seed).unwrap();
                    scheduled.mmio_write(addr, 2, seed).unwrap();
                    assert_eq!(
                        reference.mmio_read(addr, 2).unwrap(),
                        scheduled.mmio_read(addr, 2).unwrap()
                    );
                }
                1 => {
                    for port in [
                        CRTC_INDEX_PORT,
                        CRTC_DATA_PORT,
                        CGA_MODE_CONTROL_PORT,
                        CGA_COLOR_SELECT_PORT,
                    ] {
                        reference.port_write(port, 1, seed as u32).unwrap();
                        scheduled.port_write(port, 1, seed as u32).unwrap();
                        assert_eq!(
                            reference.port_read(port, 1).unwrap(),
                            scheduled.port_read(port, 1).unwrap()
                        );
                    }
                }
                _ => {
                    for port in [CGA_STATUS_PORT; 4] {
                        assert_eq!(
                            reference.port_read(port, 1).unwrap(),
                            scheduled.port_read(port, 1).unwrap()
                        );
                    }
                }
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
        // 0x3DB is between the CRTC pair and the CGA registers and belongs to no
        // register the card decodes.
        assert_eq!(
            vga.port_read(0x3DB, 1).unwrap_err(),
            DeviceError::UnmappedRegister {
                kind: "port",
                offset: 0x3DB,
            }
        );
    }

    #[test]
    fn cga_mode_and_color_registers_round_trip() {
        let mut vga = VgaText::new();
        // Mode 3 as a stock BIOS programs it, plus a colour-select value.
        vga.port_write(CGA_MODE_CONTROL_PORT, 1, 0x29).unwrap();
        vga.port_write(CGA_COLOR_SELECT_PORT, 1, 0x30).unwrap();
        assert_eq!(vga.port_read(CGA_MODE_CONTROL_PORT, 1).unwrap(), 0x29);
        assert_eq!(vga.port_read(CGA_COLOR_SELECT_PORT, 1).unwrap(), 0x30);
    }

    #[test]
    fn writing_status_register_is_ignored_not_faulted() {
        let mut vga = VgaText::new();
        // Read-only register: a write must be accepted (no fault) and change
        // nothing the guest can observe on read-back.
        vga.port_write(CGA_STATUS_PORT, 1, 0xFF).unwrap();
    }

    #[test]
    fn status_retrace_bit_sees_both_edges_within_bounded_reads() {
        // Model the BIOS busy-wait: spin until the vertical-retrace bit is high,
        // then spin until it is low again. This isolated test deliberately omits
        // ticks to verify the compatibility model toggles the bit per read.
        let mut vga = VgaText::new();
        // Vertical-retrace bit (bit 3) — the bit the BIOS's `test al,8` checks.
        const RETRACE: u32 = 0x08;
        let mut reads = 0u32;
        // Wait for retrace high.
        loop {
            reads += 1;
            assert!(reads < 100, "retrace bit never went high");
            if vga.port_read(CGA_STATUS_PORT, 1).unwrap() & RETRACE != 0 {
                break;
            }
        }
        // Wait for retrace low again (the second edge).
        loop {
            reads += 1;
            assert!(reads < 100, "retrace bit never went low");
            if vga.port_read(CGA_STATUS_PORT, 1).unwrap() & RETRACE == 0 {
                break;
            }
        }
    }

    #[test]
    fn status_display_enable_bit_toggles_each_read() {
        let mut vga = VgaText::new();
        let r0 = vga.port_read(CGA_STATUS_PORT, 1).unwrap() & 0x01;
        let r1 = vga.port_read(CGA_STATUS_PORT, 1).unwrap() & 0x01;
        assert_ne!(r0, r1, "display-enable bit must change between reads");
    }

    #[test]
    fn cga_registers_survive_save_restore() {
        let mut vga = VgaText::new();
        vga.port_write(CGA_MODE_CONTROL_PORT, 1, 0x29).unwrap();
        vga.port_write(CGA_COLOR_SELECT_PORT, 1, 0x0F).unwrap();
        let blob = vga.save();
        let mut restored = VgaText::new();
        restored.restore(&blob).unwrap();
        assert_eq!(restored.port_read(CGA_MODE_CONTROL_PORT, 1).unwrap(), 0x29);
        assert_eq!(restored.port_read(CGA_COLOR_SELECT_PORT, 1).unwrap(), 0x0F);
    }

    #[test]
    fn no_irq_is_asserted() {
        let vga = VgaText::new();
        assert!(!vga.irq_pending());
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
        let mut bus = MockBus::new(Box::new(crate::trace_policy_tests::traced(VgaText::new())));
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
