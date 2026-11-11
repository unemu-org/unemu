// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM VGA register file and four-plane video memory, without scanout.
//!
//! Source: IBM VGA/XGA Technical Reference Manual (May 1992), VGA Function,
//! especially the figures cited below. See `backlog/docs/design/spec.md` §7.1
//! and §9, and `backlog/docs/design/firmware-and-machine-classes.md` §3.6.
//!
//! READ means checked against the manual; DERIVED identifies address mapping
//! inferred from its figures; ASSUMED marks model choices or unspecified cases.
//!
//! | IBM figure / section | Verified coverage | Limit or model choice |
//! | --- | --- | --- |
//! | Figs. 2-19–2-26: general registers | READ: port directions and MSR bit 0 select CRTC, status 1 and feature-control addresses | ASSUMED: inactive pair reads FF; status 0 reads 00; reserved MSR/feature bits retain writes. RAM-enable gating, monitor sense and IRQ omitted |
//! | Figs. 2-29–2-37: sequencer | READ: five indexed registers, map mask, chain-4 and independent odd/even write control | Reset signalling, extended-memory gating and display clocks omitted; undefined read bits return their masked stored values |
//! | Fig. 2-58: CRTC protection | READ: R11 bit 7 protects R0–R7 except R7 bit 4 | Timing omitted; other registers retain full bytes, including reserved bits |
//! | Figs. 2-81–2-89: attribute controller | READ: index/data flip-flop, status reset, PAS, palette widths; R10 mask EF, R11 mask FF, R12 mask 0F | ASSUMED: reserved bits read zero; PAS disables palette writes, but undefined PAS palette reads retain stored data; no scanout |
//! | Fig. 2-94, pp. 2-104–2-105: DAC | READ: one eight-bit palette address, read/write state, RGB order, six-bit components, increment after three bytes and address writes restart the cycle | ASSUMED: 3C8 reads shared address n during a read cycle (some RAMDACs prefetch n+1); components are stored immediately even if an address write aborts the triple; eight-bit wrap and wrong-direction data accesses use the shared component phase without changing state |
//! | Fig. 2-94, p. 2-106: PEL mask | READ: mask register address and mode-set value FF | ASSUMED: writable mask ANDs the displayed palette index only; IBM warns against writes, while firmware programs it |
//! | Fig. 2-25: input status 1 | READ: vertical retrace implies display disable (DE=1) | ASSUMED: per-read phase 00,01,09,09, not raster timing; tick updates only trace time |
//! | Figs. 2-77–2-78: memory maps | READ: all four GC memory windows | ASSUMED: unselected bytes read FF without loading latches and discard writes; 128 KiB planar map mirrors at 64 KiB |
//! | Figs. 2-36, 2-74–2-77: odd/even | READ: parity selects maps, GC read-map bit 1 selects the read pair, GR6 bit 1 enables A0 replacement; otherwise A0 passes unchanged | ASSUMED: replacement is gated by odd/even routing (writes with SR4 bit 2 clear; reads with GR5 bit 4 set), not GR6 bit 1 alone; replacement uses CPU A16 in map 00 and inverted MSR bit 5 in other maps |
//! | Figs. 2-37 and 2-65: chain-4 | DERIVED: plane = a & 3; offset = (a & FFFC) OR ((a >> 14) & 3), preserving doubleword scanout placement after unchaining | ASSUMED: both reads and writes use this mapping and the full GC latch/logic/bit-mask pipeline, with sequencer map mask applied |
//! | Figs. 2-69–2-80: read/write modes | READ: all four write modes, rotate, set/reset, logic in modes 0/2/3, bit/map masks, read-map select and colour comparison with don't-care | Memory contention omitted |
//! | Power-on / model reset | Reset clears registers, indices, phase, latches, palette and RAM, including PEL mask | ASSUMED deterministic initialization, not an electrical reset sequence |
//!
//! A literal reading of IBM Fig. 2-77 enables A0 replacement from GR6 bit 1
//! alone. This model calls the replacement logic only after selecting odd/even
//! addressing: SR4 bit 2 clear for writes, GR5 bit 4 set for reads. That gating
//! is ASSUMED and unverified on real hardware. The mixed setting GR6=02h with
//! GR5 bit 4 clear is not used by the reviewed BIOS modes. The test
//! `odd_even_round_trip_pairs_chain_bit_and_independent_read_write_controls`
//! in `vga/tests.rs` records the model's unchanged planar read address in that
//! setting, rather than establishing hardware behaviour.
//!
//! DAC edge cases remain ASSUMED. During a read cycle, 3C8 returns the shared
//! address n; some RAMDACs prefetch and expose n+1. Components change palette
//! RAM immediately, so writing either address port partway through RGB leaves
//! already-written components changed. IBM Fig. 2-94 and pp. 2-104–2-105 say
//! an address write terminates the unfinished cycle and describe concatenating
//! three six-bit values, without establishing this partial-write behaviour.
//!
//! MSR bit 5 is reserved in IBM Fig. 2-21. For chained odd/even accesses in
//! maps 01–11 the compatibility policy injects zero into memory A0 when MSR bit 5
//! is one, and one when it is zero. Thus firmware values 67h/63h/66h select the
//! low page. This polarity is ASSUMED: FreeVGA Miscellaneous Output notes
//! describe the opposite polarity. Map 00 instead injects CPU A16, independent
//! of MSR bit 5. With GR6 bit 1 clear, the original CPU A0 is always preserved.
//!
//! Chain-4 placement is a DERIVATION from IBM Figs. 2-37 and 2-65, rather than
//! an explicit CPU-offset formula in the manual: after the low two CPU bits
//! select the plane, preserving A2–A15 and routing A14–A15 into memory A0–A1
//! gives the doubleword memory layout. CPU accesses use this layout whenever
//! chain-4 is selected; CRTC timing and address outputs are not executed here.
//!
//! Byte I/O and adjacent little-endian word/dword I/O are accepted when every
//! constituent port belongs to the device (including index/data OUTW). MMIO
//! accepts widths 1–8; trailing unselected bytes float FF. Invalid accesses
//! have no effects or traces. Version-2 snapshots preserve all hardware state
//! and virtual trace time; trace policy, history and drops remain run-scoped.

use device::{AccessDir, Device, DeviceError, NextEvent, TraceEvent, VClock};

const NAME: &str = "vga";
const VERSION: u8 = 2;
/// First address of the claimed VGA MMIO aperture, including unselected maps.
pub const MEMORY_BASE: u64 = 0xA_0000;
/// Size of the claimed VGA aperture in bytes.
pub const MEMORY_LEN: usize = 0x2_0000;
/// Bytes in each of the four memory planes.
pub const PLANE_LEN: usize = 0x1_0000;
const SEQ_MASKS: [u8; 5] = [0x03, 0x3F, 0x0F, 0x3F, 0x0E];
const GC_MASKS: [u8; 9] = [0x0F, 0x0F, 0x0F, 0x1F, 0x03, 0x7B, 0x0F, 0x0F, 0xFF];
const AC_MASKS: [u8; 21] = [
    0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F,
    0xEF, 0xFF, 0x0F, 0x0F, 0x0F,
];
const HEADER_LEN: usize = 12 + 4 + 5 + 25 + 9 + 21 + 256 * 3 + 16;
const SNAPSHOT_LEN: usize = HEADER_LEN + 4 * PLANE_LEN;

/// IBM VGA storage and CPU access pipeline behind [`Device`].
///
/// IBM VGA/XGA Technical Reference Manual, Figs. 2-69–2-80. No board
/// routing or rendering is implied by constructing this device.
pub struct Vga {
    planes: [Vec<u8>; 4],
    sequencer: [u8; 5],
    crtc: [u8; 25],
    graphics: [u8; 9],
    attribute: [u8; 21],
    palette: [[u8; 3]; 256],
    misc: u8,
    feature: u8,
    seq_index: u8,
    crtc_index: u8,
    gc_index: u8,
    ac_address: u8,
    ac_data: bool,
    pel_mask: u8,
    dac_address: u8,
    dac_component: u8,
    dac_state: u8,
    latches: [u8; 4],
    status_reads: u64,
    now: VClock,
    trace: device::TraceBuffer,
}

impl Vga {
    /// Deterministic power-on state: all storage zero, monochrome I/O selected.
    #[must_use]
    pub fn new() -> Self {
        Self {
            planes: std::array::from_fn(|_| vec![0; PLANE_LEN]),
            sequencer: [0; 5],
            crtc: [0; 25],
            graphics: [0; 9],
            attribute: [0; 21],
            palette: [[0; 3]; 256],
            misc: 0,
            feature: 0,
            seq_index: 0,
            crtc_index: 0,
            gc_index: 0,
            ac_address: 0,
            ac_data: false,
            pel_mask: 0,
            dac_address: 0,
            dac_component: 0,
            dac_state: 0,
            latches: [0; 4],
            status_reads: 0,
            now: VClock::ZERO,
            trace: device::TraceBuffer::default(),
        }
    }

    /// Plane storage in plane order, without changing CPU latches.
    #[must_use]
    pub fn planes(&self) -> &[Vec<u8>; 4] {
        &self.planes
    }
    /// Sequencer registers R0–R4.
    #[must_use]
    pub fn sequencer(&self) -> &[u8; 5] {
        &self.sequencer
    }
    /// CRT controller registers R00–R18.
    #[must_use]
    pub fn crtc(&self) -> &[u8; 25] {
        &self.crtc
    }
    /// Graphics controller registers R0–R8.
    #[must_use]
    pub fn graphics(&self) -> &[u8; 9] {
        &self.graphics
    }
    /// Attribute controller registers R00–R14.
    #[must_use]
    pub fn attribute(&self) -> &[u8; 21] {
        &self.attribute
    }
    /// Raw six-bit RGB DAC entries, unaffected by the PEL mask.
    #[must_use]
    pub fn palette(&self) -> &[[u8; 3]; 256] {
        &self.palette
    }
    /// RGB entry after the PEL mask has selected the displayed DAC address.
    #[must_use]
    pub fn palette_entry(&self, index: u8) -> [u8; 3] {
        self.palette[usize::from(index & self.pel_mask)]
    }
    /// Miscellaneous Output register, including clock and sync selections.
    #[must_use]
    pub fn misc_output(&self) -> u8 {
        self.misc
    }
    /// Attribute Palette Address Source bit (true selects normal display).
    #[must_use]
    pub fn palette_address_source(&self) -> bool {
        self.ac_address & 0x20 != 0
    }
    /// Stored PEL mask used for display palette lookup.
    #[must_use]
    pub fn pel_mask(&self) -> u8 {
        self.pel_mask
    }

    fn colour(&self) -> bool {
        self.misc & 1 != 0
    }
    fn active_crtc(&self) -> u16 {
        if self.colour() {
            0x3D4
        } else {
            0x3B4
        }
    }
    fn active_status(&self) -> u16 {
        if self.colour() {
            0x3DA
        } else {
            0x3BA
        }
    }

    fn owns_port(port: u16) -> bool {
        matches!(port, 0x3B4..=0x3B5 | 0x3BA | 0x3C0..=0x3C2 | 0x3C4..=0x3CA | 0x3CC | 0x3CE..=0x3CF | 0x3D4..=0x3D5 | 0x3DA)
    }
    fn check_port(port: u16, size: u8) -> Result<(), DeviceError> {
        if !matches!(size, 1 | 2 | 4)
            || !(0..u16::from(size)).all(|i| Self::owns_port(port.wrapping_add(i)))
        {
            return Err(DeviceError::UnmappedRegister {
                kind: "port",
                offset: u64::from(port),
            });
        }
        Ok(())
    }
    fn check_mmio(addr: u64, size: u8) -> Result<(), DeviceError> {
        if !(1..=8).contains(&size)
            || !(MEMORY_BASE..MEMORY_BASE + MEMORY_LEN as u64).contains(&addr)
        {
            return Err(DeviceError::UnmappedRegister {
                kind: "mmio",
                offset: addr,
            });
        }
        Ok(())
    }

    fn read_port_byte(&mut self, port: u16) -> u8 {
        if port == self.active_crtc() {
            return self.crtc_index;
        }
        if port == self.active_crtc() + 1 {
            return self
                .crtc
                .get(usize::from(self.crtc_index))
                .copied()
                .unwrap_or(0);
        }
        if port == self.active_status() {
            self.ac_data = false;
            let phase = self.status_reads as u8;
            self.status_reads = self.status_reads.wrapping_add(1);
            let retrace = (phase & 2) << 2;
            return (phase & 1) | retrace | u8::from(retrace != 0);
        }
        match port {
            0x3C0 => self.ac_address,
            0x3C1 => self
                .attribute
                .get(usize::from(self.ac_address & 0x1F))
                .copied()
                .unwrap_or(0),
            0x3C2 => 0,
            0x3C4 => self.seq_index,
            0x3C5 => self
                .sequencer
                .get(usize::from(self.seq_index))
                .copied()
                .unwrap_or(0),
            0x3C6 => self.pel_mask,
            0x3C7 => self.dac_state,
            0x3C8 => self.dac_address,
            0x3C9 => {
                let value =
                    self.palette[usize::from(self.dac_address)][usize::from(self.dac_component)];
                Self::advance_dac(&mut self.dac_address, &mut self.dac_component);
                value
            }
            0x3CA => self.feature,
            0x3CC => self.misc,
            0x3CE => self.gc_index,
            0x3CF => self
                .graphics
                .get(usize::from(self.gc_index))
                .copied()
                .unwrap_or(0),
            _ => 0xFF,
        }
    }
    fn advance_dac(index: &mut u8, component: &mut u8) {
        *component += 1;
        if *component == 3 {
            *component = 0;
            *index = index.wrapping_add(1);
        }
    }
    fn write_port_byte(&mut self, port: u16, value: u8) {
        if port == self.active_crtc() {
            self.crtc_index = value & 0x1F;
            return;
        }
        if port == self.active_crtc() + 1 {
            let index = usize::from(self.crtc_index);
            if self.crtc[0x11] & 0x80 != 0 && index <= 7 {
                if index == 7 {
                    self.crtc[7] = (self.crtc[7] & !0x10) | (value & 0x10);
                }
            } else if let Some(reg) = self.crtc.get_mut(index) {
                *reg = value;
            }
            return;
        }
        if port == self.active_status() {
            self.feature = value;
            return;
        }
        match port {
            0x3C0 => {
                if self.ac_data {
                    let index = usize::from(self.ac_address & 0x1F);
                    if index >= 16 || !self.palette_address_source() {
                        if let Some(reg) = self.attribute.get_mut(index) {
                            *reg = value & AC_MASKS[index];
                        }
                    }
                } else {
                    self.ac_address = value & 0x3F;
                }
                self.ac_data = !self.ac_data;
            }
            0x3C2 => self.misc = value,
            0x3C4 => self.seq_index = value & 7,
            0x3C5 => {
                if let Some(reg) = self.sequencer.get_mut(usize::from(self.seq_index)) {
                    *reg = value & SEQ_MASKS[usize::from(self.seq_index)];
                }
            }
            0x3C6 => self.pel_mask = value,
            0x3C7 => {
                self.dac_address = value;
                self.dac_component = 0;
                self.dac_state = 3;
            }
            0x3C8 => {
                self.dac_address = value;
                self.dac_component = 0;
                self.dac_state = 0;
            }
            0x3C9 => {
                self.palette[usize::from(self.dac_address)][usize::from(self.dac_component)] =
                    value & 0x3F;
                Self::advance_dac(&mut self.dac_address, &mut self.dac_component);
            }
            0x3CE => self.gc_index = value & 0x0F,
            0x3CF => {
                if let Some(reg) = self.graphics.get_mut(usize::from(self.gc_index)) {
                    *reg = value & GC_MASKS[usize::from(self.gc_index)];
                }
            }
            _ => {}
        }
    }

    fn window_offset(&self, addr: u64) -> Option<usize> {
        let (base, len) = match (self.graphics[6] >> 2) & 3 {
            0 => (MEMORY_BASE, 0x20000),
            1 => (MEMORY_BASE, 0x10000),
            2 => (0xB0000, 0x8000),
            _ => (0xB8000, 0x8000),
        };
        (base..base + len)
            .contains(&addr)
            .then(|| (addr - base) as usize)
    }
    /// IBM VGA/XGA Technical Reference Manual, Fig. 2-77: GR6 controls A0
    /// replacement independently of the parity used to select maps. Callers
    /// apply the ASSUMED odd/even routing gate described in the coverage matrix.
    fn odd_even_offset(&self, address: usize) -> usize {
        if self.graphics[6] & 2 == 0 {
            return address;
        }
        let replacement = if self.graphics[6] & 0x0C == 0 {
            (address >> 16) & 1
        } else {
            usize::from(self.misc & 0x20 == 0)
        };
        (address & !1) | replacement
    }

    /// DERIVED from IBM VGA/XGA Technical Reference Manual, Figs. 2-37 and
    /// 2-65: retain doubleword memory placement when software unchains planes.
    fn chain_four_offset(address: usize) -> usize {
        (address & 0xFFFC) | ((address >> 14) & 3)
    }
    fn read_byte(&mut self, addr: u64) -> u8 {
        let Some(address) = self.window_offset(addr) else {
            return 0xFF;
        };
        let (offset, plane) = if self.sequencer[4] & 8 != 0 {
            (Self::chain_four_offset(address), address & 3)
        } else if self.graphics[5] & 0x10 != 0 {
            (
                self.odd_even_offset(address),
                usize::from(self.graphics[4] & 2) | (address & 1),
            )
        } else {
            (address, usize::from(self.graphics[4] & 3))
        };
        for p in 0..4 {
            self.latches[p] = self.planes[p][offset & 0xFFFF];
        }
        if self.graphics[5] & 8 == 0 {
            return self.latches[plane];
        }
        let mut result = 0xFF;
        for p in 0..4 {
            if self.graphics[7] & (1 << p) != 0 {
                let compare = if self.graphics[2] & (1 << p) != 0 {
                    0xFF
                } else {
                    0
                };
                result &= !(self.latches[p] ^ compare);
            }
        }
        result
    }
    fn write_byte(&mut self, addr: u64, value: u8) {
        let Some(address) = self.window_offset(addr) else {
            return;
        };
        let (offset, select) = if self.sequencer[4] & 8 != 0 {
            (Self::chain_four_offset(address), 1 << (address & 3))
        } else if self.sequencer[4] & 4 == 0 {
            (
                self.odd_even_offset(address),
                if address & 1 == 0 { 5 } else { 10 },
            )
        } else {
            (address, 15)
        };
        let rotated = value.rotate_right(u32::from(self.graphics[3] & 7));
        let mode = self.graphics[5] & 3;
        for p in 0..4 {
            if self.sequencer[2] & select & (1 << p) == 0 {
                continue;
            }
            let latch = self.latches[p];
            let set_reset = if self.graphics[0] & (1 << p) != 0 {
                0xFF
            } else {
                0
            };
            let data = match mode {
                0 if self.graphics[1] & (1 << p) != 0 => set_reset,
                0 => rotated,
                1 => {
                    self.planes[p][offset & 0xFFFF] = latch;
                    continue;
                }
                2 => {
                    if value & (1 << p) != 0 {
                        0xFF
                    } else {
                        0
                    }
                }
                _ => set_reset,
            };
            let data = match (self.graphics[3] >> 3) & 3 {
                0 => data,
                1 => data & latch,
                2 => data | latch,
                _ => data ^ latch,
            };
            let mask = self.graphics[8] & if mode == 3 { rotated } else { 0xFF };
            self.planes[p][offset & 0xFFFF] = (data & mask) | (latch & !mask);
        }
    }
}

impl Default for Vga {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for Vga {
    fn name(&self) -> &'static str {
        NAME
    }
    fn port_read(&mut self, port: u16, size: u8) -> Result<u32, DeviceError> {
        Self::check_port(port, size)?;
        let mut value = 0;
        for i in 0..size {
            value |= u32::from(self.read_port_byte(port + u16::from(i))) << (8 * i);
        }
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
        Self::check_port(port, size)?;
        for i in 0..size {
            self.write_port_byte(port + u16::from(i), (value >> (8 * i)) as u8);
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
    fn mmio_read(&mut self, addr: u64, size: u8) -> Result<u64, DeviceError> {
        Self::check_mmio(addr, size)?;
        let mut value = 0;
        for i in 0..size {
            value |= u64::from(self.read_byte(addr + u64::from(i))) << (8 * i);
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
        Self::check_mmio(addr, size)?;
        for i in 0..size {
            self.write_byte(addr + u64::from(i), (value >> (8 * i)) as u8);
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
    fn next_event(&self, _now: VClock) -> NextEvent {
        NextEvent::Dormant
    }
    fn irq_pending(&self) -> bool {
        false
    }
    fn reset(&mut self) {
        let policy = self.trace.policy();
        *self = Self::new();
        self.trace.set_policy(policy);
    }
    fn save(&self) -> Vec<u8> {
        let mut blob = Vec::with_capacity(SNAPSHOT_LEN);
        blob.extend_from_slice(&[
            VERSION,
            self.misc,
            self.feature,
            self.seq_index,
            self.crtc_index,
            self.gc_index,
            self.ac_address,
            u8::from(self.ac_data),
            self.pel_mask,
            self.dac_address,
            self.dac_component,
            self.dac_state,
        ]);
        blob.extend_from_slice(&self.latches);
        blob.extend_from_slice(&self.sequencer);
        blob.extend_from_slice(&self.crtc);
        blob.extend_from_slice(&self.graphics);
        blob.extend_from_slice(&self.attribute);
        for rgb in self.palette {
            blob.extend_from_slice(&rgb);
        }
        blob.extend_from_slice(&self.status_reads.to_le_bytes());
        blob.extend_from_slice(&self.now.0.to_le_bytes());
        for plane in &self.planes {
            blob.extend_from_slice(plane);
        }
        blob
    }
    fn restore(&mut self, blob: &[u8]) -> Result<(), DeviceError> {
        let invalid = |reason| DeviceError::InvalidSnapshot {
            device: NAME,
            reason,
        };
        if blob.len() != SNAPSHOT_LEN {
            return Err(invalid("snapshot length mismatch"));
        }
        if blob[0] != VERSION {
            return Err(invalid("unsupported snapshot version"));
        }
        if blob[3] > 7
            || blob[4] > 31
            || blob[5] > 15
            || blob[6] > 63
            || blob[7] > 1
            || blob[10] > 2
            || !matches!(blob[11], 0 | 3)
        {
            return Err(invalid("invalid register or access phase"));
        }
        fn take<const N: usize>(blob: &[u8], cursor: &mut usize) -> [u8; N] {
            let value = std::array::from_fn(|i| blob[*cursor + i]);
            *cursor += N;
            value
        }
        let mut candidate = Self::new();
        candidate.misc = blob[1];
        candidate.feature = blob[2];
        candidate.seq_index = blob[3];
        candidate.crtc_index = blob[4];
        candidate.gc_index = blob[5];
        candidate.ac_address = blob[6];
        candidate.ac_data = blob[7] != 0;
        candidate.pel_mask = blob[8];
        candidate.dac_address = blob[9];
        candidate.dac_component = blob[10];
        candidate.dac_state = blob[11];
        let mut cursor = 12;
        candidate.latches = take(blob, &mut cursor);
        candidate.sequencer = take(blob, &mut cursor);
        candidate.crtc = take(blob, &mut cursor);
        candidate.graphics = take(blob, &mut cursor);
        candidate.attribute = take(blob, &mut cursor);
        for rgb in &mut candidate.palette {
            *rgb = take(blob, &mut cursor);
        }
        candidate.status_reads = u64::from_le_bytes(take(blob, &mut cursor));
        candidate.now = VClock(u64::from_le_bytes(take(blob, &mut cursor)));
        if candidate
            .sequencer
            .iter()
            .zip(SEQ_MASKS)
            .any(|(v, m)| v & !m != 0)
            || candidate
                .graphics
                .iter()
                .zip(GC_MASKS)
                .any(|(v, m)| v & !m != 0)
            || candidate
                .attribute
                .iter()
                .zip(AC_MASKS)
                .any(|(v, m)| v & !m != 0)
            || candidate.palette.iter().flatten().any(|v| *v > 63)
        {
            return Err(invalid("invalid reserved register bits or DAC component"));
        }
        for plane in &mut candidate.planes {
            plane.copy_from_slice(&blob[cursor..cursor + PLANE_LEN]);
            cursor += PLANE_LEN;
        }
        std::mem::swap(&mut candidate.trace, &mut self.trace);
        *self = candidate;
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
