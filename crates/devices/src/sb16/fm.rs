// SPDX-License-Identifier: AGPL-3.0-or-later
//! SB16 YMF262 board timing and timer/status ports.
//!
//! Yamaha YMF262 datasheet, §2 CPU Interface, Register Map (02h–04h, 104h, 105h), Timer and
//! Status Register; Creative Sound Blaster Series Hardware Programming Guide,
//! §2 FM Music I/O Ports, §4 MIDI Volume. See `backlog/docs/design/audio.md` §OPL3.
//! ASSUMED board wiring: outputs A/B feed left/right, C/D are disconnected;
//! timer IRQ is status-only and is not wired to the SB16 PIC interrupt.
//! Generator writes take effect at the next native sample boundary. Timer
//! registers act immediately. ASSUMED timer phase: ticks share the reset-origin
//! sample grid, so start uses the next future 4-/16-sample boundary. This keeps
//! oscillator-divider phase independent of software start time; masks clear and
//! suppress overflow flags.
//! ASSUMED silicon compatibility: ymfm's YMF262 address-high handling and
//! DOSBox DBOPL both fold bank 1 into bank 0 when the last written NEW=0,
//! except the NEW register
//! itself. Pending NEW writes affect addressing immediately; generator changes
//! still wait for a sample boundary. Apply the gate while latching the address
//! so dual-OPL2 software
//! reaches the first nine channels without changing the native generator.

use super::{mixer::Mixer, Output};
use crate::opl3::Opl3;
use device::VClock;

#[derive(Clone, Copy, Debug, Default)]
struct Timer {
    preset: u8,
    counter: u8,
    divider: u8,
    running: bool,
    masked: bool,
    overflow: bool,
}
impl Timer {
    fn tick(&mut self, period: u8) {
        if !self.running {
            return;
        }
        self.divider += 1;
        if self.divider != period {
            return;
        }
        self.divider = 0;
        let (next, overflow) = self.counter.overflowing_add(1);
        self.counter = if overflow { self.preset } else { next };
        if overflow && !self.masked {
            self.overflow = true;
        }
    }
    fn control(&mut self, start: bool, mask: bool, phase: u8) {
        if start && !self.running {
            self.counter = self.preset;
            self.divider = phase;
        }
        self.running = start;
        self.masked = mask;
        if mask {
            self.overflow = false;
        }
    }
}

/// Shared board FM state; assembly advances it when native audio is consumed.
/// No presentation history is part of the hardware snapshot.
pub struct Fm {
    chip: Opl3,
    address: u16,
    timers: [Timer; 2],
    pending: Vec<(u16, u8)>,
    origin: u64,
    index: u64,
    now: u64,
    level: [i16; 2],
    mixer: Mixer,
    output: Option<Box<dyn FnMut(Output)>>,
}
impl Default for Fm {
    fn default() -> Self {
        Self {
            chip: Opl3::default(),
            address: 0,
            timers: [Timer::default(); 2],
            pending: Vec::new(),
            origin: 0,
            index: 1,
            now: 0,
            level: [0; 2],
            mixer: Mixer::default(),
            output: None,
        }
    }
}
impl Fm {
    /// Bind the native A/B observer once, independently from the DSP stream.
    pub fn observe_output(&mut self, output: impl FnMut(Output) + 'static) {
        self.output = Some(Box::new(output));
    }
    fn boundary(&self) -> u64 {
        self.origin.saturating_add(self.index.saturating_mul(24))
    }
    fn emit(&mut self, event: Output) {
        if let Some(output) = &mut self.output {
            output(event);
        }
    }
    fn frame(&mut self, at: u64, period: u64) {
        self.emit(Output::Frame {
            at: VClock(at),
            numerator: period,
            denominator: 1,
            pcm: self.mixer.apply_fm(self.level),
        });
    }
    /// Catch up lazily to a PIT clock. The native core always runs, even silent.
    pub fn advance(&mut self, until: VClock) {
        if until.0 < self.now {
            return;
        }
        while self.boundary() <= until.0 && self.boundary() != u64::MAX {
            let at = self.boundary();
            self.timers[0].tick(4);
            self.timers[1].tick(16);
            for (reg, data) in std::mem::take(&mut self.pending) {
                self.chip.write(reg, data);
            }
            self.level = self.chip.generate();
            self.frame(at, 24);
            self.index = self.index.saturating_add(1);
        }
        self.now = until.0;
    }
    pub(super) fn read(&self, offset: u16) -> u8 {
        if offset != 0 {
            return 0xff;
        }
        let t1 = self.timers[0].overflow;
        let t2 = self.timers[1].overflow;
        (u8::from(t1 || t2) << 7) | (u8::from(t1) << 6) | (u8::from(t2) << 5)
    }
    pub(super) fn write(&mut self, offset: u16, data: u8) {
        if offset & 1 == 0 {
            self.address = ((offset >> 1) << 8) | u16::from(data);
            // Drivers program bank 1 immediately after NEW, before audio samples it.
            let new_mode = self
                .pending
                .iter()
                .rev()
                .find_map(|&(reg, data)| (reg == 0x105).then_some(data & 1 != 0))
                .unwrap_or_else(|| self.chip.new_mode());
            if !new_mode && self.address != 0x105 {
                self.address &= 0xff;
            }
        } else {
            // A1 is latched with the address and is don't-care during data writes
            // (Yamaha YMF262 datasheet, §2 CPU Interface, modes b/c).
            match self.address {
                2 => self.timers[0].preset = data,
                3 => self.timers[1].preset = data,
                4 if data & 0x80 != 0 => {
                    self.timers[0].overflow = false;
                    self.timers[1].overflow = false;
                }
                4 => {
                    let samples = self.index - 1;
                    self.timers[0].control(data & 1 != 0, data & 0x40 != 0, (samples % 4) as u8);
                    self.timers[1].control(data & 2 != 0, data & 0x20 != 0, (samples % 16) as u8);
                }
                _ => self.pending.push((self.address, data)),
            }
        }
    }

    pub(super) fn volume(&mut self, mixer: &Mixer) {
        self.mixer = mixer.clone();
        if self.index > 1 {
            self.frame(self.now, self.boundary().saturating_sub(self.now).max(1));
        }
    }
    pub(super) fn reset(&mut self, now: u64) {
        let output = self.output.take();
        *self = Self {
            origin: now,
            now,
            output,
            ..Self::default()
        };
        self.emit(Output::Pause(VClock(now)));
    }
    pub(super) fn save(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for v in [self.origin, self.index, self.now] {
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend(self.address.to_le_bytes());
        for v in self.level {
            bytes.extend(v.to_le_bytes());
        }
        for t in self.timers {
            bytes.extend([
                t.preset,
                t.counter,
                t.divider,
                u8::from(t.running),
                u8::from(t.masked),
                u8::from(t.overflow),
            ]);
        }
        bytes.extend((self.pending.len() as u64).to_le_bytes());
        for &(reg, data) in &self.pending {
            bytes.extend(reg.to_le_bytes());
            bytes.push(data);
        }
        bytes.extend(self.chip.save());
        bytes
    }
    pub(super) fn decode(bytes: &[u8], mixer: Mixer, now: u64) -> Option<Self> {
        let mut rest = bytes;
        fn take<const N: usize>(rest: &mut &[u8]) -> Option<[u8; N]> {
            let raw = rest.get(..N)?.try_into().ok()?;
            *rest = &rest[N..];
            Some(raw)
        }
        let origin = u64::from_le_bytes(take(&mut rest)?);
        let index = u64::from_le_bytes(take(&mut rest)?);
        let clock = u64::from_le_bytes(take(&mut rest)?);
        if clock != now || origin > now || index != (now - origin) / 24 + 1 {
            return None;
        }
        let address = u16::from_le_bytes(take(&mut rest)?);
        if address > 511 {
            return None;
        }
        let level = [
            i16::from_le_bytes(take(&mut rest)?),
            i16::from_le_bytes(take(&mut rest)?),
        ];
        let mut timers = [Timer::default(); 2];
        for (i, t) in timers.iter_mut().enumerate() {
            let b: [u8; 6] = take(&mut rest)?;
            if b[2] >= if i == 0 { 4 } else { 16 }
                || b[3..].iter().any(|&v| v > 1)
                || b[4] != 0 && b[5] != 0
            {
                return None;
            }
            *t = Timer {
                preset: b[0],
                counter: b[1],
                divider: b[2],
                running: b[3] != 0,
                masked: b[4] != 0,
                overflow: b[5] != 0,
            };
        }
        let len = usize::try_from(u64::from_le_bytes(take(&mut rest)?)).ok()?;
        if len > rest.len() / 3 {
            return None;
        }
        let mut pending = Vec::with_capacity(len);
        for _ in 0..len {
            let reg = u16::from_le_bytes(take(&mut rest)?);
            let data = take::<1>(&mut rest)?[0];
            if reg > 511 {
                return None;
            }
            pending.push((reg, data));
        }
        Some(Self {
            chip: Opl3::restore(rest)?,
            address,
            timers,
            pending,
            origin,
            index,
            now: clock,
            level,
            mixer,
            output: None,
        })
    }
    pub(super) fn restore(&mut self, mut restored: Self) {
        restored.output = self.output.take();
        *self = restored;
        if self.index > 1 {
            self.frame(self.now, self.boundary().saturating_sub(self.now).max(1));
        } else {
            self.emit(Output::Pause(VClock(self.now)));
        }
    }
}
