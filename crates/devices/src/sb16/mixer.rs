// SPDX-License-Identifier: AGPL-3.0-or-later
//! CT1745 register storage and digital voice/FM attenuation.
//!
//! Creative Sound Blaster Series Hardware Programming Guide, §4 CT1745 Mixer,
//! Register Functions and Figure 4-3. See `backlog/docs/design/audio.md` §CT1745.
//! Reserved bits read zero; unknown indices read FFh (ASSUMED). Compatibility
//! writes map each nibble n to (n << 1) | 1 (ASSUMED common compatibility
//! mapping), preserving full-scale unity. Mic writes use the upper three bits
//! and clear two low bits (ASSUMED). Native level zero is -62 dB, Q24 gain 13327.
//!
//! ASSUMED compatibility deviation: master, voice and MIDI (30h-35h) reset to
//! level 31 (F8h, 0 dB) instead of the guide's level 24 (C0h, -14 dB each, so
//! -28 dB through two stages). On a real card Creative's driver set-up programs
//! these levels at boot; without it, games that never touch the mixer play
//! 28 dB down. 86Box's CT1745 and DOSBox Staging's mixer reset to 0 dB too.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Mixer {
    pub index: u8,
    pub registers: [u8; 24],
}

impl Default for Mixer {
    fn default() -> Self {
        let mut registers = [0; 24];
        registers[..6].fill(0xF8);
        registers[12..15].copy_from_slice(&[0x1F, 0x15, 0x0B]);
        registers[20..].fill(0x80);
        Self {
            index: 0,
            registers,
        }
    }
}

// Q24 amplitudes: round(2^24 * 10^((level - 31) / 10)), including level zero.
// The generation formula is checked in tests; execution uses only integers.
const GAIN: [i64; 32] = [
    13327, 16777, 21121, 26590, 33475, 42142, 53054, 66791, 84085, 105857, 133266, 167772, 211213,
    265901, 334749, 421425, 530542, 667913, 840853, 1058571, 1332662, 1677722, 2112126, 2659010,
    3347495, 4214246, 5305422, 6679130, 8408526, 10585708, 13326616, 16777216,
];

impl Mixer {
    fn alias(index: u8) -> Option<usize> {
        match index {
            4 => Some(2),
            0x22 => Some(0),
            0x26 => Some(4),
            0x28 => Some(6),
            0x2E => Some(8),
            _ => None,
        }
    }
    pub fn read(&self) -> u8 {
        if let Some(i) = Self::alias(self.index) {
            return (self.registers[i] & 0xF0) | (self.registers[i + 1] >> 4);
        }
        match self.index {
            0 => 0,
            0x0A => self.registers[10] >> 5,
            0x30..=0x47 => self.registers[usize::from(self.index - 0x30)],
            _ => 0xFF,
        }
    }
    pub fn write(&mut self, value: u8) {
        if let Some(i) = Self::alias(self.index) {
            self.registers[i] = (value & 0xF0) | 8;
            self.registers[i + 1] = (value << 4) | 8;
            return;
        }
        match self.index {
            0 => {
                let index = self.index;
                *self = Self::default();
                self.index = index;
            }
            0x0A => self.registers[10] = value << 5,
            0x30..=0x47 => {
                let mask = Self::mask(self.index);
                self.registers[usize::from(self.index - 0x30)] = value & mask;
            }
            _ => {}
        }
    }
    pub fn mask(index: u8) -> u8 {
        match index {
            0x30..=0x3A => 0xF8,
            0x3B | 0x3F..=0x42 => 0xC0,
            0x3C => 0x1F,
            0x3D..=0x3E => 0x7F,
            0x43 => 1,
            0x44..=0x47 => 0xF0,
            _ => 0,
        }
    }
    pub fn apply(&self, pcm: [i16; 2]) -> [i16; 2] {
        self.apply_input(pcm, 2)
    }
    pub fn apply_fm(&self, pcm: [i16; 2]) -> [i16; 2] {
        self.apply_input(pcm, 4)
    }
    fn apply_input(&self, pcm: [i16; 2], input: usize) -> [i16; 2] {
        std::array::from_fn(|i| {
            let master = GAIN[usize::from(self.registers[i] >> 3)];
            let voice = GAIN[usize::from(self.registers[i + input] >> 3)];
            // Divide once, toward zero, so the two analogue stages do not
            // introduce an intermediate integer PCM quantization.
            (i64::from(pcm[i]) * master * voice / (1i64 << 48)) as i16
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gain_table_matches_the_documented_two_db_steps() {
        for (level, &gain) in GAIN.iter().enumerate() {
            assert_eq!(
                gain,
                ((1u64 << 24) as f64 * 10f64.powf((level as f64 - 31.0) / 10.0)).round() as i64
            );
        }
    }
}
