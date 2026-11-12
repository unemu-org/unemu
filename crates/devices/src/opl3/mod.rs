// SPDX-License-Identifier: LGPL-2.1-or-later
// Nesting preserves the upstream notice byte-for-byte under rustfmt.
/*
/* Nuked OPL3
 * Copyright (C) 2013-2020 Nuke.YKT
 *
 * This file is part of Nuked OPL3.
 *
 * Nuked OPL3 is free software: you can redistribute it and/or modify
 * it under the terms of the GNU Lesser General Public License as
 * published by the Free Software Foundation, either version 2.1
 * of the License, or (at your option) any later version.
 *
 * Nuked OPL3 is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public License
 * along with Nuked OPL3. If not, see <https://www.gnu.org/licenses/>.

 *  Nuked OPL3 emulator.
 *  Thanks:
 *      MAME Development Team(Jarek Burczynski, Tatsuyuki Satoh):
 *          Feedback and Rhythm part calculation information.
 *      forums.submarine.org.uk(carbon14, opl3):
 *          Tremolo and phase generator calculation information.
 *      OPLx decapsulated(Matthew Gambrell, Olli Niemitalo):
 *          OPL2 ROMs.
 *      siliconpr0n.org(John McMaster, digshadow):
 *          YMF262 and VRC VII decaps and die shots.
 *
 * version: 1.8
 */
*/
// Translated to Rust for unemupc98, 2026-11-12; see
// backlog/docs/design/audio.md, OPL3, for the port scope and attribution.
//! Yamaha YMF262 integer FM generator: Rust port of Nuked-OPL3 1.8 (Nuke.YKT),
//! upstream commit 765ec962. Yamaha YMF262 datasheet, Register Map and FM Sound
//! Generator; see `backlog/docs/design/audio.md` §OPL3.
//!
//! | Hardware / upstream function | Coverage |
//! |---|---|
//! | Operators, phase/envelope, ROMs, LFO, noise | Integer native generator |
//! | Register map, NEW, 4-op, rhythm, feedback, four buses | Upstream semantics |
//! | Generate / Generate4Ch | A/B stereo and A/B/C/D, including channel delay |
//! | Resampling, buffered writes, stream helpers, stereo extension | Not ported |
//!
//! Connections use stable indices instead of C pointers; zero is a distinct
//! signal. Narrowing casts reproduce C assignments; phase/timer overflow wraps.

mod snapshot;
mod tables;
use tables::*;

const ATTACK: u8 = 0;
const DECAY: u8 = 1;
const SUSTAIN: u8 = 2;
const RELEASE: u8 = 3;
const TWO: u8 = 0;
const FOUR: u8 = 1;
const SECOND: u8 = 2;
const DRUM: u8 = 3;

// Signal 0 is zero, 1..36 operator outputs, 37..72 feedback values.
#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    channel: u8,
    out: i16,
    fbmod: i16,
    modulation: u8,
    prout: i16,
    eg_rout: u16,
    eg_out: u16,
    eg_gen: u8,
    eg_ksl: u8,
    trem: u8,
    reg_vib: u8,
    reg_type: u8,
    reg_ksr: u8,
    reg_mult: u8,
    reg_ksl: u8,
    reg_tl: u8,
    reg_ar: u8,
    reg_dr: u8,
    reg_sl: u8,
    reg_rr: u8,
    reg_wf: u8,
    key: u8,
    pg_reset: u32,
    pg_phase: u32,
    pg_phase_out: u16,
}

#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    out: [u8; 4],
    chtype: u8,
    f_num: u16,
    f_num_reg: u16,
    block: u8,
    block_reg: u8,
    fb: u8,
    con: u8,
    alg: u8,
    ksv: u8,
    cha: u16,
    chb: u16,
    chc: u16,
    chd: u16,
}

/// Native YMF262 state. No host rate, write buffer or presentation history.
#[derive(Clone, Debug)]
pub struct Opl3 {
    slot: [Slot; 36],
    channel: [Channel; 18],
    timer: u16,
    eg_timer: u64,
    eg_timerrem: u8,
    eg_state: u8,
    eg_add: u8,
    eg_timer_lo: u8,
    newm: u8,
    nts: u8,
    rhy: u8,
    vibpos: u8,
    vibshift: u8,
    tremolo: u8,
    tremolopos: u8,
    tremoloshift: u8,
    noise: u32,
    mixbuff: [i32; 4],
    rm_hh_bit2: u8,
    rm_hh_bit3: u8,
    rm_hh_bit7: u8,
    rm_hh_bit8: u8,
    rm_tc_bit3: u8,
    rm_tc_bit5: u8,
}

impl Default for Opl3 {
    fn default() -> Self {
        let mut chip = Self {
            slot: [Slot::default(); 36],
            channel: [Channel::default(); 18],
            timer: 0,
            eg_timer: 0,
            eg_timerrem: 0,
            eg_state: 0,
            eg_add: 0,
            eg_timer_lo: 0,
            newm: 0,
            nts: 0,
            rhy: 0,
            vibpos: 0,
            vibshift: 1,
            tremolo: 0,
            tremolopos: 0,
            tremoloshift: 4,
            noise: 1,
            mixbuff: [0; 4],
            rm_hh_bit2: 0,
            rm_hh_bit3: 0,
            rm_hh_bit7: 0,
            rm_hh_bit8: 0,
            rm_tc_bit3: 0,
            rm_tc_bit5: 0,
        };
        for (ch, &slot) in CH_SLOT.iter().enumerate() {
            for s in [slot, slot + 3] {
                chip.slot[s].channel = ch as u8;
                chip.slot[s].eg_rout = 0x1ff;
                chip.slot[s].eg_out = 0x1ff;
                chip.slot[s].eg_gen = RELEASE;
            }
            chip.channel[ch].cha = 0xffff;
            chip.channel[ch].chb = 0xffff;
            chip.setup_alg(ch);
        }
        chip
    }
}

fn exp(level: u32) -> i16 {
    let level = level.min(0x1fff);
    ((u32::from(EXPROM[(level & 0xff) as usize]) << 1) >> (level >> 8)) as i16
}

fn sine(wave: u8, phase: u16, envelope: u16) -> i16 {
    let mut p = u32::from(phase & 0x3ff);
    let mut neg = false;
    let log = |p: u32| u32::from(LOGSINROM[(p & 0xff) as usize]);
    let out = match wave {
        0 => {
            neg = p & 0x200 != 0;
            log(if p & 0x100 != 0 { p ^ 0xff } else { p })
        }
        1 => {
            if p & 0x200 != 0 {
                0x1000
            } else {
                log(if p & 0x100 != 0 { p ^ 0xff } else { p })
            }
        }
        2 => log(if p & 0x100 != 0 { p ^ 0xff } else { p }),
        3 => {
            if p & 0x100 != 0 {
                0x1000
            } else {
                log(p)
            }
        }
        4 | 5 => {
            neg = wave == 4 && p & 0x300 == 0x100;
            if p & 0x200 != 0 {
                0x1000
            } else {
                log((if p & 0x80 != 0 { p ^ 0xff } else { p }) << 1)
            }
        }
        6 => {
            neg = p & 0x200 != 0;
            0
        }
        _ => {
            if p & 0x200 != 0 {
                neg = true;
                p = (p & 0x1ff) ^ 0x1ff;
            }
            p << 3
        }
    };
    exp(out + (u32::from(envelope) << 3)) ^ if neg { -1 } else { 0 }
}

impl Opl3 {
    /// Current generator NEW bit; pending bus writes may already select a bank.
    pub(crate) fn new_mode(&self) -> bool {
        self.newm != 0
    }

    fn signal(&self, id: u8) -> i16 {
        match id {
            0 => 0,
            1..=36 => self.slot[usize::from(id - 1)].out,
            _ => self.slot[usize::from(id - 37)].fbmod,
        }
    }
    fn ksl(&mut self, s: usize) {
        let ch = self.channel[usize::from(self.slot[s].channel)];
        self.slot[s].eg_ksl = ((i16::from(KSLROM[usize::from(ch.f_num >> 6)]) << 2)
            - ((8 - i16::from(ch.block)) << 5))
            .max(0) as u8;
    }
    fn envelope(&mut self, s: usize) {
        let ch = self.channel[usize::from(self.slot[s].channel)];
        let slot = &mut self.slot[s];
        slot.eg_out = slot.eg_rout
            + (u16::from(slot.reg_tl) << 2)
            + (u16::from(slot.eg_ksl) >> KSLSHIFT[usize::from(slot.reg_ksl)])
            + if slot.trem != 0 {
                u16::from(self.tremolo)
            } else {
                0
            };
        let reset = slot.key != 0 && slot.eg_gen == RELEASE;
        let reg_rate = if reset {
            slot.reg_ar
        } else {
            match slot.eg_gen {
                ATTACK => slot.reg_ar,
                DECAY => slot.reg_dr,
                SUSTAIN if slot.reg_type != 0 => 0,
                _ => slot.reg_rr,
            }
        };
        slot.pg_reset = u32::from(reset);
        let ks = ch.ksv >> ((slot.reg_ksr ^ 1) << 1);
        let rate = ks + (reg_rate << 2);
        let rate_hi = (rate >> 2).min(15);
        let rate_lo = rate & 3;
        let eg_shift = rate_hi + self.eg_add;
        let mut shift = 0;
        if reg_rate != 0 {
            if rate_hi < 12 {
                if self.eg_state != 0 {
                    shift = match eg_shift {
                        12 => 1,
                        13 => (rate_lo >> 1) & 1,
                        14 => rate_lo & 1,
                        _ => 0,
                    };
                }
            } else {
                const STEP: [[u8; 4]; 4] = [[0, 0, 0, 0], [1, 0, 0, 0], [1, 0, 1, 0], [1, 1, 1, 0]];
                shift = ((rate_hi & 3) + STEP[usize::from(rate_lo)][usize::from(self.eg_timer_lo)])
                    .min(3);
                if shift == 0 {
                    shift = self.eg_state;
                }
            }
        }
        let mut rout = i32::from(slot.eg_rout);
        let mut inc = 0i32;
        let off = slot.eg_rout & 0x1f8 == 0x1f8;
        if reset && rate_hi == 15 {
            rout = 0;
        }
        if slot.eg_gen != ATTACK && !reset && off {
            rout = 0x1ff;
        }
        match slot.eg_gen {
            ATTACK => {
                if slot.eg_rout == 0 {
                    slot.eg_gen = DECAY;
                } else if slot.key != 0 && shift > 0 && rate_hi != 15 {
                    inc = !i32::from(slot.eg_rout) >> (4 - shift);
                }
            }
            DECAY => {
                if slot.eg_rout >> 4 == u16::from(slot.reg_sl) {
                    slot.eg_gen = SUSTAIN;
                } else if !off && !reset && shift > 0 {
                    inc = 1 << (shift - 1);
                }
            }
            _ => {
                if !off && !reset && shift > 0 {
                    inc = 1 << (shift - 1);
                }
            }
        }
        slot.eg_rout = ((rout + inc) & 0x1ff) as u16;
        if reset {
            slot.eg_gen = ATTACK;
        }
        if slot.key == 0 {
            slot.eg_gen = RELEASE;
        }
    }
    fn phase(&mut self, s: usize) {
        let slot = &mut self.slot[s];
        let ch = self.channel[usize::from(slot.channel)];
        let mut f_num = ch.f_num;
        if slot.reg_vib != 0 {
            let mut range = ((f_num >> 7) & 7) as i16;
            if self.vibpos & 3 == 0 {
                range = 0;
            } else if self.vibpos & 1 != 0 {
                range >>= 1;
            }
            range >>= self.vibshift;
            if self.vibpos & 4 != 0 {
                range = -range;
            }
            f_num = f_num.wrapping_add(range as u16);
        }
        let basefreq = (u32::from(f_num) << ch.block) >> 1;
        let phase = (slot.pg_phase >> 9) as u16;
        if slot.pg_reset != 0 {
            slot.pg_phase = 0;
        }
        slot.pg_phase = slot
            .pg_phase
            .wrapping_add((basefreq * u32::from(MT[usize::from(slot.reg_mult)])) >> 1);
        slot.pg_phase_out = phase;
        if s == 13 {
            self.rm_hh_bit2 = ((phase >> 2) & 1) as u8;
            self.rm_hh_bit3 = ((phase >> 3) & 1) as u8;
            self.rm_hh_bit7 = ((phase >> 7) & 1) as u8;
            self.rm_hh_bit8 = ((phase >> 8) & 1) as u8;
        }
        if s == 17 && self.rhy & 0x20 != 0 {
            self.rm_tc_bit3 = ((phase >> 3) & 1) as u8;
            self.rm_tc_bit5 = ((phase >> 5) & 1) as u8;
        }
        if self.rhy & 0x20 != 0 {
            let xor = (self.rm_hh_bit2 ^ self.rm_hh_bit7)
                | (self.rm_hh_bit3 ^ self.rm_tc_bit5)
                | (self.rm_tc_bit3 ^ self.rm_tc_bit5);
            slot.pg_phase_out = match s {
                13 => {
                    (u16::from(xor) << 9)
                        | if u32::from(xor) ^ (self.noise & 1) != 0 {
                            0xd0
                        } else {
                            0x34
                        }
                }
                16 => {
                    (u16::from(self.rm_hh_bit8) << 9)
                        | (((u16::from(self.rm_hh_bit8)) ^ (self.noise & 1) as u16) << 8)
                }
                17 => (u16::from(xor) << 9) | 0x80,
                _ => phase,
            };
        }
        self.noise = (self.noise >> 1) | ((((self.noise >> 14) ^ self.noise) & 1) << 22);
    }
    fn process(&mut self, s: usize) {
        let fb = self.channel[usize::from(self.slot[s].channel)].fb;
        self.slot[s].fbmod = if fb != 0 {
            ((i32::from(self.slot[s].prout) + i32::from(self.slot[s].out)) >> (9 - fb)) as i16
        } else {
            0
        };
        self.slot[s].prout = self.slot[s].out;
        self.envelope(s);
        self.phase(s);
        let slot = self.slot[s];
        self.slot[s].out = sine(
            slot.reg_wf,
            slot.pg_phase_out
                .wrapping_add(self.signal(slot.modulation) as u16),
            slot.eg_out,
        );
    }
    fn mix(&self, right: bool) -> [i32; 2] {
        let mut mix = [0; 2];
        for ch in self.channel {
            let acc = ch
                .out
                .iter()
                .map(|&id| i32::from(self.signal(id)))
                .sum::<i32>() as i16;
            mix[0] += i32::from((acc as u16 & if right { ch.chb } else { ch.cha }) as i16);
            mix[1] += i32::from((acc as u16 & if right { ch.chd } else { ch.chc }) as i16);
        }
        mix
    }
    /// Generate A/B/C/D with upstream's default channel-sample-delay quirk.
    pub fn generate4(&mut self) -> [i16; 4] {
        let mut pcm = [
            0,
            self.mixbuff[1].clamp(-32768, 32767) as i16,
            0,
            self.mixbuff[3].clamp(-32768, 32767) as i16,
        ];
        for s in 0..15 {
            self.process(s);
        }
        let mix = self.mix(false);
        self.mixbuff[0] = mix[0];
        self.mixbuff[2] = mix[1];
        for s in 15..18 {
            self.process(s);
        }
        pcm[0] = self.mixbuff[0].clamp(-32768, 32767) as i16;
        pcm[2] = self.mixbuff[2].clamp(-32768, 32767) as i16;
        for s in 18..33 {
            self.process(s);
        }
        let mix = self.mix(true);
        self.mixbuff[1] = mix[0];
        self.mixbuff[3] = mix[1];
        for s in 33..36 {
            self.process(s);
        }
        if self.timer & 0x3f == 0x3f {
            self.tremolopos = (self.tremolopos + 1) % 210;
        }
        self.tremolo = (if self.tremolopos < 105 {
            self.tremolopos
        } else {
            210 - self.tremolopos
        }) >> self.tremoloshift;
        if self.timer & 0x3ff == 0x3ff {
            self.vibpos = (self.vibpos + 1) & 7;
        }
        self.timer = self.timer.wrapping_add(1);
        if self.eg_state != 0 {
            let shift = self.eg_timer.trailing_zeros().min(13) as u8;
            self.eg_add = if shift > 12 { 0 } else { shift + 1 };
            self.eg_timer_lo = (self.eg_timer & 3) as u8;
        }
        if self.eg_timerrem != 0 || self.eg_state != 0 {
            if self.eg_timer == 0xfffffffff {
                self.eg_timer = 0;
                self.eg_timerrem = 1;
            } else {
                self.eg_timer += 1;
                self.eg_timerrem = 0;
            }
        }
        self.eg_state ^= 1;
        pcm
    }
    /// Generate A/B, as OPL3_Generate does upstream.
    pub fn generate(&mut self) -> [i16; 2] {
        let pcm = self.generate4();
        [pcm[0], pcm[1]]
    }
    fn pair(ch: usize) -> usize {
        if ch % 9 < 3 {
            ch + 3
        } else {
            ch - 3
        }
    }
    fn setup_alg(&mut self, ch: usize) {
        let c = self.channel[ch];
        let a = CH_SLOT[ch];
        let b = a + 3;
        if c.chtype == DRUM {
            if ch == 7 || ch == 8 {
                self.slot[a].modulation = 0;
                self.slot[b].modulation = 0;
            } else {
                self.slot[a].modulation = (a + 37) as u8;
                self.slot[b].modulation = if c.alg & 1 == 0 { (a + 1) as u8 } else { 0 };
            }
            return;
        }
        if c.alg & 8 != 0 {
            return;
        }
        if c.alg & 4 != 0 {
            let pair = Self::pair(ch);
            let p = CH_SLOT[pair];
            let q = p + 3;
            self.channel[pair].out = [0; 4];
            self.slot[p].modulation = (p + 37) as u8;
            match c.alg & 3 {
                0 => {
                    self.slot[q].modulation = (p + 1) as u8;
                    self.slot[a].modulation = (q + 1) as u8;
                    self.slot[b].modulation = (a + 1) as u8;
                    self.channel[ch].out = [(b + 1) as u8, 0, 0, 0];
                }
                1 => {
                    self.slot[q].modulation = (p + 1) as u8;
                    self.slot[a].modulation = 0;
                    self.slot[b].modulation = (a + 1) as u8;
                    self.channel[ch].out = [(q + 1) as u8, (b + 1) as u8, 0, 0];
                }
                2 => {
                    self.slot[q].modulation = 0;
                    self.slot[a].modulation = (q + 1) as u8;
                    self.slot[b].modulation = (a + 1) as u8;
                    self.channel[ch].out = [(p + 1) as u8, (b + 1) as u8, 0, 0];
                }
                _ => {
                    self.slot[q].modulation = 0;
                    self.slot[a].modulation = (q + 1) as u8;
                    self.slot[b].modulation = 0;
                    self.channel[ch].out = [(p + 1) as u8, (a + 1) as u8, (b + 1) as u8, 0];
                }
            }
        } else {
            self.slot[a].modulation = (a + 37) as u8;
            self.slot[b].modulation = if c.alg & 1 == 0 { (a + 1) as u8 } else { 0 };
            self.channel[ch].out = if c.alg & 1 == 0 {
                [(b + 1) as u8, 0, 0, 0]
            } else {
                [(a + 1) as u8, (b + 1) as u8, 0, 0]
            };
        }
    }
    fn update_alg(&mut self, ch: usize) {
        self.channel[ch].alg = self.channel[ch].con;
        match self.channel[ch].chtype {
            FOUR => {
                let p = Self::pair(ch);
                self.channel[p].alg = 4 | (self.channel[ch].con << 1) | self.channel[p].con;
                self.channel[ch].alg = 8;
                self.setup_alg(p);
            }
            SECOND => {
                let p = Self::pair(ch);
                self.channel[ch].alg = 4 | (self.channel[p].con << 1) | self.channel[ch].con;
                self.channel[p].alg = 8;
                self.setup_alg(ch);
            }
            _ => self.setup_alg(ch),
        }
    }
    fn frequency(&mut self, ch: usize) {
        let c = &mut self.channel[ch];
        c.ksv = (c.block << 1) | ((c.f_num >> (9 - self.nts)) & 1) as u8;
        self.ksl(CH_SLOT[ch]);
        self.ksl(CH_SLOT[ch] + 3);
    }
    fn restore_frequency(&mut self, ch: usize) {
        self.channel[ch].f_num = self.channel[ch].f_num_reg;
        self.channel[ch].block = self.channel[ch].block_reg;
        self.frequency(ch);
    }
    fn sync(&mut self, ch: usize) {
        let p = Self::pair(ch);
        self.channel[p].f_num = self.channel[ch].f_num;
        self.channel[p].block = self.channel[ch].block;
        self.frequency(p);
    }
    fn key(&mut self, ch: usize, on: bool) {
        if self.channel[ch].chtype == SECOND {
            return;
        }
        let count = if self.channel[ch].chtype == FOUR {
            2
        } else {
            1
        };
        for c in [ch, if count == 2 { Self::pair(ch) } else { ch }]
            .into_iter()
            .take(count)
        {
            for s in [CH_SLOT[c], CH_SLOT[c] + 3] {
                if on {
                    self.slot[s].key |= 1;
                } else {
                    self.slot[s].key &= !1;
                }
            }
        }
    }
    fn rhythm(&mut self, data: u8) {
        self.rhy = data & 0x3f;
        if self.rhy & 0x20 != 0 {
            self.channel[6].out = [16, 16, 0, 0];
            self.channel[7].out = [14, 14, 17, 17];
            self.channel[8].out = [15, 15, 18, 18];
            for ch in 6..9 {
                self.channel[ch].chtype = DRUM;
                self.setup_alg(ch);
            }
            for (s, bit) in [(13, 1), (17, 2), (14, 4), (16, 8), (12, 16), (15, 16)] {
                if self.rhy & bit != 0 {
                    self.slot[s].key |= 2;
                } else {
                    self.slot[s].key &= !2;
                }
            }
        } else {
            for (ch, &slot) in CH_SLOT.iter().enumerate().take(9).skip(6) {
                self.channel[ch].chtype = TWO;
                self.setup_alg(ch);
                self.slot[slot].key &= !2;
                self.slot[slot + 3].key &= !2;
            }
        }
    }
    /// Decode one immediate register write; the board supplies sample boundaries.
    pub fn write(&mut self, reg: u16, data: u8) {
        let high = usize::from((reg >> 8) & 1);
        let r = (reg & 0xff) as u8;
        let ch = high * 9 + usize::from(r & 15);
        match r & 0xf0 {
            0 => match (high, r & 15) {
                (1, 4) => {
                    for bit in 0..6 {
                        let c = if bit < 3 { bit } else { bit + 6 };
                        if data >> bit & 1 != 0 {
                            self.channel[c].chtype = FOUR;
                            self.channel[c + 3].chtype = SECOND;
                            self.sync(c);
                            self.update_alg(c);
                        } else {
                            self.channel[c].chtype = TWO;
                            self.channel[c + 3].chtype = TWO;
                            self.restore_frequency(c + 3);
                            self.update_alg(c);
                            self.update_alg(c + 3);
                        }
                    }
                }
                (1, 5) => self.newm = data & 1,
                (0, 8) => self.nts = (data >> 6) & 1,
                _ => {}
            },
            0x20..=0x90 | 0xe0 | 0xf0 if AD_SLOT[usize::from(r & 31)] >= 0 => {
                let s = high * 18 + AD_SLOT[usize::from(r & 31)] as usize;
                let slot = &mut self.slot[s];
                match r & 0xe0 {
                    0x20 => {
                        slot.trem = data >> 7;
                        slot.reg_vib = (data >> 6) & 1;
                        slot.reg_type = (data >> 5) & 1;
                        slot.reg_ksr = (data >> 4) & 1;
                        slot.reg_mult = data & 15;
                    }
                    0x40 => {
                        slot.reg_ksl = data >> 6;
                        slot.reg_tl = data & 63;
                        self.ksl(s);
                    }
                    0x60 => {
                        slot.reg_ar = data >> 4;
                        slot.reg_dr = data & 15;
                    }
                    0x80 => {
                        slot.reg_sl = data >> 4;
                        if slot.reg_sl == 15 {
                            slot.reg_sl = 31;
                        }
                        slot.reg_rr = data & 15;
                    }
                    0xe0 => slot.reg_wf = data & if self.newm == 0 { 3 } else { 7 },
                    _ => {}
                }
            }
            0xa0 | 0xb0 if r & 15 < 9 => {
                if r & 0xf0 == 0xa0 {
                    self.channel[ch].f_num_reg =
                        (self.channel[ch].f_num_reg & 0x300) | u16::from(data);
                } else {
                    self.channel[ch].f_num_reg =
                        (self.channel[ch].f_num_reg & 255) | (u16::from(data & 3) << 8);
                    self.channel[ch].block_reg = (data >> 2) & 7;
                }
                if self.channel[ch].chtype != SECOND {
                    self.restore_frequency(ch);
                    if self.channel[ch].chtype == FOUR {
                        self.sync(ch);
                    }
                }
                if r & 0xf0 == 0xb0 {
                    self.key(ch, data & 0x20 != 0);
                }
            }
            0xb0 if r == 0xbd && high == 0 => {
                self.tremoloshift = (((data >> 7) ^ 1) << 1) + 2;
                self.vibshift = ((data >> 6) & 1) ^ 1;
                self.rhythm(data);
            }
            0xc0 if r & 15 < 9 => {
                self.channel[ch].fb = (data & 14) >> 1;
                self.channel[ch].con = data & 1;
                self.update_alg(ch);
                let c = &mut self.channel[ch];
                if self.newm != 0 {
                    c.cha = if data & 0x10 != 0 { 0xffff } else { 0 };
                    c.chb = if data & 0x20 != 0 { 0xffff } else { 0 };
                    c.chc = if data & 0x40 != 0 { 0xffff } else { 0 };
                    c.chd = if data & 0x80 != 0 { 0xffff } else { 0 };
                } else {
                    c.cha = 0xffff;
                    c.chb = 0xffff;
                    c.chc = 0;
                    c.chd = 0;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
