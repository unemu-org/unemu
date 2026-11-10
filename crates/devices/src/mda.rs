// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM Monochrome Display and Printer Adapter, display portion (80×25).
//!
//! Sources: IBM PC Technical Reference, Options and Adapters, **IBM Monochrome
//! Display and Printer Adapter**, §Monochrome Display Adapter Function,
//! Description (pp. 1–3), Programming Considerations (pp. 5–8), and Logic
//! Diagrams; Motorola **MC6845 CRTC** data sheet, §Register File Descriptions,
//! Table 2 (register access), and §Cursor Control, Table 4 (cursor modes).
//! See `backlog/docs/design/spec.md` §7.1 and §9, and
//! `backlog/docs/design/observation.md` §Typed capabilities and route ownership.
//!
//! Hardware coverage matrix (`backlog/docs/testing.md` §3):
//!
//! | Source defines | Modelled | Skipped / approximation |
//! | --- | --- | --- |
//! | IBM Description, Programming Considerations, Logic Diagrams: display RAM | 4 KiB, glyph/attribute pairs, low 12 address bits mirrored in B0000–B7FFF | Memory contention and wait states |
//! | IBM Programming Considerations p. 7, Logic Diagrams: I/O decode | Even 3B0–3B6 select the index; odd 3B1–3B7 access data | None |
//! | MC6845 Table 2: index and R0–R17 | Five-bit index; R0–R13 write-only, R14/R15 read/write | None |
//! | IBM Description p. 1: no light pen, R16/R17 reserved | R16/R17 stay zero; writes ignored | None |
//! | IBM Programming Considerations p. 7: 3B0–3BF decoded; the index and mode registers are write-only, 3B9/3BB/3BF unassigned | Assumption: reads return FF and unassigned-port writes are ignored, following the project's open-bus convention | Electrical bus behaviour |
//! | IBM Programming Considerations p. 7: status defines bits 0 and 3 only | Assumption: bits 1, 2, 4–7 read 1, following the project's open-bus convention | Electrical bus behaviour |
//! | MC6845 Table 2: write-only and unassigned data-register reads undefined | Assumption: return 00, ignore unassigned writes | No hardware-defined read value |
//! | MC6845 Start Address, Cursor Location | 14-bit cell addresses, exposed without wrapping to VRAM | Raster address generator; 14-bit cursor wrap (the shared renderer compares the cursor with 16-bit wrap; scanout wraps at the 2048-cell buffer) |
//! | MC6845 Cursor Control, Table 4 | R10 bits 6:5 = 01 disables cursor | Shape and blink timing; both blink modes stay visible |
//! | IBM Programming Considerations: 3B8 | Stored mode byte; high-resolution, video-enable and blink getters; bit 5 feeds the scanout blink flag | No access stall when high-resolution is clear; no video blanking or blink timing |
//! | IBM Programming Considerations: 3BA | Horizontal drive (bit 0), black/white video (bit 3) | Per-read phase, not electrical video or raster timing |
//! | MC6845 timing registers | R0–R11 stored as bytes | Timing, interlace, unused-bit masks and autonomous events; geometry from R1/R6/R9 is not honoured (fixed 80×25) |
//! | IBM Description: character generator; Programming Considerations p. 6: attributes | 80×25 monochrome scanout; shared renderer decodes non-display, underline, intensity, reverse video and blink/background intensity | Font rasterisation and character blink timing |
//! | IBM Printer Adapter Function | None | Ports 3BC–3BE and printer IRQ; machine profiles must not route these ports to this display device |
//!
//! Undriven port reads follow the project's open-bus convention (FF). Reads
//! through the CRTC data port for write-only or unassigned indices instead
//! return zero as an assumption: the MC6845 data sheet defines no read value.
//! Invalid indices do not alias registers. Writes to read-only registers are
//! accepted and ignored. Like [`crate::VgaText`], multi-byte MMIO is little-endian;
//! trailing bytes outside the window read as zero and writes are discarded,
//! while bytes crossing a 4 KiB mirror wrap. Widths outside 1–8 bytes are rejected
//! with `DeviceError::UnmappedRegister`, the existing access-error variant.
//!
//! Status uses the same per-read phase as [`crate::VgaText`]'s colour status
//! register (`0x3DA`): bit 0 toggles every read and bit 3 every second read, independent of clocks or mode. BIOS polling
//! sees both edges within two/four reads, even without ticking. Scanout is passive
//! and does not advance that phase. Snapshots preserve the phase as well as RAM
//! and registers, including the absolute trace clock. Run-scoped trace policy,
//! history and drop counters are excluded and preserved on restore.
//! Reset preserves the trace policy and clears hardware state and trace history
//! for repeatability, rather than reproducing undefined power-on RAM or the
//! MC6845 RESET pin's retention of programmed registers.

use device::{AccessDir, Device, DeviceError, NextEvent, TextModeState, TraceEvent, VClock};

const NAME: &str = "mda-text";
/// Visible text columns.
pub const COLS: u16 = 80;
/// Visible text rows.
pub const ROWS: u16 = 25;
/// Base of the monochrome text MMIO window.
pub const TEXT_BUFFER_BASE: u64 = 0xB_0000;
/// Decoded MMIO window length, including mirrors.
pub const TEXT_BUFFER_LEN: usize = 0x8000;
/// Physical display RAM length, shared by all eight mirrors.
pub const VRAM_LEN: usize = 0x1000;
/// First display port, including the MC6845 register mirrors.
/// IBM PC Technical Reference, Monochrome Display and Printer Adapter,
/// Programming Considerations, I/O address map.
pub const DISPLAY_PORT_BASE: u16 = 0x3B0;
/// Last display port before the printer function's ports.
pub const DISPLAY_PORT_TOP: u16 = 0x3BB;
/// Additional unassigned display port after the printer function's ports.
pub const UNASSIGNED_PORT: u16 = 0x3BF;
/// MC6845 write-only register selector.
pub const CRTC_INDEX_PORT: u16 = 0x3B4;
/// MC6845 selected-register data port.
pub const CRTC_DATA_PORT: u16 = 0x3B5;
/// Write-only CRT mode control port.
pub const MODE_CONTROL_PORT: u16 = 0x3B8;
/// Read-only CRT status port.
pub const STATUS_PORT: u16 = 0x3BA;

const CRTC_REG_COUNT: usize = 18;
const SNAPSHOT_VERSION: u8 = 1;
const SNAPSHOT_CRTC_INDEX: usize = 1;
const SNAPSHOT_CRTC_REGS: usize = SNAPSHOT_CRTC_INDEX + 1;
const SNAPSHOT_MODE_CONTROL: usize = SNAPSHOT_CRTC_REGS + CRTC_REG_COUNT;
const SNAPSHOT_STATUS_READS: usize = SNAPSHOT_MODE_CONTROL + 1;
const SNAPSHOT_CLOCK: usize = SNAPSHOT_STATUS_READS + 8;
const SNAPSHOT_HEADER_LEN: usize = SNAPSHOT_CLOCK + 8;

/// MDA display RAM and MC6845 registers behind the [`Device`] seam (§7.1, §9).
///
/// The printer portion is absent. Presentation consumes [`Device::text_scanout`]
/// without depending on this concrete card.
pub struct MdaText {
    vram: Vec<u8>,
    crtc_index: u8,
    crtc: [u8; CRTC_REG_COUNT],
    mode_control: u8,
    status_reads: u64,
    trace: device::TraceBuffer,
    now: VClock,
}

impl MdaText {
    /// Create a card with cleared memory, registers and status phase.
    #[must_use]
    pub fn new() -> Self {
        Self {
            vram: vec![0; VRAM_LEN],
            crtc_index: 0,
            crtc: [0; CRTC_REG_COUNT],
            mode_control: 0,
            status_reads: 0,
            trace: device::TraceBuffer::default(),
            now: VClock::ZERO,
        }
    }

    /// Borrow physical text RAM (glyph then attribute per cell).
    #[must_use]
    pub fn vram(&self) -> &[u8] {
        &self.vram
    }

    /// Display start address in cells, from R12/R13.
    #[must_use]
    pub fn start_cell(&self) -> u16 {
        (u16::from(self.crtc[12] & 0x3F) << 8) | u16::from(self.crtc[13])
    }

    /// Absolute cursor location in cells, from R14/R15.
    #[must_use]
    pub fn cursor_cell(&self) -> u16 {
        (u16::from(self.crtc[14] & 0x3F) << 8) | u16::from(self.crtc[15])
    }

    /// Whether R10 selects a visible cursor mode (blink is not timed).
    #[must_use]
    pub fn cursor_visible(&self) -> bool {
        self.crtc[10] & 0x60 != 0x20
    }

    /// Last mode-control byte written, including unused bits.
    #[must_use]
    pub fn mode_control(&self) -> u8 {
        self.mode_control
    }

    /// High-resolution mode selection (mode-control bit 0).
    #[must_use]
    pub fn high_resolution(&self) -> bool {
        self.mode_control & 0x01 != 0
    }

    /// Stored video-enable selection (mode-control bit 3).
    #[must_use]
    pub fn video_enabled(&self) -> bool {
        self.mode_control & 0x08 != 0
    }

    /// Stored character-blink selection (mode-control bit 5).
    #[must_use]
    pub fn blink_enabled(&self) -> bool {
        self.mode_control & 0x20 != 0
    }

    fn status_register(&mut self) -> u8 {
        let phase = self.status_reads;
        self.status_reads = phase.wrapping_add(1);
        0xF6 | (phase as u8 & 1) | ((phase as u8 & 2) << 2)
    }

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

    fn trace_port(&mut self, port: u16, size: u8, dir: AccessDir, value: u32) {
        self.trace.push(TraceEvent::Port {
            at: self.now,
            port,
            size,
            dir,
            value,
        });
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
}

impl Default for MdaText {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for MdaText {
    fn name(&self) -> &'static str {
        NAME
    }

    fn text_scanout(&self) -> Option<TextModeState<'_>> {
        Some(TextModeState {
            vram: self.vram(),
            cols: COLS,
            rows: ROWS,
            start_cell: self.start_cell(),
            cursor_cell: self.cursor_cell(),
            cursor_visible: self.cursor_visible(),
            attributes: device::TextAttributeSet::Monochrome {
                blink_enabled: self.blink_enabled(),
            },
        })
    }

    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        let value = match port {
            0x3B0 | 0x3B2 | CRTC_INDEX_PORT | 0x3B6 | MODE_CONTROL_PORT | 0x3B9 | 0x3BB | 0x3BF => {
                0xFF
            }
            0x3B1 | 0x3B3 | CRTC_DATA_PORT | 0x3B7 => match self.crtc_index {
                14..=17 => u32::from(self.crtc[usize::from(self.crtc_index)]),
                _ => 0,
            },
            STATUS_PORT => u32::from(self.status_register()),
            _ => {
                return Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(port),
                });
            }
        };
        self.trace_port(port, size, AccessDir::Read, value);
        Ok(value)
    }

    fn port_write(&mut self, port: u16, size: u8, value: u32) -> Result<(), DeviceError> {
        let byte = value as u8;
        match port {
            0x3B0 | 0x3B2 | CRTC_INDEX_PORT | 0x3B6 => self.crtc_index = byte & 0x1F,
            0x3B1 | 0x3B3 | CRTC_DATA_PORT | 0x3B7 => match self.crtc_index {
                12 | 14 => self.crtc[usize::from(self.crtc_index)] = byte & 0x3F,
                0..=15 => self.crtc[usize::from(self.crtc_index)] = byte,
                _ => {}
            },
            MODE_CONTROL_PORT => self.mode_control = byte,
            STATUS_PORT | 0x3B9 | 0x3BB | 0x3BF => {}
            _ => {
                return Err(DeviceError::UnmappedRegister {
                    kind: "port",
                    offset: u64::from(port),
                });
            }
        }
        self.trace_port(port, size, AccessDir::Write, value);
        Ok(())
    }

    fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
        let offset = self.mmio_offset(addr, size)?;
        let mut value = 0;
        for i in 0..usize::from(size) {
            if offset + i < TEXT_BUFFER_LEN {
                value |= u64::from(self.vram[(offset + i) & (VRAM_LEN - 1)]) << (8 * i);
            }
        }
        self.trace_mmio(addr, size, AccessDir::Read, value);
        Ok(value)
    }

    fn mmio_write(&mut self, addr: u64, size: u8, value: u64) -> Result<(), DeviceError> {
        let offset = self.mmio_offset(addr, size)?;
        for i in 0..usize::from(size) {
            if offset + i < TEXT_BUFFER_LEN {
                self.vram[(offset + i) & (VRAM_LEN - 1)] = (value >> (8 * i)) as u8;
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
        false
    }

    fn reset(&mut self) {
        let trace_policy = self.trace.policy();
        *self = Self::new();
        self.trace.set_policy(trace_policy);
    }

    fn save(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(SNAPSHOT_HEADER_LEN + VRAM_LEN);
        blob.push(SNAPSHOT_VERSION);
        blob.push(self.crtc_index);
        blob.extend_from_slice(&self.crtc);
        blob.push(self.mode_control);
        blob.extend_from_slice(&self.status_reads.to_le_bytes());
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        blob.extend_from_slice(&self.vram);
        blob
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), DeviceError> {
        if snapshot.len() != SNAPSHOT_HEADER_LEN + VRAM_LEN {
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
        if [12, 14]
            .iter()
            .any(|&i| snapshot[SNAPSHOT_CRTC_REGS + i] & 0xC0 != 0)
            || snapshot[SNAPSHOT_CRTC_REGS + 16..SNAPSHOT_MODE_CONTROL] != [0, 0]
        {
            return Err(DeviceError::InvalidSnapshot {
                device: NAME,
                reason: "invalid CRTC register state",
            });
        }
        self.crtc_index = snapshot[SNAPSHOT_CRTC_INDEX];
        self.crtc
            .copy_from_slice(&snapshot[SNAPSHOT_CRTC_REGS..SNAPSHOT_MODE_CONTROL]);
        self.mode_control = snapshot[SNAPSHOT_MODE_CONTROL];
        self.status_reads =
            u64::from_le_bytes(std::array::from_fn(|i| snapshot[SNAPSHOT_STATUS_READS + i]));
        self.now = VClock(u64::from_le_bytes(std::array::from_fn(|i| {
            snapshot[SNAPSHOT_CLOCK + i]
        })));
        self.vram.copy_from_slice(&snapshot[SNAPSHOT_HEADER_LEN..]);
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

    fn bus() -> MockBus {
        MockBus::new(Box::new(MdaText::new()))
    }

    fn traced_bus() -> MockBus {
        MockBus::new(Box::new(crate::trace_policy_tests::traced(MdaText::new())))
    }

    fn write_crtc(bus: &mut MockBus, index: u8, value: u8) {
        bus.port_write(CRTC_INDEX_PORT, 1, u32::from(index))
            .unwrap();
        bus.port_write(CRTC_DATA_PORT, 1, u32::from(value)).unwrap();
    }

    #[test]
    fn scanout_reports_monochrome_attributes_and_mode_control_blink_selection() {
        let mut bus = bus();
        for value in [0x00, 0x29, 0x09, 0xFF] {
            bus.port_write(MODE_CONTROL_PORT, 1, value).unwrap();
            assert_eq!(
                bus.device().text_scanout().unwrap().attributes,
                device::TextAttributeSet::Monochrome {
                    blink_enabled: value & 0x20 != 0
                }
            );
        }
    }

    #[test]
    fn word_write_stores_little_endian_glyph_and_attribute() {
        let mut bus = bus();
        bus.mmio_write(TEXT_BUFFER_BASE, 2, 0x0741).unwrap();
        assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE, 2).unwrap(), 0x0741);
        assert_eq!(&bus.device().text_scanout().unwrap().vram[..2], &[b'A', 7]);
    }

    #[test]
    fn byte_writes_address_glyph_and_attribute_independently() {
        let mut bus = bus();
        bus.mmio_write(TEXT_BUFFER_BASE + 6, 1, u64::from(b'Z'))
            .unwrap();
        bus.mmio_write(TEXT_BUFFER_BASE + 7, 1, 0x70).unwrap();
        assert_eq!(
            bus.mmio_read(TEXT_BUFFER_BASE + 6, 1).unwrap(),
            u64::from(b'Z')
        );
        assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE + 7, 1).unwrap(), 0x70);
        assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE + 6, 2).unwrap(), 0x705A);
    }

    #[test]
    fn all_eight_mirrors_share_the_same_four_kibibytes() {
        let mut bus = bus();
        for writer in 0..8 {
            for offset in [0, 1, 0x72, 0xFFE, 0xFFF] {
                let value = writer * 17 + offset % 17;
                bus.mmio_write(TEXT_BUFFER_BASE + writer * 0x1000 + offset, 1, value)
                    .unwrap();
                for reader in 0..8 {
                    assert_eq!(
                        bus.mmio_read(TEXT_BUFFER_BASE + reader * 0x1000 + offset, 1)
                            .unwrap(),
                        value
                    );
                }
            }
        }
        assert_eq!(bus.device().text_scanout().unwrap().vram.len(), 4096);
        bus.mmio_write(TEXT_BUFFER_BASE, 1, 0x12).unwrap();
        bus.mmio_write(TEXT_BUFFER_BASE + 0x800, 1, 0x34).unwrap();
        assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE, 1).unwrap(), 0x12);
        assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE + 0x800, 1).unwrap(), 0x34);
    }

    #[test]
    fn word_access_wraps_at_mirror_boundary_but_not_window_end() {
        let mut bus = bus();
        bus.mmio_write(TEXT_BUFFER_BASE + 0xFFF, 2, 0x1234).unwrap();
        assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE + 0x7FFF, 1).unwrap(), 0x34);
        assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE, 1).unwrap(), 0x12);
        assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE + 0x1FFF, 2).unwrap(), 0x1234);
        bus.mmio_write(TEXT_BUFFER_BASE + 0x7FFF, 2, 0x5678)
            .unwrap();
        assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE + 0x7FFF, 2).unwrap(), 0x78);
        assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE, 1).unwrap(), 0x12);
    }

    #[test]
    fn accesses_starting_outside_the_mmio_window_are_rejected() {
        let mut bus = traced_bus();
        for addr in [
            TEXT_BUFFER_BASE - 1,
            TEXT_BUFFER_BASE + TEXT_BUFFER_LEN as u64,
            u64::MAX,
        ] {
            for size in [1, 2] {
                let error = DeviceError::UnmappedRegister {
                    kind: "mmio",
                    offset: addr,
                };
                assert_eq!(bus.mmio_read(addr, size).unwrap_err(), error);
                assert_eq!(bus.mmio_write(addr, size, 0).unwrap_err(), error);
            }
        }
        assert!(bus.device_mut().take_trace().is_empty());
    }

    #[test]
    fn invalid_mmio_widths_are_rejected_without_state_changes_or_trace() {
        let mut bus = traced_bus();
        let saved = bus.device().save();
        for size in [0, 9, 16, u8::MAX] {
            let error = DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: TEXT_BUFFER_BASE,
            };
            assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE, size).unwrap_err(), error);
            assert_eq!(
                bus.mmio_write(TEXT_BUFFER_BASE, size, u64::MAX)
                    .unwrap_err(),
                error
            );
            assert_eq!(bus.device().save(), saved);
        }
        assert!(bus.device_mut().take_trace().is_empty());
        bus.mmio_write(TEXT_BUFFER_BASE, 8, 0x0123_4567_89AB_CDEF)
            .unwrap();
        assert_eq!(
            bus.mmio_read(TEXT_BUFFER_BASE, 8).unwrap(),
            0x0123_4567_89AB_CDEF
        );
    }

    #[test]
    fn crtc_port_mirrors_share_one_selector_and_register_file() {
        let mut bus = bus();
        for index_port in [0x3B0, 0x3B2, 0x3B4, 0x3B6] {
            for data_port in [0x3B1, 0x3B3, 0x3B5, 0x3B7] {
                bus.port_write(index_port, 1, 14).unwrap();
                bus.port_write(data_port, 1, 0xFF).unwrap();
                bus.port_write(index_port, 1, 15).unwrap();
                bus.port_write(data_port, 1, 0x57).unwrap();
                for reader in [0x3B1, 0x3B3, 0x3B5, 0x3B7] {
                    assert_eq!(bus.port_read(reader, 1).unwrap(), 0x57);
                }
                assert_eq!(bus.device().text_scanout().unwrap().cursor_cell, 0x3F57);
                assert_eq!(bus.port_read(index_port, 1).unwrap(), 0xFF);
            }
        }
    }

    #[test]
    fn undriven_decoded_ports_read_open_bus_and_ignore_writes() {
        let mut bus = bus();
        write_crtc(&mut bus, 15, 0x57);
        let saved = bus.device().save();
        for port in [0x3B9, 0x3BB, 0x3BF] {
            bus.port_write(port, 1, 0xFF).unwrap();
            assert_eq!(bus.port_read(port, 1).unwrap(), 0xFF);
            assert_eq!(bus.device().save(), saved);
        }
    }

    #[test]
    fn snapshot_round_trip_preserves_extreme_legal_crtc_state() {
        let mut bus = bus();
        write_crtc(&mut bus, 12, 0xFF);
        write_crtc(&mut bus, 14, 0xFF);
        bus.port_write(CRTC_INDEX_PORT, 1, 0x1F).unwrap();
        let saved = bus.device().save();
        assert_eq!(saved[SNAPSHOT_CRTC_REGS + 12], 0x3F);
        assert_eq!(saved[SNAPSHOT_CRTC_REGS + 14], 0x3F);
        assert_eq!(saved[SNAPSHOT_CRTC_INDEX], 0x1F);
        let mut restored = MdaText::new();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.save(), saved);
    }

    #[test]
    fn crtc_addresses_round_trip_through_cursor_reads_and_scanout() {
        let mut bus = bus();
        for (index, value) in [(12, 0xC1), (13, 0x40), (14, 0xC2), (15, 0x57)] {
            write_crtc(&mut bus, index, value);
        }
        let state = bus.device().text_scanout().unwrap();
        assert_eq!(state.start_cell, 0x0140);
        assert_eq!(state.cursor_cell, 0x0257);
        assert_eq!(bus.port_read(CRTC_DATA_PORT, 1).unwrap(), 0x57);
        bus.port_write(CRTC_INDEX_PORT, 1, 14).unwrap();
        assert_eq!(bus.port_read(CRTC_DATA_PORT, 1).unwrap(), 2);
        // R12/R13 are write-only; their stored address is observed via scanout.
        for index in [12, 13] {
            bus.port_write(CRTC_INDEX_PORT, 1, index).unwrap();
            assert_eq!(bus.port_read(CRTC_DATA_PORT, 1).unwrap(), 0);
        }
    }

    #[test]
    fn crtc_write_only_and_read_only_permissions_are_respected() {
        let mut bus = bus();
        for index in 0..14 {
            write_crtc(&mut bus, index, 0x15);
            assert_eq!(bus.port_read(CRTC_DATA_PORT, 1).unwrap(), 0);
            assert_eq!(bus.port_read(CRTC_INDEX_PORT, 1).unwrap(), 0xFF);
        }
        let saved = bus.device().save();
        assert_eq!(&saved[2..16], &[0x15; 14]);
        for index in [16, 17] {
            write_crtc(&mut bus, index, 0xFF);
            assert_eq!(bus.port_read(CRTC_DATA_PORT, 1).unwrap(), 0);
        }
    }

    #[test]
    fn invalid_crtc_indices_do_not_alias_and_selector_uses_five_bits() {
        let mut bus = bus();
        write_crtc(&mut bus, 15, 0x47);
        let before = bus.device().save();
        for index in 18..32 {
            write_crtc(&mut bus, index, 0xFF);
            assert_eq!(bus.port_read(CRTC_DATA_PORT, 1).unwrap(), 0);
            assert_eq!(&bus.device().save()[2..], &before[2..]);
        }
        write_crtc(&mut bus, 0xEF, 0x56);
        assert_eq!(bus.port_read(CRTC_DATA_PORT, 1).unwrap(), 0x56);
        assert_eq!(bus.device().text_scanout().unwrap().cursor_cell, 0x56);
    }

    #[test]
    fn cursor_disable_encoding_hides_only_the_non_display_mode() {
        let mut bus = bus();
        for (value, visible) in [(0x00, true), (0x20, false), (0x40, true), (0x60, true)] {
            write_crtc(&mut bus, 10, value | 0x0B);
            assert_eq!(bus.device().text_scanout().unwrap().cursor_visible, visible);
        }
    }

    #[test]
    fn mode_control_is_write_only_and_getters_expose_stored_bits() {
        let mut bus = bus();
        for value in [0, 1, 8, 0x20, 0x29, 0xFF] {
            bus.port_write(MODE_CONTROL_PORT, 1, value).unwrap();
            assert_eq!(bus.port_read(MODE_CONTROL_PORT, 1).unwrap(), 0xFF);
            let mut card = MdaText::default();
            card.restore(&bus.device().save()).unwrap();
            assert_eq!(u32::from(card.mode_control()), value);
            assert_eq!(card.high_resolution(), value & 1 != 0);
            assert_eq!(card.video_enabled(), value & 8 != 0);
            assert_eq!(card.blink_enabled(), value & 0x20 != 0);
        }
    }

    #[test]
    fn status_sees_both_edges_within_bounded_reads_without_ticking() {
        let mut bus = bus();
        let values: Vec<_> = (0..12)
            .map(|_| bus.port_read(STATUS_PORT, 1).unwrap())
            .collect();
        assert_eq!(
            values,
            [0xF6, 0xF7, 0xFE, 0xFF, 0xF6, 0xF7, 0xFE, 0xFF, 0xF6, 0xF7, 0xFE, 0xFF]
        );
    }

    #[test]
    fn status_writes_are_ignored_without_advancing_the_read_phase() {
        let mut bus = bus();
        bus.port_read(STATUS_PORT, 1).unwrap();
        let before = bus.device().save();
        bus.port_write(STATUS_PORT, 1, 0xFF).unwrap();
        assert_eq!(bus.device().save(), before);
        assert_eq!(bus.port_read(STATUS_PORT, 1).unwrap(), 0xF7);
    }

    #[test]
    fn printer_and_outside_ports_are_unmapped() {
        let mut bus = traced_bus();
        for port in [
            0x3AF, 0x3BC, 0x3BD, 0x3BE, 0x3C0, 0x3D4, 0x3D5, 0x3D8, 0x3DA,
        ] {
            let error = DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            };
            assert_eq!(bus.port_read(port, 1).unwrap_err(), error);
            assert_eq!(bus.port_write(port, 1, 0).unwrap_err(), error);
        }
        assert!(bus.device_mut().take_trace().is_empty());
    }

    #[test]
    fn scanout_reflects_memory_and_crtc_writes() {
        let mut bus = bus();
        bus.mmio_write(TEXT_BUFFER_BASE + 0x1240, 2, 0x0741)
            .unwrap();
        for (index, value) in [(12, 1), (13, 0x20), (14, 1), (15, 0x22)] {
            write_crtc(&mut bus, index, value);
        }
        let first = bus.device().text_scanout().unwrap();
        assert_eq!((first.cols, first.rows), (80, 25));
        assert_eq!((first.start_cell, first.cursor_cell), (0x120, 0x122));
        assert_eq!(&first.vram[0x240..0x242], &[b'A', 7]);
        assert!(first.cursor_visible);
    }

    #[test]
    fn snapshot_restores_memory_registers_selector_and_status_phase() {
        let mut original = traced_bus();
        original.tick(137);
        original
            .mmio_write(TEXT_BUFFER_BASE + 0x1234, 2, 0x7042)
            .unwrap();
        for index in 0..16 {
            write_crtc(&mut original, index, index + 1);
        }
        original.port_write(MODE_CONTROL_PORT, 1, 0x29).unwrap();
        for _ in 0..3 {
            original.port_read(STATUS_PORT, 1).unwrap();
        }
        let saved = original.device().save();
        let mut restored = traced_bus();
        restored.device_mut().restore(&saved).unwrap();
        original.device_mut().take_trace();
        assert!(restored.device_mut().take_trace().is_empty());
        assert_eq!(restored.device().save(), saved);
        assert_eq!(
            restored.device().text_scanout(),
            original.device().text_scanout()
        );
        assert_eq!(restored.port_read(CRTC_DATA_PORT, 1).unwrap(), 16);
        assert_eq!(original.port_read(CRTC_DATA_PORT, 1).unwrap(), 16);
        for _ in 0..8 {
            assert_eq!(
                restored.port_read(STATUS_PORT, 1).unwrap(),
                original.port_read(STATUS_PORT, 1).unwrap()
            );
        }
        for bus in [&mut original, &mut restored] {
            assert_eq!(bus.mmio_read(TEXT_BUFFER_BASE + 0x234, 2).unwrap(), 0x7042);
        }
        assert_eq!(original.device().save(), restored.device().save());
        let expected = original.device_mut().take_trace();
        assert_eq!(restored.device_mut().take_trace(), expected);
        assert!(expected.iter().all(|event| match event {
            TraceEvent::Mmio { at, .. } | TraceEvent::Port { at, .. } => *at == VClock(137),
        }));
    }

    #[test]
    fn malformed_snapshots_are_rejected_without_mutating_state() {
        let mut bus = bus();
        bus.mmio_write(TEXT_BUFFER_BASE, 2, 0x0741).unwrap();
        bus.tick(23);
        write_crtc(&mut bus, 15, 0x57);
        let saved = bus.device().save();
        for length in 0..saved.len() {
            assert!(matches!(
                bus.device_mut().restore(&saved[..length]),
                Err(DeviceError::InvalidSnapshot { device: NAME, .. })
            ));
            assert_eq!(bus.device().save(), saved);
        }
        let mut bad_version = saved.clone();
        bad_version[0] = SNAPSHOT_VERSION + 1;
        let mut too_long = saved.clone();
        too_long.push(0);
        for bad in [
            vec![],
            vec![SNAPSHOT_VERSION],
            saved[..saved.len() - 1].to_vec(),
            too_long,
            bad_version,
        ] {
            assert!(matches!(
                bus.device_mut().restore(&bad),
                Err(DeviceError::InvalidSnapshot { device: NAME, .. })
            ));
            assert_eq!(bus.device().save(), saved);
        }
        for (offset, value) in [
            (SNAPSHOT_CRTC_INDEX, 0x20),
            (SNAPSHOT_CRTC_INDEX, 0xFF),
            (SNAPSHOT_CRTC_REGS + 12, 0x40),
            (SNAPSHOT_CRTC_REGS + 14, 0x80),
            (SNAPSHOT_CRTC_REGS + 16, 1),
            (SNAPSHOT_CRTC_REGS + 17, 1),
        ] {
            let mut bad = saved.clone();
            bad[offset] = value;
            assert!(matches!(
                bus.device_mut().restore(&bad),
                Err(DeviceError::InvalidSnapshot { device: NAME, .. })
            ));
            assert_eq!(bus.device().save(), saved);
        }
    }

    #[test]
    fn snapshot_byte_mutations_reject_atomically_or_continue_without_panic() {
        let mut bus = bus();
        bus.mmio_write(TEXT_BUFFER_BASE, 2, 0x7041).unwrap();
        write_crtc(&mut bus, 15, 7);
        bus.tick(137);
        let valid = bus.device().save();
        for index in 0..valid.len() {
            for value in [0, 1, 0x7F, 0xFF] {
                let mut blob = valid.clone();
                blob[index] = value;
                bus.device_mut().restore(&valid).unwrap();
                match bus.device_mut().restore(&blob) {
                    Ok(()) => {
                        assert_eq!(bus.device().save(), blob);
                        bus.device_mut().sync(VClock(u64::MAX));
                        bus.port_read(STATUS_PORT, 1).unwrap();
                        bus.port_read(CRTC_DATA_PORT, 1).unwrap();
                        bus.port_write(CRTC_DATA_PORT, 1, 0xFF).unwrap();
                        bus.mmio_write(TEXT_BUFFER_BASE + 0x7FFF, 2, 0xFFFF)
                            .unwrap();
                        bus.device().text_scanout().unwrap();
                        assert_eq!(
                            bus.device().next_event(VClock(u64::MAX)),
                            NextEvent::Dormant
                        );
                        assert!(!bus.device().irq_pending());
                    }
                    Err(DeviceError::InvalidSnapshot { .. }) => {
                        assert_eq!(bus.device().save(), valid, "mutation at {index}");
                    }
                    Err(error) => panic!("unexpected restore error: {error}"),
                }
            }
        }
        let mut wrapped = valid;
        wrapped[SNAPSHOT_STATUS_READS..SNAPSHOT_CLOCK].copy_from_slice(&u64::MAX.to_le_bytes());
        bus.device_mut().restore(&wrapped).unwrap();
        assert_eq!(bus.port_read(STATUS_PORT, 1).unwrap(), 0xFF);
        assert_eq!(bus.port_read(STATUS_PORT, 1).unwrap(), 0xF6);
    }

    #[test]
    fn reset_clears_memory_registers_status_and_trace_without_irq() {
        let mut bus = traced_bus();
        bus.tick(42);
        bus.mmio_write(TEXT_BUFFER_BASE, 2, 0x7042).unwrap();
        write_crtc(&mut bus, 15, 7);
        bus.port_write(MODE_CONTROL_PORT, 1, 0x29).unwrap();
        bus.port_read(STATUS_PORT, 1).unwrap();
        bus.device_mut().reset();
        assert_eq!(bus.device().save(), MdaText::new().save());
        assert!(bus.device_mut().take_trace().is_empty());
        assert!(!bus.device().irq_pending());
        assert_eq!(bus.device().next_event(bus.clock()), NextEvent::Dormant);
        assert_eq!(bus.port_read(STATUS_PORT, 1).unwrap(), 0xF6);
    }

    #[test]
    fn trace_records_access_values_widths_addresses_and_clock_then_drains() {
        let mut bus = traced_bus();
        bus.tick(17);
        bus.mmio_write(TEXT_BUFFER_BASE + 0x1000, 2, 0x0741)
            .unwrap();
        bus.mmio_read(TEXT_BUFFER_BASE, 2).unwrap();
        bus.mmio_write(TEXT_BUFFER_BASE + 1, 1, 0x70).unwrap();
        bus.mmio_read(TEXT_BUFFER_BASE + 1, 1).unwrap();
        bus.port_write(STATUS_PORT, 1, 0xFF).unwrap();
        bus.port_read(STATUS_PORT, 1).unwrap();
        assert_eq!(
            bus.device_mut().take_trace(),
            vec![
                TraceEvent::Mmio {
                    at: VClock(17),
                    addr: TEXT_BUFFER_BASE + 0x1000,
                    size: 2,
                    dir: AccessDir::Write,
                    value: 0x0741
                },
                TraceEvent::Mmio {
                    at: VClock(17),
                    addr: TEXT_BUFFER_BASE,
                    size: 2,
                    dir: AccessDir::Read,
                    value: 0x0741
                },
                TraceEvent::Mmio {
                    at: VClock(17),
                    addr: TEXT_BUFFER_BASE + 1,
                    size: 1,
                    dir: AccessDir::Write,
                    value: 0x70
                },
                TraceEvent::Mmio {
                    at: VClock(17),
                    addr: TEXT_BUFFER_BASE + 1,
                    size: 1,
                    dir: AccessDir::Read,
                    value: 0x70
                },
                TraceEvent::Port {
                    at: VClock(17),
                    port: STATUS_PORT,
                    size: 1,
                    dir: AccessDir::Write,
                    value: 0xFF
                },
                TraceEvent::Port {
                    at: VClock(17),
                    port: STATUS_PORT,
                    size: 1,
                    dir: AccessDir::Read,
                    value: 0xF6
                },
            ]
        );
        assert!(bus.device_mut().take_trace().is_empty());
    }

    #[test]
    fn trace_is_off_by_default_and_ring_is_bounded_and_run_scoped() {
        let mut bus = bus();
        bus.mmio_write(TEXT_BUFFER_BASE, 2, 0x0741).unwrap();
        bus.port_read(STATUS_PORT, 1).unwrap();
        assert!(bus.device_mut().take_trace().is_empty());
        assert_eq!(bus.device().trace_dropped(), 0);
        bus.device_mut()
            .set_trace_policy(device::TracePolicy::Ring { capacity: 2 });
        bus.tick(17);
        bus.port_write(CRTC_INDEX_PORT, 1, 15).unwrap();
        bus.mmio_write(TEXT_BUFFER_BASE, 2, 0x0742).unwrap();
        bus.mmio_read(TEXT_BUFFER_BASE, 2).unwrap();
        assert_eq!(bus.device().trace_dropped(), 1);
        let saved = bus.device().save();
        bus.device_mut().restore(&saved).unwrap();
        let events = bus.device_mut().take_trace();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            TraceEvent::Mmio {
                dir: AccessDir::Write,
                ..
            }
        ));
        assert!(matches!(
            events[1],
            TraceEvent::Mmio {
                dir: AccessDir::Read,
                ..
            }
        ));
        assert_eq!(bus.device().trace_dropped(), 1);
        assert!(bus.device_mut().take_trace().is_empty());
        let mut restored = traced_bus();
        restored.device_mut().restore(&saved).unwrap();
        assert!(restored.device_mut().take_trace().is_empty());
        assert_eq!(restored.device().trace_dropped(), 0);
        bus.device_mut().reset();
        assert_eq!(bus.device().save(), MdaText::new().save());
        assert_eq!(bus.device().trace_dropped(), 0);
        bus.port_read(STATUS_PORT, 1).unwrap();
        assert_eq!(bus.device_mut().take_trace().len(), 1);
        bus.device_mut().set_trace_policy(device::TracePolicy::Off);
        bus.port_read(STATUS_PORT, 1).unwrap();
        assert!(bus.device_mut().take_trace().is_empty());
    }

    #[test]
    fn dormant_matches_per_clock_ticking() {
        let mut reference = traced_bus();
        let mut scheduled = traced_bus();
        for step in 0..128u64 {
            assert_eq!(
                scheduled.device().next_event(reference.clock()),
                NextEvent::Dormant
            );
            let clocks = if step % 31 == 0 { 100_003 } else { step % 17 };
            for _ in 0..clocks {
                reference.tick(1);
                assert!(!reference.device().irq_pending());
            }
            scheduled.device_mut().sync(reference.clock());
            for bus in [&mut reference, &mut scheduled] {
                let addr = TEXT_BUFFER_BASE + (step * 257 % TEXT_BUFFER_LEN as u64);
                bus.mmio_write(addr, 2, step * 513).unwrap();
                bus.mmio_read(addr, 2).unwrap();
                write_crtc(bus, (step % 32) as u8, step as u8);
                bus.port_read(CRTC_DATA_PORT, 1).unwrap();
                bus.port_write(MODE_CONTROL_PORT, 1, step as u32).unwrap();
                bus.port_read(STATUS_PORT, 1).unwrap();
                bus.port_read(STATUS_PORT, 1).unwrap();
            }
            assert_eq!(reference.device().save(), scheduled.device().save());
            assert_eq!(
                reference.device().text_scanout(),
                scheduled.device().text_scanout()
            );
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
}
