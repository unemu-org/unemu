// SPDX-License-Identifier: AGPL-3.0-or-later
//! Differential native-frame tests, Nuked-OPL3 1.8 commit 765ec962.
//! Yamaha YMF262 datasheet, Register Map; `backlog/docs/design/audio.md` §OPL3.
use super::*;

fn vector(sequence: &str, rle: &[u8]) {
    let mut words = sequence.split_whitespace();
    let frames: usize = words.next().unwrap().parse().unwrap();
    let mut writes = Vec::new();
    while let Some(n) = words.next() {
        writes.push((
            n.parse::<usize>().unwrap(),
            u16::from_str_radix(words.next().unwrap(), 16).unwrap(),
            u8::from_str_radix(words.next().unwrap(), 16).unwrap(),
        ));
    }
    let mut expected = Vec::new();
    assert_eq!(rle.len() % 10, 0);
    for record in rle.as_chunks::<10>().0 {
        let count = u16::from_le_bytes([record[0], record[1]]);
        let frame =
            std::array::from_fn(|i| i16::from_le_bytes([record[2 + i * 2], record[3 + i * 2]]));
        expected.extend(std::iter::repeat_n(frame, usize::from(count)));
    }
    assert_eq!(expected.len(), frames);
    let mut chip = Opl3::default();
    let mut stereo = chip.clone();
    let mut writes = writes.into_iter().peekable();
    for (n, frame) in expected.into_iter().enumerate() {
        while writes.peek().is_some_and(|w| w.0 == n) {
            let (_, reg, data) = writes.next().unwrap();
            chip.write(reg, data);
            stereo.write(reg, data);
        }
        assert_eq!(chip.generate4(), frame, "native frame {n}");
        assert_eq!(stereo.generate(), [frame[0], frame[1]], "stereo frame {n}");
        if n % 997 == 0 {
            chip = Opl3::restore(&chip.save()).unwrap();
        }
    }
    assert!(writes.next().is_none());
}
macro_rules! reference {
    ($name:ident, $file:literal) => {
        #[test]
        fn $name() {
            vector(
                include_str!(concat!("../../tests/data/opl3/", $file, ".seq")),
                include_bytes!(concat!("../../tests/data/opl3/", $file, ".rle")),
            );
        }
    };
}
reference!(reference_tone_attack_keyoff_release, "tone");
reference!(reference_four_operator_algorithms_and_pair_changes, "four");
reference!(reference_rhythm_noise_and_drum_keys, "rhythm");
reference!(reference_opl2_waveform_select, "opl2");
reference!(reference_tremolo_vibrato_depths, "lfo");
reference!(reference_feedback_connections, "feedback");
reference!(reference_four_output_panning, "panning");

#[test]
fn new_masks_waveforms_at_write_time_as_upstream() {
    let mut chip = Opl3::default();
    chip.write(0xe0, 7);
    assert_eq!(chip.slot[0].reg_wf, 3);
    chip.write(0x105, 1);
    chip.write(0xe0, 7);
    assert_eq!(chip.slot[0].reg_wf, 7);
    chip.write(0x105, 0);
    assert_eq!(chip.slot[0].reg_wf, 7);
    chip.write(0xc0, 0);
    assert_eq!([chip.channel[0].cha, chip.channel[0].chb], [65535; 2]);
    chip.write(0x105, 1);
    chip.write(0xc0, 0);
    assert_eq!([chip.channel[0].cha, chip.channel[0].chb], [0; 2]);
}

#[test]
fn snapshot_rejects_invalid_rhythm_latches() {
    for bit in 0..6 {
        let mut chip = Opl3::default();
        let fields = [
            &mut chip.rm_hh_bit2,
            &mut chip.rm_hh_bit3,
            &mut chip.rm_hh_bit7,
            &mut chip.rm_hh_bit8,
            &mut chip.rm_tc_bit3,
            &mut chip.rm_tc_bit5,
        ];
        *fields.into_iter().nth(bit).unwrap() = 2;
        assert!(Opl3::restore(&chip.save()).is_none(), "rhythm latch {bit}");
    }
}

#[test]
fn snapshot_rejects_out_of_range_tremolo() {
    let mut chip = Opl3 {
        tremolo: 27,
        ..Opl3::default()
    };
    assert!(Opl3::restore(&chip.save()).is_none());
    chip.tremolo = 26;
    assert!(Opl3::restore(&chip.save()).is_some());
}

#[test]
fn snapshot_rejects_unencodable_sustain_levels() {
    let mut chip = Opl3::default();
    for level in 0..=31 {
        chip.slot[0].reg_sl = level;
        assert_eq!(
            Opl3::restore(&chip.save()).is_some(),
            level <= 14 || level == 31,
            "sustain level {level}"
        );
    }
}
