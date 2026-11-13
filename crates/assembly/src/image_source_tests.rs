// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic split-ROM tests; see firmware design §3.5.

use super::*;
use crate::{
    check_firmware_parts, check_system_bios, interleave_parts, AssemblyOptions, BoardModel,
    FirmwareDistribution, FirmwareEntry, FirmwareStatus, ImagePart, KnownPart, Lane, LoadImage,
};
use std::path::PathBuf;

struct PartsDirectory(PathBuf);
impl PartsDirectory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "assembly-rom-parts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn source(&self, parts: &[(Lane, u64, Vec<u8>)]) -> ImageSource {
        ImageSource::Interleaved(
            parts
                .iter()
                .enumerate()
                .map(|(i, (lane, offset, bytes))| {
                    let path = self.0.join(format!("chip-{i}"));
                    std::fs::write(&path, bytes).unwrap();
                    ImagePart {
                        path,
                        lane: *lane,
                        offset: *offset,
                    }
                })
                .collect(),
        )
    }
}
impl Drop for PartsDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn pair() -> Vec<(Lane, u64, Vec<u8>)> {
    vec![
        (Lane::Even, 0, vec![0x12; 4096]),
        (Lane::Odd, 0, vec![0x34; 4096]),
    ]
}
fn synthetic_entry(bytes: &[u8]) -> FirmwareEntry {
    FirmwareEntry {
        id: "synthetic-user-bios",
        sha256: Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str()),
        size: bytes.len() as u64,
        distribution: FirmwareDistribution::UserSupplied,
        status: FirmwareStatus::Listed,
        parts: Box::leak(
            vec![Box::leak(
                pair()
                    .iter()
                    .enumerate()
                    .map(|(i, (lane, offset, bytes))| KnownPart {
                        label: if i == 0 { "U27" } else { "U47" },
                        part_number: if i == 0 {
                            "synthetic-even"
                        } else {
                            "synthetic-odd"
                        },
                        lane: *lane,
                        offset: *offset,
                        size: bytes.len() as u64,
                        sha256: Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str()),
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ) as &'static [KnownPart]]
            .into_boxed_slice(),
        ),
        ..crate::KISELEV_XI8088
    }
}

#[test]
fn parts_one_and_two_pairs_place_every_byte_including_last() {
    let directory = PartsDirectory::new();
    let one = vec![
        (Lane::Odd, 0, vec![0x20, 0x21, 0x22]),
        (Lane::Even, 0, vec![0x10, 0x11, 0x12]),
    ];
    assert_eq!(
        load_image_bytes(&directory.source(&one)).unwrap(),
        [0x10, 0x20, 0x11, 0x21, 0x12, 0x22]
    );
    let two = vec![
        (Lane::Odd, 4, vec![8, 10]),
        (Lane::Even, 0, vec![1, 3]),
        (Lane::Even, 4, vec![7, 9]),
        (Lane::Odd, 0, vec![2, 4]),
    ];
    assert_eq!(
        load_image_bytes(&directory.source(&two)).unwrap(),
        [1, 2, 3, 4, 7, 8, 9, 10]
    );
}
#[test]
fn parts_layout_errors_have_exact_messages_even_with_escape() {
    let cases = [
        (vec![], "parts require at least one even and one odd part"),
        (
            vec![(Lane::Even, 0, vec![1])],
            "parts require at least one even and one odd part",
        ),
        (
            vec![(Lane::Odd, 0, vec![1])],
            "parts require at least one even and one odd part",
        ),
        (
            vec![
                (Lane::Even, 0, vec![1]),
                (Lane::Even, 0, vec![2]),
                (Lane::Odd, 0, vec![3]),
            ],
            "duplicate even part at offset 0",
        ),
        (
            vec![
                (Lane::Even, 0, vec![1]),
                (Lane::Odd, 0, vec![2]),
                (Lane::Odd, 0, vec![3]),
            ],
            "duplicate odd part at offset 0",
        ),
        (
            vec![(Lane::Even, 0, vec![1]), (Lane::Odd, 2, vec![2])],
            "offset 0 requires exactly one even and one odd part",
        ),
        (
            vec![
                (Lane::Even, 0, vec![1]),
                (Lane::Odd, 0, vec![2]),
                (Lane::Even, 2, vec![3]),
            ],
            "offset 2 requires exactly one even and one odd part",
        ),
        (
            vec![(Lane::Even, 0, vec![1]), (Lane::Odd, 0, vec![2, 3])],
            "parts at offset 0 have unequal sizes: even 1, odd 2",
        ),
        (
            vec![(Lane::Even, 0, vec![]), (Lane::Odd, 0, vec![])],
            "parts at offset 0 must be nonempty",
        ),
        (
            vec![(Lane::Even, 2, vec![1]), (Lane::Odd, 2, vec![2])],
            "parts leave a gap at offset 2; expected offset 0",
        ),
        (
            vec![
                (Lane::Even, 0, vec![1]),
                (Lane::Odd, 0, vec![2]),
                (Lane::Even, 4, vec![3]),
                (Lane::Odd, 4, vec![4]),
            ],
            "parts leave a gap at offset 4; expected offset 2",
        ),
        (
            vec![
                (Lane::Even, 0, vec![1, 2]),
                (Lane::Odd, 0, vec![3, 4]),
                (Lane::Even, 2, vec![5]),
                (Lane::Odd, 2, vec![6]),
            ],
            "parts overlap at offset 2; expected offset 4",
        ),
    ];
    for (parts, message) in cases {
        let expected = format!("firmware parts: {message}");
        assert_eq!(interleave_parts(&parts).unwrap_err().to_string(), expected);
        let directory = PartsDirectory::new();
        let mut config = crate::bios(vec![0; 8192], None);
        config.images[0].source = directory.source(&parts);
        for allow_unlisted_firmware in [false, true] {
            assert_eq!(
                resolve_file_sources(
                    &config,
                    AssemblyOptions {
                        allow_unlisted_firmware
                    }
                )
                .unwrap_err()
                .to_string(),
                expected
            );
        }
    }
}
#[test]
fn parts_reject_drives_ramloads_and_option_roms_before_io() {
    let mut config = crate::bios(vec![0; 8192], Some(vec![0; 1_474_560]));
    config.drives[0].image = ImageSource::Interleaved(vec![]);
    assert_eq!(
        config.validate().unwrap_err().to_string(),
        "firmware parts: drives cannot use parts"
    );
    config.drives.clear();
    config.images.push(crate::LoadImage {
        base: 0,
        source: ImageSource::Interleaved(vec![]),
        kind: ImageKind::RamLoad,
        provides: vec![],
    });
    assert_eq!(
        config.validate().unwrap_err().to_string(),
        "firmware parts: RAM load images cannot use parts"
    );
    config.images[1].kind = ImageKind::Rom;
    config.images[1].base = 0xC8000;
    assert_eq!(
        config.validate().unwrap_err().to_string(),
        "firmware parts: option ROMs cannot use parts; only firmware.bios supports parts"
    );
}
#[test]
fn parts_and_preinterleaved_file_match_same_synthetic_entry() {
    let directory = PartsDirectory::new();
    let parts = pair();
    let bytes = interleave_parts(&parts).unwrap();
    let entry = synthetic_entry(&bytes);
    let path = directory.0.join("assembled");
    std::fs::write(&path, &bytes).unwrap();
    for source in [directory.source(&parts), ImageSource::File(path)] {
        let mut config = crate::bios(vec![0; 8192], None);
        config.images[0].source = source;
        config.images[0].base = 0xFE000;
        let (resolved, _) = resolve_file_sources(&config, AssemblyOptions::default()).unwrap();
        assert_eq!(
            resolved.images[0].source,
            ImageSource::Inline(bytes.clone())
        );
        assert_eq!(
            crate::firmware::matched_entry(&resolved.images[0], &[entry]),
            Some(&entry)
        );
        assert_eq!(
            check_system_bios(
                &resolved,
                BoardModel::Ibm5170,
                &[entry],
                0xFE000,
                &bytes,
                AssemblyOptions::default()
            )
            .unwrap(),
            None
        );
    }
}
#[test]
fn parts_diagnose_swapped_lanes_and_each_wrong_chip() {
    let original = pair();
    let bytes = interleave_parts(&original).unwrap();
    let entry = synthetic_entry(&bytes);
    let mut swapped = original.clone();
    swapped[0].0 = Lane::Odd;
    swapped[1].0 = Lane::Even;
    let swapped_bytes = interleave_parts(&swapped).unwrap();
    let error = check_firmware_parts(
        &swapped,
        &swapped_bytes,
        &[entry],
        AssemblyOptions::default(),
    )
    .unwrap_err();
    let expected = concat!(
        "firmware parts are swapped for synthetic-user-bios: ",
        "odd lane offset 0 holds U27 synthetic-even, which belongs in the even lane at offset 0; ",
        "even lane offset 0 holds U47 synthetic-odd, which belongs in the odd lane at offset 0"
    );
    assert_eq!(error.to_string(), expected);
    for index in 0..2 {
        let mut wrong = original.clone();
        wrong[index].2[4095] ^= 1;
        let wrong_bytes = interleave_parts(&wrong).unwrap();
        let expected = entry.parts[0][index];
        let error =
            check_firmware_parts(&wrong, &wrong_bytes, &[entry], AssemblyOptions::default())
                .unwrap_err();
        let actual_hash = format!("{:x}", Sha256::digest(&wrong[index].2));
        let message = format!(
            "firmware parts do not match synthetic-user-bios: {} lane offset 0: \
             expected {} {} sha256 {}, actual {actual_hash} (bad dump or wrong chip)",
            expected.lane, expected.label, expected.part_number, expected.sha256
        );
        assert_eq!(error.to_string(), message);
        assert!(check_firmware_parts(
            &wrong,
            &wrong_bytes,
            &[entry],
            AssemblyOptions {
                allow_unlisted_firmware: true
            }
        )
        .is_ok());
    }
    assert!(check_firmware_parts(
        &swapped,
        &swapped_bytes,
        &[entry],
        AssemblyOptions {
            allow_unlisted_firmware: true
        }
    )
    .is_ok());
    let mut config = crate::bios(vec![0; 8192], None);
    config.images[0].source = ImageSource::Inline(vec![0; 8192]);
    let error = check_system_bios(
        &config,
        BoardModel::Ibm5170,
        &[entry],
        0xFE000,
        &vec![0; 8192],
        AssemblyOptions::default(),
    )
    .unwrap_err();
    let unknown_hash = format!("{:x}", Sha256::digest(vec![0; 8192]));
    let expected = format!(
        "bios image (sha256 {unknown_hash}) is not a known image for board \"ibm-5170\"; \
         known: synthetic-user-bios (user-supplied: U27 synthetic-even even, \
         U47 synthetic-odd odd; see vendor/user-roms/README.md)"
    );
    assert_eq!(error.to_string(), expected);
    let unknown = vec![
        (Lane::Even, 0, vec![9; 4096]),
        (Lane::Odd, 0, vec![8; 4096]),
    ];
    assert!(check_firmware_parts(
        &unknown,
        &interleave_parts(&unknown).unwrap(),
        &[entry],
        AssemblyOptions::default()
    )
    .is_ok());
}

#[test]
fn parts_production_catalogue_exact_data_and_tiling() {
    let catalogue = BoardModel::Ibm5170.firmware_catalogue();
    assert_eq!(
        catalogue
            .iter()
            .filter(|e| e.distribution == FirmwareDistribution::UserSupplied
                && e.kind == crate::FirmwareKind::SystemBios)
            .count(),
        4
    );
    let entry = catalogue
        .iter()
        .find(|e| e.id == "ibm-5170-bios-1984-01-10")
        .unwrap();
    assert_eq!(
        entry.sha256,
        "74e7b36b4ec0adc5ac3277a887579996c1d2aa755b9892ef3afe7485c10ce04f"
    );
    assert_eq!(entry.size, 65536);
    assert_eq!(entry.kind, crate::FirmwareKind::SystemBios);
    assert_eq!(entry.base, crate::FirmwareBase::TopOfFirstMib);
    assert_eq!(entry.scan_window, Some((0xC8000, 0xE0000)));
    assert_eq!(
        entry.provides,
        &[
            crate::FirmwareService::Floppy,
            crate::FirmwareService::FixedDisk
        ]
    );
    assert_eq!(
        entry.capabilities,
        &[crate::FirmwareCapability::CmosDriveTypes]
    );
    assert_eq!(
        entry.requires,
        &[
            crate::FirmwarePart::Kbc8042,
            crate::FirmwarePart::RtcCmos,
            crate::FirmwarePart::PicCascade
        ]
    );
    assert_eq!(entry.cpu_min, crate::FirmwareCpu::I80286);
    assert_eq!(entry.boards, &["ibm-5170"]);
    assert_eq!(entry.distribution, FirmwareDistribution::UserSupplied);
    assert_eq!(entry.status, FirmwareStatus::Listed);
    assert_eq!(entry.parts.len(), 2);
    assert_eq!(
        entry.parts[0],
        &[
            KnownPart {
                label: "U27",
                part_number: "6181028",
                lane: Lane::Even,
                offset: 0,
                size: 32768,
                sha256: "a7d0f995c25a29f2556fce563939cbd101f9225f71103debe441e3b6458cf933"
            },
            KnownPart {
                label: "U47",
                part_number: "6181029",
                lane: Lane::Odd,
                offset: 0,
                size: 32768,
                sha256: "fadb3a5c949b0096b2f753c76bac2edf73040066c1959eeee8f7d092c836b6c5"
            },
        ]
    );
    assert_eq!(
        entry.parts[1],
        &[
            KnownPart {
                label: "U17",
                part_number: "6181024",
                lane: Lane::Even,
                offset: 0,
                size: 16384,
                sha256: "b9a9aa7a32c4d50ac16034a489962cbd06455ab14cbb78a6d2bbf519b22c8a73"
            },
            KnownPart {
                label: "U37",
                part_number: "6181026",
                lane: Lane::Odd,
                offset: 0,
                size: 16384,
                sha256: "a54d9546361464489a118e6d7a7f9f959e6678013ab4f4e8a98993ac7bdc916d"
            },
            KnownPart {
                label: "U27",
                part_number: "6181025",
                lane: Lane::Even,
                offset: 32768,
                size: 16384,
                sha256: "87340797ab2f2eb86185f5289bd0a2bc2e9f14e0633770a87bf4cb0624208a5c"
            },
            KnownPart {
                label: "U47",
                part_number: "6181027",
                lane: Lane::Odd,
                offset: 32768,
                size: 16384,
                sha256: "a1106cc91a86251c524db72934f7eaa5abb39c2a986a5ac3477a017da7aac6e3"
            },
        ]
    );
    for layout in entry.parts {
        let parts: Vec<_> = layout
            .iter()
            .map(|p| (p.lane, p.offset, vec![0; p.size as usize]))
            .collect();
        assert_eq!(interleave_parts(&parts).unwrap().len(), 65536);
    }
    let entry = catalogue
        .iter()
        .find(|e| e.id == "ibm-5170-bios-1985-06-10")
        .unwrap();
    assert_eq!(
        entry.sha256,
        "6fb653b58e678cb69b0f943a105c6c803b18a6db23d13c0abccec5b66c6ee652"
    );
    assert_eq!(entry.size, 65536);
    assert_eq!(entry.kind, crate::FirmwareKind::SystemBios);
    assert_eq!(entry.base, crate::FirmwareBase::TopOfFirstMib);
    assert_eq!(entry.scan_window, Some((0xC8000, 0xE0000)));
    assert_eq!(
        entry.provides,
        &[
            crate::FirmwareService::Floppy,
            crate::FirmwareService::FixedDisk
        ]
    );
    assert_eq!(
        entry.capabilities,
        &[crate::FirmwareCapability::CmosDriveTypes]
    );
    assert_eq!(
        entry.requires,
        &[
            crate::FirmwarePart::Kbc8042,
            crate::FirmwarePart::RtcCmos,
            crate::FirmwarePart::PicCascade
        ]
    );
    assert_eq!(entry.cpu_min, crate::FirmwareCpu::I80286);
    assert_eq!(entry.boards, &["ibm-5170"]);
    assert_eq!(entry.distribution, FirmwareDistribution::UserSupplied);
    assert_eq!(entry.status, FirmwareStatus::Listed);
    assert_eq!(entry.parts.len(), 1);
    assert_eq!(
        entry.parts[0],
        &[
            KnownPart {
                label: "U27",
                part_number: "6480090",
                lane: Lane::Even,
                offset: 0,
                size: 32768,
                sha256: "c8654ed7a922644db2b2cce54c540fcfa7cbbae8c514d7eb3dbfbb8187471bfd"
            },
            KnownPart {
                label: "U47",
                part_number: "6480091",
                lane: Lane::Odd,
                offset: 0,
                size: 32768,
                sha256: "470ab30f9eb7d56c24f5ce10c7d307c539b9f923ea97dd536d2d5ba7e36d471b"
            },
        ]
    );
    for layout in entry.parts {
        let parts: Vec<_> = layout
            .iter()
            .map(|p| (p.lane, p.offset, vec![0; p.size as usize]))
            .collect();
        assert_eq!(interleave_parts(&parts).unwrap().len(), 65536);
    }
    let entry = catalogue
        .iter()
        .find(|e| e.id == "ibm-5170-bios-1985-11-15")
        .unwrap();
    assert_eq!(
        entry.sha256,
        "872febf7d7034bfadf72ed1936e06600eca72456b9ded41d15b7a284de20337d"
    );
    assert_eq!(entry.size, 65536);
    assert_eq!(entry.kind, crate::FirmwareKind::SystemBios);
    assert_eq!(entry.base, crate::FirmwareBase::TopOfFirstMib);
    assert_eq!(entry.scan_window, Some((0xC8000, 0xE0000)));
    assert_eq!(
        entry.provides,
        &[
            crate::FirmwareService::Floppy,
            crate::FirmwareService::FixedDisk
        ]
    );
    assert_eq!(
        entry.capabilities,
        &[crate::FirmwareCapability::CmosDriveTypes]
    );
    assert_eq!(
        entry.requires,
        &[
            crate::FirmwarePart::Kbc8042,
            crate::FirmwarePart::RtcCmos,
            crate::FirmwarePart::PicCascade
        ]
    );
    assert_eq!(entry.cpu_min, crate::FirmwareCpu::I80286);
    assert_eq!(entry.boards, &["ibm-5170"]);
    assert_eq!(entry.distribution, FirmwareDistribution::UserSupplied);
    assert_eq!(entry.status, FirmwareStatus::Listed);
    assert_eq!(entry.parts.len(), 1);
    assert_eq!(
        entry.parts[0],
        &[
            KnownPart {
                label: "U27",
                part_number: "61X9266",
                lane: Lane::Even,
                offset: 0,
                size: 32768,
                sha256: "3b921a451d78efcd91232a01cd6936d6ba12dad4374466655aff8ef3a5dc764e"
            },
            KnownPart {
                label: "U47",
                part_number: "61X9265",
                lane: Lane::Odd,
                offset: 0,
                size: 32768,
                sha256: "aec1759a8245d50be93c9b081ab1ac7de58dd8141f603c7745ae66d2d4c29e4d"
            },
        ]
    );
    for layout in entry.parts {
        let parts: Vec<_> = layout
            .iter()
            .map(|p| (p.lane, p.offset, vec![0; p.size as usize]))
            .collect();
        assert_eq!(interleave_parts(&parts).unwrap().len(), 65536);
    }
}

fn layout_entry(parts: &[(Lane, u64, Vec<u8>)]) -> FirmwareEntry {
    let assembled = interleave_parts(parts).unwrap();
    let two = [
        (
            Lane::Even,
            0,
            [parts[0].2.clone(), parts[2].2.clone()].concat(),
        ),
        (
            Lane::Odd,
            0,
            [parts[1].2.clone(), parts[3].2.clone()].concat(),
        ),
    ];
    let known = |parts: &[(Lane, u64, Vec<u8>)], labels: &[(&'static str, &'static str)]| {
        Box::leak(
            parts
                .iter()
                .zip(labels)
                .map(|((lane, offset, bytes), (label, number))| KnownPart {
                    label,
                    part_number: number,
                    lane: *lane,
                    offset: *offset,
                    size: bytes.len() as u64,
                    sha256: Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str()),
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        ) as &'static [KnownPart]
    };
    FirmwareEntry {
        id: "synthetic-two-layout-bios",
        sha256: Box::leak(format!("{:x}", Sha256::digest(assembled)).into_boxed_str()),
        size: 8,
        parts: Box::leak(
            vec![
                known(&two, &[("U27", "pair-even"), ("U47", "pair-odd")]),
                known(
                    parts,
                    &[
                        ("U17", "low-even"),
                        ("U37", "low-odd"),
                        ("U27", "high-even"),
                        ("U47", "high-odd"),
                    ],
                ),
            ]
            .into_boxed_slice(),
        ),
        distribution: FirmwareDistribution::UserSupplied,
        ..crate::KISELEV_XI8088
    }
}
fn four_parts() -> Vec<(Lane, u64, Vec<u8>)> {
    vec![
        (Lane::Even, 0, vec![0x10; 2]),
        (Lane::Odd, 0, vec![0x20; 2]),
        (Lane::Even, 4, vec![0x30; 2]),
        (Lane::Odd, 4, vec![0x40; 2]),
    ]
}
fn diagnose(parts: &[(Lane, u64, Vec<u8>)], entries: &[FirmwareEntry]) -> String {
    check_firmware_parts(
        parts,
        &interleave_parts(parts).unwrap(),
        entries,
        AssemblyOptions::default(),
    )
    .unwrap_err()
    .to_string()
}
#[test]
fn parts_two_layouts_name_both_bad_chips_and_choose_most_hash_matches() {
    let original = four_parts();
    let entry = layout_entry(&original);
    let mut bad = original.clone();
    bad[1].2[0] = 9;
    bad[3].2[0] = 8;
    let message = format!(
        "firmware parts do not match synthetic-two-layout-bios: \
         odd lane offset 0: expected U37 low-odd sha256 {}, actual {:x} (bad dump or wrong chip); \
         odd lane offset 4: expected U47 high-odd sha256 {}, actual {:x} (bad dump or wrong chip)",
        entry.parts[1][1].sha256,
        Sha256::digest(&bad[1].2),
        entry.parts[1][3].sha256,
        Sha256::digest(&bad[3].2)
    );
    assert_eq!(diagnose(&bad, &[entry]), message);
    let mut decoy_layout = entry.parts[1].to_vec();
    for part in &mut decoy_layout[1..] {
        part.sha256 = "unknown";
    }
    let decoy = FirmwareEntry {
        id: "decoy",
        parts: Box::leak(
            vec![Box::leak(decoy_layout.into_boxed_slice()) as &'static [KnownPart]]
                .into_boxed_slice(),
        ),
        ..entry
    };
    assert_eq!(diagnose(&bad, &[decoy, entry]), message);
}
#[test]
fn parts_partial_pair_swap_and_exchanged_halves_identify_each_chip() {
    let original = four_parts();
    let entry = layout_entry(&original);
    let mut swapped = original.clone();
    swapped.swap(0, 1);
    swapped[0].0 = Lane::Even;
    swapped[1].0 = Lane::Odd;
    assert_eq!(
        diagnose(&swapped, &[entry]),
        concat!(
            "firmware parts are swapped for synthetic-two-layout-bios: ",
            "even lane offset 0 holds U37 low-odd, which belongs in the odd lane at offset 0; ",
            "odd lane offset 0 holds U17 low-even, which belongs in the even lane at offset 0"
        )
    );
    let mut halves = original.clone();
    halves.swap(0, 2);
    halves.swap(1, 3);
    halves[0].1 = 0;
    halves[1].1 = 0;
    halves[2].1 = 4;
    halves[3].1 = 4;
    assert_eq!(
        diagnose(&halves, &[entry]),
        concat!(
            "firmware parts do not match synthetic-two-layout-bios: ",
            "even lane offset 0 holds U27 high-even, which belongs in the even lane at offset 4; ",
            "odd lane offset 0 holds U47 high-odd, which belongs in the odd lane at offset 4; ",
            "even lane offset 4 holds U17 low-even, which belongs in the even lane at offset 0; ",
            "odd lane offset 4 holds U37 low-odd, which belongs in the odd lane at offset 0"
        )
    );
    let two = vec![
        (
            Lane::Even,
            0,
            [original[1].2.clone(), original[3].2.clone()].concat(),
        ),
        (
            Lane::Odd,
            0,
            [original[0].2.clone(), original[2].2.clone()].concat(),
        ),
    ];
    assert_eq!(
        diagnose(&two, &[entry]),
        concat!(
            "firmware parts are swapped for synthetic-two-layout-bios: ",
            "even lane offset 0 holds U47 pair-odd, which belongs in the odd lane at offset 0; ",
            "odd lane offset 0 holds U27 pair-even, which belongs in the even lane at offset 0"
        )
    );
}
#[test]
fn parts_layout_selection_filters_part_count_and_listing_shows_offsets() {
    let original = four_parts();
    let entry = layout_entry(&original);
    let mut six = entry.parts[1].to_vec();
    six[1].label = "DECOY";
    six.extend([
        KnownPart {
            offset: 8,
            ..six[0]
        },
        KnownPart {
            offset: 8,
            ..six[1]
        },
    ]);
    let six = Box::leak(six.into_boxed_slice()) as &'static [KnownPart];
    let filtered = FirmwareEntry {
        parts: Box::leak(vec![six, entry.parts[1]].into_boxed_slice()),
        ..entry
    };
    let mut bad = original;
    bad[1].2[0] = 9;
    assert_eq!(diagnose(&bad, &[filtered]), diagnose(&bad, &[entry]));
    assert_eq!(
        entry.listing(),
        concat!(
            "synthetic-two-layout-bios (user-supplied: U27 pair-even even, U47 pair-odd odd or ",
            "U17 low-even even@0, U37 low-odd odd@0, U27 high-even even@4, U47 high-odd odd@4; ",
            "see vendor/user-roms/README.md)"
        )
    );
}
#[test]
fn parts_unresolved_bios_without_declared_index_tolerates_inline_option_rom_like_file() {
    let mut config = crate::bios(vec![], Some(vec![0; 1_474_560]));
    config.system_bios_image = None;
    let mut option = vec![0; 2048];
    option[..3].copy_from_slice(&[0x55, 0xAA, 4]);
    option[2047] = 0xFD;
    config.images.push(crate::LoadImage {
        base: 0xC8000,
        source: ImageSource::Inline(option),
        kind: ImageKind::Rom,
        provides: vec![],
    });
    for source in [
        ImageSource::File("unread-bios".into()),
        ImageSource::Interleaved(vec![]),
    ] {
        config.images[0].source = source;
        assert_eq!(config.validate(), Ok(()));
    }
}

#[test]
fn parts_deferred_option_rom_source_is_rejected_after_resolution() {
    let directory = PartsDirectory::new();
    let path = directory.0.join("bios");
    std::fs::write(&path, vec![0; 32768]).unwrap();
    let mut config = crate::bios(vec![], Some(vec![0; 1_474_560]));
    config.system_bios_image = None;
    config.images[0].source = ImageSource::File(path);
    config.images.push(crate::LoadImage {
        base: 0xC8000,
        source: directory.source(&pair()),
        kind: ImageKind::Rom,
        provides: vec![],
    });
    config.validate().unwrap();
    let error = resolve_file_sources(&config, AssemblyOptions::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "firmware parts: option ROMs cannot use parts; only firmware.bios supports parts"
    );
}

#[test]
fn parts_duplicate_two_chip_input_names_duplicate_and_missing_chip() {
    let mut parts = pair();
    let entry = synthetic_entry(&interleave_parts(&parts).unwrap());
    parts[1].2 = parts[0].2.clone();
    assert_eq!(
        diagnose(&parts, &[entry]),
        concat!(
            "firmware parts do not match synthetic-user-bios: ",
            "U27 synthetic-even appears 2 times; U47 synthetic-odd (odd lane offset 0) is missing"
        )
    );
}
#[test]
fn parts_duplicate_four_chip_input_names_duplicate_and_missing_chip() {
    let mut parts = four_parts();
    let entry = layout_entry(&parts);
    parts[1].2 = parts[0].2.clone();
    assert_eq!(
        diagnose(&parts, &[entry]),
        concat!(
            "firmware parts do not match synthetic-two-layout-bios: ",
            "U17 low-even appears 2 times; U37 low-odd (odd lane offset 0) is missing"
        )
    );
}
#[test]
fn parts_unknown_offsets_are_reported_for_every_supplied_part() {
    let original = vec![
        (Lane::Even, 0, vec![0x10; 16384]),
        (Lane::Odd, 0, vec![0x20; 16384]),
        (Lane::Even, 32768, vec![0x30; 24576]),
        (Lane::Odd, 32768, vec![0x40; 24576]),
    ];
    let entry = FirmwareEntry {
        size: 81920,
        ..layout_entry(&original)
    };
    let parts = vec![
        (Lane::Even, 0, original[2].2.clone()),
        (Lane::Odd, 0, original[3].2.clone()),
        (Lane::Even, 49152, vec![9; 16384]),
        (Lane::Odd, 49152, vec![8; 16384]),
    ];
    assert_eq!(
        diagnose(&parts, &[entry]),
        concat!(
        "firmware parts do not match synthetic-two-layout-bios: ",
        "even lane offset 0 holds U27 high-even, which belongs in the even lane at offset 32768; ",
        "odd lane offset 0 holds U47 high-odd, which belongs in the odd lane at offset 32768; ",
        "even lane offset 49152 is not part of synthetic-two-layout-bios layout; ",
        "odd lane offset 49152 is not part of synthetic-two-layout-bios layout"
    )
    );
}
#[test]
fn parts_option_rom_role_precedes_layout_errors_and_part_reads() {
    let directory = PartsDirectory::new();
    let path = directory.0.join("bios");
    std::fs::write(&path, vec![0; 32768]).unwrap();
    let mut config = crate::bios(vec![], Some(vec![0; 1_474_560]));
    config.system_bios_image = None;
    config.images[0].source = ImageSource::File(path);
    for source in [
        ImageSource::Interleaved(vec![]),
        ImageSource::Interleaved(vec![ImagePart {
            path: directory.0.join("absent-part"),
            lane: Lane::Even,
            offset: 0,
        }]),
    ] {
        config.images.truncate(1);
        config.images.push(crate::LoadImage {
            base: 0xC8000,
            source,
            kind: ImageKind::Rom,
            provides: vec![],
        });
        let error = resolve_file_sources(&config, AssemblyOptions::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "firmware parts: option ROMs cannot use parts; only firmware.bios supports parts"
        );
    }
}

#[test]
fn parts_resolution_call_site_diagnoses_swapped_and_bad_chips_and_preserves_escape() {
    let directory = PartsDirectory::new();
    let original = pair();
    let expected_bytes = interleave_parts(&original).unwrap();
    let entry = synthetic_entry(&expected_bytes);
    let mut config = crate::bios(vec![], Some(vec![0; 1_474_560]));
    config.images[0].base = 0xFE000;
    config.images[0].source = directory.source(&original);
    let (resolved, _) =
        resolve_file_sources_with_catalogue(&config, AssemblyOptions::default(), &[entry]).unwrap();
    assert_eq!(
        resolved.images[0].source,
        ImageSource::Inline(expected_bytes)
    );
    let mut swapped = original.clone();
    swapped[0].0 = Lane::Odd;
    swapped[1].0 = Lane::Even;
    config.images[0].source = directory.source(&swapped);
    let error = resolve_file_sources_with_catalogue(&config, AssemblyOptions::default(), &[entry])
        .map(|_| ())
        .expect_err("expected swapped chip diagnosis");
    assert_eq!(
        error.to_string(),
        concat!(
        "firmware parts are swapped for synthetic-user-bios: ",
        "odd lane offset 0 holds U27 synthetic-even, which belongs in the even lane at offset 0; ",
        "even lane offset 0 holds U47 synthetic-odd, which belongs in the odd lane at offset 0"
    )
    );
    resolve_file_sources_with_catalogue(
        &config,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
        &[entry],
    )
    .unwrap();
    let mut bad = original;
    bad[1].2[0] ^= 1;
    config.images[0].source = directory.source(&bad);
    let error = resolve_file_sources_with_catalogue(&config, AssemblyOptions::default(), &[entry])
        .map(|_| ())
        .expect_err("expected bad chip diagnosis");
    let expected = format!(
        "firmware parts do not match synthetic-user-bios: odd lane offset 0: \
         expected U47 synthetic-odd sha256 {}, actual {:x} (bad dump or wrong chip)",
        entry.parts[0][1].sha256,
        Sha256::digest(&bad[1].2)
    );
    assert_eq!(error.to_string(), expected);
    resolve_file_sources_with_catalogue(
        &config,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
        &[entry],
    )
    .unwrap();
}

#[test]
fn video_container_file_resolution_retains_complete_identity_and_mapped_size() {
    let directory = PartsDirectory::new();
    let mut bytes = vec![0; 32768];
    bytes[8192..8196].copy_from_slice(&[0x55, 0xAA, 0x30, 0xCB]);
    let sum = bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    bytes[32767] = 0u8.wrapping_sub(sum);
    let path = directory.0.join("video.bin");
    std::fs::write(&path, &bytes).unwrap();
    let video = FirmwareEntry {
        sha256: Box::leak(format!("{:x}", Sha256::digest(&bytes)).into_boxed_str()),
        requires: &[],
        ..FirmwareEntry::IBM_VGA_1987
    };
    let mut config = crate::bios(vec![0; 32768], Some(vec![0; 1_474_560]));
    config.images.push(LoadImage {
        base: 0xC0000,
        source: ImageSource::File(path),
        kind: ImageKind::Rom,
        provides: Vec::new(),
    });
    let (resolved, _) =
        resolve_file_sources_with_catalogue(&config, AssemblyOptions::default(), &[video]).unwrap();
    assert_eq!(resolved.images[1].source.known_len(), Some(24576));
    assert!(matches!(
        resolved.images[1].source,
        ImageSource::RomContainer { offset: 8192, .. }
    ));
    assert_eq!(
        crate::firmware::matched_entry(&resolved.images[1], &[video]),
        Some(&video)
    );
    let mapped = load_image_bytes(&resolved.images[1].source).unwrap();
    assert_eq!(mapped, bytes[8192..]);
}

#[test]
fn video_container_used_as_system_file_is_not_sliced() {
    let directory = PartsDirectory::new();
    let bytes = vec![0; 32768];
    let path = directory.0.join("video.bin");
    std::fs::write(&path, &bytes).unwrap();
    let entry = FirmwareEntry {
        sha256: Box::leak(format!("{:x}", Sha256::digest(&bytes)).into_boxed_str()),
        ..FirmwareEntry::IBM_VGA_1987
    };
    let mut config = crate::bios(vec![0; 32768], Some(vec![0; 1_474_560]));
    config.images[0].source = ImageSource::File(path);
    config.images.push(LoadImage {
        base: 0xC8000,
        source: ImageSource::Inline(vec![0; 2048]),
        kind: ImageKind::Rom,
        provides: vec![],
    });
    for marker in [Some(0), None] {
        config.system_bios_image = marker;
        let (resolved, _) =
            resolve_file_sources_with_catalogue(&config, AssemblyOptions::default(), &[entry])
                .unwrap();
        assert!(matches!(resolved.images[0].source, ImageSource::Inline(_)));
        assert_eq!(resolved.images[0].source.known_len(), Some(32768));
        let error = check_system_bios(
            &resolved,
            BoardModel::Ibm5170,
            &[entry],
            0xF8000,
            &load_image_bytes(&resolved.images[0].source).unwrap(),
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "video BIOS ibm-vga-bios-1987-32k cannot be used as the system BIOS"
        );
    }
}
