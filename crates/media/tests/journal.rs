// SPDX-License-Identifier: AGPL-3.0-or-later
//! Wire vectors and crash-model checks for persistent-media.md § Chosen format
//! and § Open and tail recovery. All fixtures are generated in memory.

use media::{
    scan, ClosingReason, Error, Frame, Header, Record, Status, FRAME_LEN, HEADER_LEN, SECTOR_LEN,
};
use sha2::{Digest, Sha256};

fn sector(sequence: u64, generation: u64, lba: u64, value: u8) -> Frame {
    Frame {
        sequence,
        generation,
        record: Record::Sector {
            lba,
            bytes: Box::new([value; SECTOR_LEN]),
        },
    }
}
fn command(sequence: u64, generation: u64, sectors: u64) -> Frame {
    Frame {
        sequence,
        generation,
        record: Record::CommandEnd { sectors },
    }
}
fn end(sequence: u64, generation: u64, reason: ClosingReason) -> Frame {
    Frame {
        sequence,
        generation,
        record: Record::GenerationEnd { reason },
    }
}
fn journal(base: &[u8], anchor: u64, frames: &[Frame]) -> Vec<u8> {
    let mut bytes = Header::new(base, anchor)
        .unwrap()
        .encode()
        .unwrap()
        .to_vec();
    for frame in frames {
        bytes.extend_from_slice(&frame.encode().unwrap());
    }
    bytes
}
fn fixture() -> (Vec<u8>, Vec<u8>) {
    let base = vec![0x55; 3 * SECTOR_LEN];
    let frames = [
        sector(1, 1, 0, 0x11),
        sector(2, 1, 2, 0x22),
        command(3, 1, 2),
        sector(4, 1, 1, 0x33),
        command(5, 1, 1),
        end(6, 1, ClosingReason::Checkpoint),
        sector(7, 2, 0, 0x44),
        command(8, 2, 1),
        end(9, 2, ClosingReason::SessionEnd),
        end(10, 3, ClosingReason::Recovered),
    ];
    let bytes = journal(&base, 0, &frames);
    (base, bytes)
}
fn hex<const N: usize>(value: &str) -> [u8; N] {
    assert_eq!(value.len(), N * 2);
    std::array::from_fn(|i| u8::from_str_radix(&value[2 * i..2 * i + 2], 16).unwrap())
}
fn rehash_header(bytes: &mut [u8]) {
    let hash = Sha256::digest(&bytes[..96]);
    bytes[96..128].copy_from_slice(&hash);
}
fn rehash_frame(bytes: &mut [u8]) {
    let hash = Sha256::digest(&bytes[..608]);
    bytes[608..640].copy_from_slice(&hash);
}

#[test]
fn exact_header_vector_and_round_trip() {
    let base = [0x5a; 1024];
    let expected = hex::<128>(concat!(
        "555043464a4e4c0001008000000200000004000000000000",
        "e8fb68ce4d4d002dba40c0a459d96807c96ded1c2fdefae3f56f8a0c06a4fecf",
        "0700000000000000",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "043ee287a0b4639e3483e22c1f99398f5a0bef81ff6718cb1cd9b294a0f2daf4"
    ));
    let header = Header::new(&base, 7).unwrap();
    assert_eq!(header.encode().unwrap(), expected);
    assert_eq!(Header::decode(&expected, &base, 1024).unwrap(), header);
    assert_eq!(HEADER_LEN, 128);
}

fn check_frame_vector(record: Record, kind: u8, hash: &str) {
    let mut expected = [0; 640];
    expected[..8].copy_from_slice(b"UPCFREC\0");
    expected[8] = kind;
    expected[10] = 64;
    expected[16..24].copy_from_slice(&[8, 7, 6, 5, 4, 3, 2, 1]);
    expected[24..32].copy_from_slice(&[24, 23, 22, 21, 20, 19, 18, 17]);
    if kind == 1 {
        expected[13] = 2;
        expected[32..40].copy_from_slice(&[40, 39, 38, 37, 36, 35, 34, 33]);
        for (i, byte) in expected[64..576].iter_mut().enumerate() {
            *byte = i as u8;
        }
    } else {
        expected[32] = 2;
    }
    expected[608..].copy_from_slice(&hex::<32>(hash));
    let frame = Frame {
        sequence: 0x0102030405060708,
        generation: 0x1112131415161718,
        record,
    };
    assert_eq!(frame.encode().unwrap(), expected);
    assert_eq!(Frame::decode(&expected).unwrap(), frame);
    assert_eq!(FRAME_LEN, 640);
}

#[test]
fn exact_sector_vector_and_round_trip() {
    check_frame_vector(
        Record::Sector {
            lba: 0x2122232425262728,
            bytes: Box::new(std::array::from_fn(|i| i as u8)),
        },
        1,
        "d3e0363690d78005914411d91656b5962945297c347c6a6b891230eeff2ee0e7",
    );
}
#[test]
fn exact_command_end_vector_and_round_trip() {
    check_frame_vector(
        Record::CommandEnd { sectors: 2 },
        2,
        "98cd688658c04f1ca67325ff52b29132a68332a8fd4366e3d032a4a7e2b3a824",
    );
}
#[test]
fn exact_generation_end_vector_and_round_trip() {
    check_frame_vector(
        Record::GenerationEnd {
            reason: ClosingReason::Checkpoint,
        },
        3,
        "da5945deccdcbf0192a18e818d9fa0ea2e35efb8c52b417ba3df5cffdeb55b96",
    );
    for reason in [
        ClosingReason::SessionEnd,
        ClosingReason::Checkpoint,
        ClosingReason::Recovered,
        ClosingReason::RetentionAnchor,
    ] {
        let frame = end(1, 1, reason);
        assert_eq!(Frame::decode(&frame.encode().unwrap()).unwrap(), frame);
    }
}

#[test]
fn header_binding_sizes_and_reserved_fields_are_enforced() {
    let base = [0; 1024];
    let header = Header::new(&base, 0).unwrap().encode().unwrap();
    let mut other_base = base;
    other_base[777] = 1;
    assert_eq!(
        Header::decode(&header, &other_base, 1024),
        Err(Error::BaseDigest)
    );
    assert_eq!(
        Header::decode(&header, &base[..512], 512),
        Err(Error::BaseLength)
    );
    assert_eq!(
        Header::decode(&header, &base, 512),
        Err(Error::GeometryLength)
    );
    for length in [0, 1, 511, 513] {
        assert_eq!(Header::new(&vec![0; length], 0), Err(Error::BaseLength));
    }
    for length in 0..128 {
        assert_eq!(
            Header::decode(&header[..length], &base, 1024),
            Err(Error::HeaderLength)
        );
    }
    let mut extra = header.to_vec();
    extra.push(0);
    assert_eq!(
        Header::decode(&extra, &base, 1024),
        Err(Error::HeaderLength)
    );
    for index in (0..24).chain(64..96) {
        let mut damaged = header;
        damaged[index] ^= 1;
        rehash_header(&mut damaged);
        assert!(
            Header::decode(&damaged, &base, 1024).is_err(),
            "header field {index}"
        );
    }
    for length in [0u64, 513] {
        let mut damaged = header;
        damaged[16..24].copy_from_slice(&length.to_le_bytes());
        rehash_header(&mut damaged);
        assert_eq!(
            Header::decode(&damaged, &base, 1024),
            Err(Error::BaseLength)
        );
    }
    let mut invalid = Header::new(&base, 0).unwrap();
    invalid.base_length = 0;
    assert_eq!(invalid.encode(), Err(Error::BaseLength));
    let report = scan(&header, &other_base, 1024);
    assert!(matches!(report.report.status, Status::Refused(_)));
    assert!(report.image.is_none());
}

#[test]
fn header_rejects_a_matching_non_sector_aligned_base() {
    let base = [0; 600];
    let mut header = Header::new(&base[..512], 0).unwrap().encode().unwrap();
    header[16..24].copy_from_slice(&600u64.to_le_bytes());
    header[24..56].copy_from_slice(&Sha256::digest(base));
    rehash_header(&mut header);
    assert_eq!(Header::decode(&header, &base, 600), Err(Error::BaseLength));
    let result = scan(&header, &base, 600);
    assert!(matches!(
        result.report.status,
        Status::Refused(media::Refusal {
            reason: Error::BaseLength,
            ..
        })
    ));
    assert!(result.image.is_none());
}

#[test]
fn header_version_and_length_are_checked_before_self_hash() {
    let base = [0; 512];
    let header = Header::new(&base, 0).unwrap().encode().unwrap();
    for version in [2u16, u16::MAX] {
        let mut future = header;
        future[8..10].copy_from_slice(&version.to_le_bytes());
        assert_eq!(
            Header::decode(&future, &base, 512),
            Err(Error::Version(version))
        );
        let result = scan(&future, &base, 512);
        assert!(matches!(result.report.status,
            Status::Refused(media::Refusal { reason: Error::Version(found), .. }) if found == version));
        assert!(result.image.is_none());
    }
    for length in [64u16, 129] {
        let mut other_length = header;
        other_length[10..12].copy_from_slice(&length.to_le_bytes());
        assert_eq!(
            Header::decode(&other_length, &base, 512),
            Err(Error::HeaderLength)
        );
    }
    let mut bad_hash = header;
    bad_hash[96] ^= 1;
    assert_eq!(
        Header::decode(&bad_hash, &base, 512),
        Err(Error::HeaderChecksum)
    );
}

#[test]
fn frame_sizes_reserved_fields_and_unknown_encodings_are_enforced() {
    let frames = [
        sector(1, 1, 0, 0x42),
        command(1, 1, 1),
        end(1, 1, ClosingReason::SessionEnd),
    ];
    for frame in &frames {
        let encoded = frame.encode().unwrap();
        for cut in 0..640 {
            assert_eq!(Frame::decode(&encoded[..cut]), Err(Error::FrameLength));
        }
        let mut longer = encoded.to_vec();
        longer.push(0);
        assert_eq!(Frame::decode(&longer), Err(Error::FrameLength));
        for index in (9..16).chain(40..64).chain(576..608) {
            let mut damaged = encoded;
            damaged[index] ^= 1;
            rehash_frame(&mut damaged);
            assert!(Frame::decode(&damaged).is_err(), "field {index}");
        }
        if !matches!(frame.record, Record::Sector { .. }) {
            for index in 64..576 {
                let mut damaged = encoded;
                damaged[index] = 1;
                rehash_frame(&mut damaged);
                assert!(Frame::decode(&damaged).is_err(), "marker payload {index}");
            }
        }
    }
    for kind in [0, 4, 255] {
        let mut damaged = frames[0].encode().unwrap();
        damaged[8] = kind;
        rehash_frame(&mut damaged);
        assert_eq!(Frame::decode(&damaged), Err(Error::UnknownKind(kind)));
    }
    for reason in [0u64, 5, u64::MAX] {
        let mut damaged = frames[2].encode().unwrap();
        damaged[32..40].copy_from_slice(&reason.to_le_bytes());
        rehash_frame(&mut damaged);
        assert_eq!(
            Frame::decode(&damaged),
            Err(Error::Encoding("closing reason"))
        );
    }
    for (sequence, generation) in [(0, 1), (1, 0)] {
        let mut invalid = frames[0].clone();
        invalid.sequence = sequence;
        invalid.generation = generation;
        assert_eq!(
            invalid.encode(),
            Err(Error::Encoding("zero sequence or generation"))
        );
        let mut damaged = frames[0].encode().unwrap();
        damaged[16..24].copy_from_slice(&sequence.to_le_bytes());
        damaged[24..32].copy_from_slice(&generation.to_le_bytes());
        rehash_frame(&mut damaged);
        assert_eq!(
            Frame::decode(&damaged),
            Err(Error::Encoding("zero sequence or generation"))
        );
    }
}

#[test]
fn every_truncation_replays_only_complete_commands() {
    let (base, bytes) = fixture();
    let safe_frames = [0, 0, 0, 3, 3, 5, 6, 6, 8, 9, 10];
    for cut in 0..=bytes.len() {
        let result = scan(&bytes[..cut], &base, base.len() as u64);
        if cut < HEADER_LEN {
            assert!(
                matches!(result.report.status, Status::Refused(_)),
                "cut {cut}"
            );
            assert!(result.image.is_none());
            continue;
        }
        let count = (cut - HEADER_LEN) / FRAME_LEN;
        let safe = safe_frames[count];
        let expected_offset = HEADER_LEN + safe * FRAME_LEN;
        let tail = cut != expected_offset;
        assert_eq!(
            matches!(result.report.status, Status::TornTail(_)),
            tail,
            "cut {cut}"
        );
        assert!(
            !matches!(result.report.status, Status::Refused(_)),
            "cut {cut}"
        );
        assert_eq!(result.report.safe_offset, expected_offset, "cut {cut}");
        assert_eq!(
            result.report.discarded_bytes,
            cut - expected_offset,
            "cut {cut}"
        );
        assert_eq!(
            result.report.last_durable_sequence, safe as u64,
            "cut {cut}"
        );
        let closed = if count >= 10 {
            3
        } else if count >= 9 {
            2
        } else if count >= 6 {
            1
        } else {
            0
        };
        assert_eq!(result.report.last_closed_generation, Some(closed));
        assert_eq!(result.report.generations.len(), closed as usize);
        let mut expected = base.clone();
        if count >= 3 {
            expected[..512].fill(0x11);
            expected[1024..].fill(0x22);
        }
        if count >= 5 {
            expected[512..1024].fill(0x33);
        }
        if count >= 8 {
            expected[..512].fill(0x44);
        }
        assert_eq!(result.image.as_ref().unwrap(), &expected, "cut {cut}");
        let tip = (3..6).contains(&count) || count == 8;
        assert_eq!(result.report.unsealed_tip.is_some(), tip);
        assert_eq!(result.report.needs_recovery(), tail || tip);
    }
}

#[test]
fn every_single_byte_corruption_is_detected() {
    let (base, bytes) = fixture();
    for index in 0..bytes.len() {
        let mut damaged = bytes.clone();
        damaged[index] ^= 1;
        let result = scan(&damaged, &base, base.len() as u64);
        if index < bytes.len() - FRAME_LEN {
            assert!(
                matches!(result.report.status, Status::Refused(_)),
                "byte {index}"
            );
            assert!(result.image.is_none(), "byte {index}");
            assert_eq!(result.report.safe_offset, 0);
        } else {
            assert!(
                matches!(result.report.status, Status::TornTail(_)),
                "byte {index}"
            );
            assert_eq!(result.report.safe_offset, HEADER_LEN + 9 * FRAME_LEN);
            assert_eq!(result.report.discarded_bytes, FRAME_LEN);
        }
    }
}

#[test]
fn torn_frame_with_later_marker_refuses_at_every_cut() {
    let (base, bytes) = fixture();
    for frame in 0..9 {
        let offset = HEADER_LEN + frame * FRAME_LEN;
        for cut in 0..FRAME_LEN {
            let mut damaged = bytes.clone();
            damaged[offset + cut..offset + FRAME_LEN].fill(0);
            let result = scan(&damaged, &base, base.len() as u64);
            assert!(
                matches!(result.report.status, Status::Refused(_)),
                "frame {frame} cut {cut}"
            );
            assert!(result.image.is_none());
        }
    }
    for frame in 0..9 {
        let mut removed = bytes.clone();
        removed.drain(HEADER_LEN + frame * FRAME_LEN..HEADER_LEN + (frame + 1) * FRAME_LEN);
        let status = scan(&removed, &base, base.len() as u64).report.status;
        assert_eq!(
            matches!(status, Status::Refused(_)),
            frame < 8,
            "removed frame {frame}"
        );
        if frame == 8 {
            assert!(matches!(status, Status::TornTail(_)));
        }
    }
}

fn interior_error(frames: &[Frame], expected: Error) {
    let base = [0; 3 * SECTOR_LEN];
    let mut bytes = journal(&base, 0, frames);
    bytes.extend_from_slice(&end(99, 99, ClosingReason::SessionEnd).encode().unwrap());
    let result = scan(&bytes, &base, base.len() as u64);
    assert!(!result.report.needs_recovery());
    match result.report.status {
        Status::Refused(refusal) => {
            assert_eq!(refusal.reason, expected);
            assert!(refusal.later_generation_end.is_some());
        }
        other => panic!("expected interior refusal: {other:?}"),
    }
    assert!(result.image.is_none());
}

#[test]
fn command_and_generation_grammar_is_enforced() {
    interior_error(
        &[sector(2, 1, 0, 1)],
        Error::Sequence {
            expected: 1,
            actual: 2,
        },
    );
    interior_error(
        &[sector(1, 1, 0, 1), command(1, 1, 1)],
        Error::Sequence {
            expected: 2,
            actual: 1,
        },
    );
    interior_error(
        &[sector(1, 2, 0, 1)],
        Error::Generation {
            expected: 1,
            actual: 2,
        },
    );
    interior_error(
        &[sector(1, 1, 0, 1), command(2, 2, 1)],
        Error::Generation {
            expected: 1,
            actual: 2,
        },
    );
    interior_error(
        &[sector(1, 1, 0, 1), end(2, 1, ClosingReason::SessionEnd)],
        Error::GenerationInsideCommand,
    );
    interior_error(
        &[sector(1, 1, 0, 1), sector(2, 1, 0, 2)],
        Error::DuplicateSector(0),
    );
    interior_error(
        &[sector(1, 1, 0, 1), command(2, 1, 2)],
        Error::CommandCount {
            expected: 1,
            actual: 2,
        },
    );
    interior_error(&[command(1, 1, 0)], Error::EmptyCommand);
    for lba in [3, u64::MAX] {
        interior_error(&[sector(1, 1, lba, 1)], Error::SectorBounds(lba));
    }
    interior_error(
        &[end(1, 1, ClosingReason::RetentionAnchor)],
        Error::Anchor("anchor reason outside initial anchor"),
    );
    for generation in [1, 3] {
        interior_error(
            &[
                end(1, 1, ClosingReason::Checkpoint),
                sector(2, generation, 0, 1),
            ],
            Error::Generation {
                expected: 2,
                actual: generation,
            },
        );
    }
}

#[test]
fn final_grammar_failure_is_reported_as_uncertain_tail() {
    let base = [0; 512];
    let bytes = journal(&base, 0, &[sector(1, 1, 0, 1), command(3, 1, 1)]);
    let result = scan(&bytes, &base, 512);
    assert_eq!(result.image.unwrap(), base);
    assert_eq!(result.report.safe_offset, HEADER_LEN);
    assert_eq!(result.report.discarded_bytes, 2 * FRAME_LEN);
    assert_eq!(result.report.last_durable_sequence, 0);
    assert!(matches!(
        result.report.status,
        Status::TornTail(media::Tail {
            failure_offset: 768,
            reason: Error::Sequence {
                expected: 2,
                actual: 3
            }
        })
    ));
    assert_eq!(result.report.recovery_seal().unwrap(), None);
}

#[test]
fn later_marker_detection_is_conservative_and_uses_only_fixed_stride() {
    let base = [0; 512];
    let mut bytes = journal(&base, 0, &[sector(1, 1, 0, 1), command(2, 1, 1)]);
    bytes[HEADER_LEN + 100] ^= 1;
    let mut marker = end(1, 1, ClosingReason::SessionEnd).encode().unwrap();
    marker[16..32].fill(0);
    marker[9] = 1;
    rehash_frame(&mut marker);
    bytes.extend_from_slice(&marker);
    let result = scan(&bytes, &base, 512);
    assert!(matches!(
        result.report.status,
        Status::Refused(media::Refusal {
            later_generation_end: Some(1408),
            ..
        })
    ));
    assert_eq!(result.report.recovery_seal().unwrap(), None);
    marker[639] ^= 1;
    bytes.truncate(1408);
    bytes.extend_from_slice(&marker);
    assert!(matches!(
        scan(&bytes, &base, 512).report.status,
        Status::TornTail(_)
    ));
    bytes.truncate(1408);
    marker[639] ^= 1;
    bytes.push(0);
    bytes.extend_from_slice(&marker);
    assert!(matches!(
        scan(&bytes, &base, 512).report.status,
        Status::TornTail(_)
    ));
    // Valid marker-shaped guest bytes cannot create an interior-corruption signal.
    let mut payload = Box::new([0; 512]);
    payload[..128].copy_from_slice(&marker[..128]);
    let frames = [
        Frame {
            sequence: 1,
            generation: 1,
            record: Record::Sector {
                lba: 0,
                bytes: payload,
            },
        },
        command(2, 1, 1),
    ];
    let valid = journal(&base, 0, &frames);
    assert_eq!(scan(&valid, &base, 512).report.status, Status::Valid);
}

#[test]
fn replay_preserves_order_padding_and_unchanged_sectors() {
    let base = [0x55; 3 * SECTOR_LEN];
    let mut partial = Box::new([0; SECTOR_LEN]);
    partial[..13].fill(0x77);
    let frames = [
        sector(1, 1, 2, 0x22),
        sector(2, 1, 0, 0x11),
        command(3, 1, 2),
        end(4, 1, ClosingReason::Checkpoint),
        Frame {
            sequence: 5,
            generation: 2,
            record: Record::Sector {
                lba: 0,
                bytes: partial,
            },
        },
        command(6, 2, 1),
        sector(7, 2, 2, 0xff),
    ];
    let bytes = journal(&base, 0, &frames);
    let result = scan(&bytes, &base, base.len() as u64);
    let mut expected = base;
    expected[..512].fill(0);
    expected[..13].fill(0x77);
    expected[1024..].fill(0x22);
    assert_eq!(result.image.unwrap(), expected);
    assert_eq!(result.report.generations[0].commands, 1);
    assert_eq!(result.report.generations[0].sectors, 2);
    assert_eq!(result.report.unsealed_tip.unwrap().sectors, 1);
    assert_eq!(result.report.last_durable_sequence, 6);
    assert_eq!(base, [0x55; 3 * SECTOR_LEN]);
}

#[test]
fn refused_report_with_a_committed_tip_never_proposes_recovery() {
    let base = [0; 512];
    let mut bytes = journal(
        &base,
        0,
        &[
            sector(1, 1, 0, 1),
            command(2, 1, 1),
            sector(3, 1, 0, 2),
            end(4, 1, ClosingReason::SessionEnd),
        ],
    );
    bytes[HEADER_LEN + 2 * FRAME_LEN + 64] ^= 1;
    let result = scan(&bytes, &base, 512);
    let Status::Refused(refusal) = &result.report.status else {
        panic!("expected interior refusal: {:?}", result.report.status);
    };
    assert_eq!(refusal.reason, Error::FrameChecksum);
    assert_eq!(
        refusal.later_generation_end,
        Some(HEADER_LEN + 3 * FRAME_LEN)
    );
    let tip = result.report.unsealed_tip.as_ref().unwrap();
    assert_eq!(tip.generation, 1);
    assert_eq!(tip.commands, 1);
    assert_eq!(result.report.last_durable_sequence, 2);
    assert_eq!(result.report.recovery_seal().unwrap(), None);
    assert!(!result.report.needs_recovery());
    assert!(result.image.is_none());
}

#[test]
fn report_proposes_sealing_only_a_command_complete_tip() {
    let (base, bytes) = fixture();
    let cut = HEADER_LEN + 8 * FRAME_LEN + 57;
    let result = scan(&bytes[..cut], &base, base.len() as u64);
    assert_eq!(result.report.unsealed_tip.as_ref().unwrap().generation, 2);
    assert_eq!(result.report.unsealed_tip.as_ref().unwrap().commands, 1);
    let seal = result.report.recovery_seal().unwrap().unwrap();
    assert_eq!(seal, end(9, 2, ClosingReason::Recovered));
    let mut repaired = bytes[..result.report.safe_offset].to_vec();
    repaired.extend_from_slice(&seal.encode().unwrap());
    let closed = scan(&repaired, &base, base.len() as u64);
    assert_eq!(closed.report.status, Status::Valid);
    assert!(!closed.report.needs_recovery());
    assert_eq!(
        closed.report.generations[1].reason,
        ClosingReason::Recovered
    );
    assert_eq!(closed.image, result.image);
    assert_eq!(closed.report.recovery_seal().unwrap(), None);
    assert_eq!(bytes.len(), HEADER_LEN + 10 * FRAME_LEN);
}

#[test]
fn anchor_round_trip_and_every_incomplete_prefix() {
    let base = [0; 3 * SECTOR_LEN];
    let frames = [
        sector(1, 7, 0, 0x11),
        sector(2, 7, 2, 0x22),
        command(3, 7, 2),
        end(4, 7, ClosingReason::RetentionAnchor),
        sector(5, 8, 1, 0x33),
        command(6, 8, 1),
        end(7, 8, ClosingReason::SessionEnd),
    ];
    let bytes = journal(&base, 7, &frames);
    let anchor_end = HEADER_LEN + 4 * FRAME_LEN;
    for cut in HEADER_LEN..anchor_end {
        let result = scan(&bytes[..cut], &base, base.len() as u64);
        assert!(
            matches!(
                result.report.status,
                Status::Refused(media::Refusal {
                    incomplete_anchor: true,
                    ..
                })
            ),
            "cut {cut}"
        );
        assert!(result.image.is_none());
        assert_eq!(result.report.safe_offset, 0);
        assert_eq!(result.report.last_closed_generation, None);
    }
    for cut in anchor_end..=bytes.len() {
        let result = scan(&bytes[..cut], &base, base.len() as u64);
        assert!(
            !matches!(result.report.status, Status::Refused(_)),
            "cut {cut}"
        );
        let mut expected = base;
        expected[..512].fill(0x11);
        expected[1024..].fill(0x22);
        if cut >= HEADER_LEN + 6 * FRAME_LEN {
            expected[512..1024].fill(0x33);
        }
        assert_eq!(result.image.unwrap(), expected, "cut {cut}");
        assert_eq!(result.report.generations[0].id, 7);
        assert_eq!(
            result.report.generations[0].reason,
            ClosingReason::RetentionAnchor
        );
        assert_eq!(result.report.generations[0].end_offset, anchor_end);
        assert_eq!(result.report.generations[0].commands, 1);
        assert_eq!(result.report.generations[0].sectors, 2);
    }
    let result = scan(&bytes, &base, base.len() as u64);
    assert_eq!(result.report.last_closed_generation, Some(8));
    assert_eq!(result.report.last_durable_sequence, 7);
}

#[test]
fn empty_anchor_is_the_only_legal_zero_sector_command() {
    let base = [0; 512];
    let bytes = journal(
        &base,
        4,
        &[command(1, 4, 0), end(2, 4, ClosingReason::RetentionAnchor)],
    );
    let result = scan(&bytes, &base, 512);
    assert_eq!(result.report.status, Status::Valid);
    assert_eq!(result.image.unwrap(), base);
    assert_eq!(result.report.generations[0].commands, 1);
    assert_eq!(result.report.generations[0].sectors, 0);
    assert_eq!(result.report.last_closed_generation, Some(4));
    for cut in HEADER_LEN..bytes.len() {
        let result = scan(&bytes[..cut], &base, 512);
        assert!(matches!(result.report.status, Status::Refused(_)));
        assert_eq!(result.report.last_closed_generation, None);
    }
    let empty = journal(&base, 0, &[]);
    assert_eq!(
        scan(&empty, &base, 512).report.last_closed_generation,
        Some(0)
    );
    let ended = journal(&base, 0, &[end(1, 1, ClosingReason::SessionEnd)]);
    assert_eq!(scan(&ended, &base, 512).report.generations[0].commands, 0);
}

#[test]
fn anchor_grammar_rejects_noncanonical_snapshots() {
    let base = [0; 3 * SECTOR_LEN];
    let cases = [
        (
            vec![sector(1, 7, 0, 0)],
            Error::Anchor("unchanged base sector"),
        ),
        (
            vec![sector(1, 7, 2, 1), sector(2, 7, 0, 1)],
            Error::Anchor("sectors must be in increasing LBA order"),
        ),
        (
            vec![command(1, 7, 0), command(2, 7, 0)],
            Error::Anchor("more than one synthetic command"),
        ),
        (
            vec![command(1, 7, 0), sector(2, 7, 0, 1)],
            Error::Anchor("more than one synthetic command"),
        ),
        (
            vec![command(1, 7, 0), end(2, 7, ClosingReason::SessionEnd)],
            Error::Anchor("missing synthetic command or anchor reason"),
        ),
        (
            vec![end(1, 7, ClosingReason::RetentionAnchor)],
            Error::Anchor("missing synthetic command or anchor reason"),
        ),
        (
            vec![sector(1, 8, 0, 1)],
            Error::Generation {
                expected: 7,
                actual: 8,
            },
        ),
    ];
    for (frames, expected) in cases {
        let bytes = journal(&base, 7, &frames);
        let result = scan(&bytes, &base, base.len() as u64);
        assert!(result.image.is_none());
        match result.report.status {
            Status::Refused(refusal) => {
                assert_eq!(refusal.reason, expected);
                assert!(refusal.incomplete_anchor);
            }
            other => panic!("expected anchor refusal: {other:?}"),
        }
    }
    let bytes = journal(
        &base,
        u64::MAX,
        &[
            command(1, u64::MAX, 0),
            end(2, u64::MAX, ClosingReason::RetentionAnchor),
        ],
    );
    assert!(matches!(
        scan(&bytes, &base, base.len() as u64).report.status,
        Status::Refused(media::Refusal {
            reason: Error::IntegerExhausted,
            ..
        })
    ));
}

struct BoundedReader {
    cursor: std::io::Cursor<Vec<u8>>,
    fail_at: Option<u64>,
    max_read: usize,
}
impl std::io::Read for BoundedReader {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        assert!(
            bytes.len() <= FRAME_LEN,
            "scan must stream one frame at a time"
        );
        self.max_read = self.max_read.max(bytes.len());
        if self
            .fail_at
            .is_some_and(|offset| self.cursor.position() >= offset)
        {
            return Err(std::io::Error::other("injected read failure"));
        }
        std::io::Read::read(&mut self.cursor, bytes)
    }
}
impl std::io::Seek for BoundedReader {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        std::io::Seek::seek(&mut self.cursor, position)
    }
}

#[test]
fn streaming_matches_slice_at_every_cut_and_bounds_read_requests() {
    let (base, journal) = fixture();
    for cut in 0..=journal.len() {
        let mut input = BoundedReader {
            cursor: std::io::Cursor::new(journal[..cut].to_vec()),
            fail_at: None,
            max_read: 0,
        };
        assert_eq!(
            media::scan_reader(&mut input, &base, base.len() as u64).unwrap(),
            scan(&journal[..cut], &base, base.len() as u64),
            "cut {cut}"
        );
    }
    for index in [HEADER_LEN, HEADER_LEN + 3 * FRAME_LEN, journal.len() - 1] {
        let mut damaged = journal.clone();
        damaged[index] ^= 1;
        let mut cursor = std::io::Cursor::new(damaged.clone());
        assert_eq!(
            media::scan_reader(&mut cursor, &base, base.len() as u64).unwrap(),
            scan(&damaged, &base, base.len() as u64)
        );
    }
}

#[test]
fn streaming_io_errors_abort_instead_of_classifying_tail() {
    let (base, journal) = fixture();
    for offset in [
        0,
        HEADER_LEN,
        HEADER_LEN + FRAME_LEN,
        journal.len() - FRAME_LEN,
    ] {
        let mut input = BoundedReader {
            cursor: std::io::Cursor::new(journal.clone()),
            fail_at: Some(offset as u64),
            max_read: 0,
        };
        assert!(media::scan_reader(&mut input, &base, base.len() as u64).is_err());
    }
    // Failure while checking for later markers also aborts; otherwise an I/O
    // error could hide closed history and incorrectly authorize truncation.
    let mut damaged = journal;
    damaged[HEADER_LEN] ^= 1;
    let mut input = BoundedReader {
        cursor: std::io::Cursor::new(damaged),
        fail_at: Some((HEADER_LEN + FRAME_LEN) as u64),
        max_read: 0,
    };
    assert!(media::scan_reader(&mut input, &base, base.len() as u64).is_err());
}

#[test]
fn nvram_block_rejects_clock_changes_padding_and_other_short_bases() {
    let base = [0x22; 64];
    for length in [0, 14, 50, 63, 65, 511, 513] {
        assert_eq!(Header::new(&vec![0; length], 0), Err(Error::BaseLength));
    }
    let mut block = [0; 512];
    block[..64].copy_from_slice(&base);
    block[14] = 0x99;
    let frames = |block| {
        [
            Frame {
                sequence: 1,
                generation: 1,
                record: Record::Sector {
                    lba: 0,
                    bytes: Box::new(block),
                },
            },
            command(2, 1, 1),
            end(3, 1, ClosingReason::Checkpoint),
        ]
    };
    let valid = journal(&base, 0, &frames(block));
    let scan = scan(&valid, &base, 64);
    assert_eq!(scan.report.status, Status::Valid);
    assert_eq!(scan.image.as_ref().unwrap()[14], 0x99);
    for index in [0, 13, 15, 64, 511] {
        let mut bad = block;
        bad[index] ^= 1;
        let bytes = journal(&base, 0, &frames(bad));
        assert!(
            matches!(
                media::scan(&bytes, &base, 64).report.status,
                Status::Refused(_)
            ),
            "index {index}"
        );
    }
}
