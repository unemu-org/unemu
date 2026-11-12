// SPDX-License-Identifier: AGPL-3.0-or-later
//! Portable YMF262 snapshot encoding, independent of Rust structure layout.
//! Yamaha YMF262 datasheet, Register Map; `backlog/docs/design/audio.md` §OPL3.

use super::*;

struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let bytes = self.0.get(..N)?.try_into().ok()?;
        self.0 = &self.0[N..];
        Some(bytes)
    }
}
impl Slot {
    pub(super) fn encode(&self, bytes: &mut Vec<u8>) {
        bytes.extend(self.channel.to_le_bytes());
        bytes.extend(self.out.to_le_bytes());
        bytes.extend(self.fbmod.to_le_bytes());
        bytes.extend(self.modulation.to_le_bytes());
        bytes.extend(self.prout.to_le_bytes());
        bytes.extend(self.eg_rout.to_le_bytes());
        bytes.extend(self.eg_out.to_le_bytes());
        bytes.extend(self.eg_gen.to_le_bytes());
        bytes.extend(self.eg_ksl.to_le_bytes());
        bytes.extend(self.trem.to_le_bytes());
        bytes.extend(self.reg_vib.to_le_bytes());
        bytes.extend(self.reg_type.to_le_bytes());
        bytes.extend(self.reg_ksr.to_le_bytes());
        bytes.extend(self.reg_mult.to_le_bytes());
        bytes.extend(self.reg_ksl.to_le_bytes());
        bytes.extend(self.reg_tl.to_le_bytes());
        bytes.extend(self.reg_ar.to_le_bytes());
        bytes.extend(self.reg_dr.to_le_bytes());
        bytes.extend(self.reg_sl.to_le_bytes());
        bytes.extend(self.reg_rr.to_le_bytes());
        bytes.extend(self.reg_wf.to_le_bytes());
        bytes.extend(self.key.to_le_bytes());
        bytes.extend(self.pg_reset.to_le_bytes());
        bytes.extend(self.pg_phase.to_le_bytes());
        bytes.extend(self.pg_phase_out.to_le_bytes());
    }
    fn decode(reader: &mut Reader<'_>) -> Option<Self> {
        Some(Self {
            channel: u8::from_le_bytes(reader.take()?),
            out: i16::from_le_bytes(reader.take()?),
            fbmod: i16::from_le_bytes(reader.take()?),
            modulation: u8::from_le_bytes(reader.take()?),
            prout: i16::from_le_bytes(reader.take()?),
            eg_rout: u16::from_le_bytes(reader.take()?),
            eg_out: u16::from_le_bytes(reader.take()?),
            eg_gen: u8::from_le_bytes(reader.take()?),
            eg_ksl: u8::from_le_bytes(reader.take()?),
            trem: u8::from_le_bytes(reader.take()?),
            reg_vib: u8::from_le_bytes(reader.take()?),
            reg_type: u8::from_le_bytes(reader.take()?),
            reg_ksr: u8::from_le_bytes(reader.take()?),
            reg_mult: u8::from_le_bytes(reader.take()?),
            reg_ksl: u8::from_le_bytes(reader.take()?),
            reg_tl: u8::from_le_bytes(reader.take()?),
            reg_ar: u8::from_le_bytes(reader.take()?),
            reg_dr: u8::from_le_bytes(reader.take()?),
            reg_sl: u8::from_le_bytes(reader.take()?),
            reg_rr: u8::from_le_bytes(reader.take()?),
            reg_wf: u8::from_le_bytes(reader.take()?),
            key: u8::from_le_bytes(reader.take()?),
            pg_reset: u32::from_le_bytes(reader.take()?),
            pg_phase: u32::from_le_bytes(reader.take()?),
            pg_phase_out: u16::from_le_bytes(reader.take()?),
        })
    }
}
impl Channel {
    pub(super) fn encode(&self, bytes: &mut Vec<u8>) {
        for value in self.out {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(self.chtype.to_le_bytes());
        bytes.extend(self.f_num.to_le_bytes());
        bytes.extend(self.f_num_reg.to_le_bytes());
        bytes.extend(self.block.to_le_bytes());
        bytes.extend(self.block_reg.to_le_bytes());
        bytes.extend(self.fb.to_le_bytes());
        bytes.extend(self.con.to_le_bytes());
        bytes.extend(self.alg.to_le_bytes());
        bytes.extend(self.ksv.to_le_bytes());
        bytes.extend(self.cha.to_le_bytes());
        bytes.extend(self.chb.to_le_bytes());
        bytes.extend(self.chc.to_le_bytes());
        bytes.extend(self.chd.to_le_bytes());
    }
    fn decode(reader: &mut Reader<'_>) -> Option<Self> {
        Some(Self {
            out: [
                u8::from_le_bytes(reader.take()?),
                u8::from_le_bytes(reader.take()?),
                u8::from_le_bytes(reader.take()?),
                u8::from_le_bytes(reader.take()?),
            ],
            chtype: u8::from_le_bytes(reader.take()?),
            f_num: u16::from_le_bytes(reader.take()?),
            f_num_reg: u16::from_le_bytes(reader.take()?),
            block: u8::from_le_bytes(reader.take()?),
            block_reg: u8::from_le_bytes(reader.take()?),
            fb: u8::from_le_bytes(reader.take()?),
            con: u8::from_le_bytes(reader.take()?),
            alg: u8::from_le_bytes(reader.take()?),
            ksv: u8::from_le_bytes(reader.take()?),
            cha: u16::from_le_bytes(reader.take()?),
            chb: u16::from_le_bytes(reader.take()?),
            chc: u16::from_le_bytes(reader.take()?),
            chd: u16::from_le_bytes(reader.take()?),
        })
    }
}
impl Opl3 {
    pub(super) fn encode(&self, bytes: &mut Vec<u8>) {
        for value in &self.slot {
            value.encode(bytes);
        }
        for value in &self.channel {
            value.encode(bytes);
        }
        bytes.extend(self.timer.to_le_bytes());
        bytes.extend(self.eg_timer.to_le_bytes());
        bytes.extend(self.eg_timerrem.to_le_bytes());
        bytes.extend(self.eg_state.to_le_bytes());
        bytes.extend(self.eg_add.to_le_bytes());
        bytes.extend(self.eg_timer_lo.to_le_bytes());
        bytes.extend(self.newm.to_le_bytes());
        bytes.extend(self.nts.to_le_bytes());
        bytes.extend(self.rhy.to_le_bytes());
        bytes.extend(self.vibpos.to_le_bytes());
        bytes.extend(self.vibshift.to_le_bytes());
        bytes.extend(self.tremolo.to_le_bytes());
        bytes.extend(self.tremolopos.to_le_bytes());
        bytes.extend(self.tremoloshift.to_le_bytes());
        bytes.extend(self.noise.to_le_bytes());
        for value in self.mixbuff {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(self.rm_hh_bit2.to_le_bytes());
        bytes.extend(self.rm_hh_bit3.to_le_bytes());
        bytes.extend(self.rm_hh_bit7.to_le_bytes());
        bytes.extend(self.rm_hh_bit8.to_le_bytes());
        bytes.extend(self.rm_tc_bit3.to_le_bytes());
        bytes.extend(self.rm_tc_bit5.to_le_bytes());
    }
    fn decode(reader: &mut Reader<'_>) -> Option<Self> {
        Some(Self {
            slot: [
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
                Slot::decode(reader)?,
            ],
            channel: [
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
                Channel::decode(reader)?,
            ],
            timer: u16::from_le_bytes(reader.take()?),
            eg_timer: u64::from_le_bytes(reader.take()?),
            eg_timerrem: u8::from_le_bytes(reader.take()?),
            eg_state: u8::from_le_bytes(reader.take()?),
            eg_add: u8::from_le_bytes(reader.take()?),
            eg_timer_lo: u8::from_le_bytes(reader.take()?),
            newm: u8::from_le_bytes(reader.take()?),
            nts: u8::from_le_bytes(reader.take()?),
            rhy: u8::from_le_bytes(reader.take()?),
            vibpos: u8::from_le_bytes(reader.take()?),
            vibshift: u8::from_le_bytes(reader.take()?),
            tremolo: u8::from_le_bytes(reader.take()?),
            tremolopos: u8::from_le_bytes(reader.take()?),
            tremoloshift: u8::from_le_bytes(reader.take()?),
            noise: u32::from_le_bytes(reader.take()?),
            mixbuff: [
                i32::from_le_bytes(reader.take()?),
                i32::from_le_bytes(reader.take()?),
                i32::from_le_bytes(reader.take()?),
                i32::from_le_bytes(reader.take()?),
            ],
            rm_hh_bit2: u8::from_le_bytes(reader.take()?),
            rm_hh_bit3: u8::from_le_bytes(reader.take()?),
            rm_hh_bit7: u8::from_le_bytes(reader.take()?),
            rm_hh_bit8: u8::from_le_bytes(reader.take()?),
            rm_tc_bit3: u8::from_le_bytes(reader.take()?),
            rm_tc_bit5: u8::from_le_bytes(reader.take()?),
        })
    }
}

impl Opl3 {
    pub(crate) fn save(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.encode(&mut bytes);
        bytes
    }
    pub(crate) fn restore(bytes: &[u8]) -> Option<Self> {
        let mut reader = Reader(bytes);
        let chip = Self::decode(&mut reader)?;
        if !reader.0.is_empty()
            || chip.eg_timer > 0xfffffffff
            || chip.eg_timerrem > 1
            || chip.eg_state > 1
            || chip.eg_add > 13
            || chip.eg_timer_lo > 3
            || chip.newm > 1
            || chip.nts > 1
            || chip.rhy > 63
            || chip.vibpos > 7
            || chip.vibshift > 1
            || chip.tremolopos >= 210
            || ![2, 4].contains(&chip.tremoloshift)
            || chip.noise > 0x7fffff
            || chip.tremolo > 26
            || [
                chip.rm_hh_bit2,
                chip.rm_hh_bit3,
                chip.rm_hh_bit7,
                chip.rm_hh_bit8,
                chip.rm_tc_bit3,
                chip.rm_tc_bit5,
            ]
            .iter()
            .any(|&bit| bit > 1)
        {
            return None;
        }
        for (index, slot) in chip.slot.iter().enumerate() {
            let expected = CH_SLOT.iter().position(|&s| s == index || s + 3 == index)?;
            if usize::from(slot.channel) != expected
                || slot.modulation > 72
                || slot.eg_rout > 511
                || slot.eg_out > 1023
                || slot.eg_gen > RELEASE
                || slot.trem > 1
                || slot.reg_vib > 1
                || slot.reg_type > 1
                || slot.reg_ksr > 1
                || slot.reg_mult > 15
                || slot.reg_ksl > 3
                || slot.reg_tl > 63
                || slot.reg_ar > 15
                || slot.reg_dr > 15
                || (slot.reg_sl > 14 && slot.reg_sl != 31)
                || slot.reg_rr > 15
                || slot.reg_wf > 7
                || slot.key > 3
                || slot.pg_reset > 1
            {
                return None;
            }
        }
        for ch in chip.channel {
            if ch.out.iter().any(|&id| id > 36)
                || ch.chtype > DRUM
                || ch.f_num > 1023
                || ch.f_num_reg > 1023
                || ch.block > 7
                || ch.block_reg > 7
                || ch.fb > 7
                || ch.con > 1
                || ch.alg > 8
                || ch.ksv > 15
                || [ch.cha, ch.chb, ch.chc, ch.chd]
                    .iter()
                    .any(|&v| v != 0 && v != 65535)
            {
                return None;
            }
        }
        Some(chip)
    }
}
