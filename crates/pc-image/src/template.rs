// SPDX-License-Identifier: AGPL-3.0-or-later
//! Official template images, built reproducibly from the fetched open
//! fixtures; machine-images §5.5.
//!
//! A build creates each template machine through [`crate::create`], never
//! from a machine file, from the open firmware packages it binds. Nothing
//! depends on the clock, the host or the fixture paths: the same fixtures
//! give the same manifests and digests on every run and host. Each template
//! is offered to a caller's guard before anything of it is tagged, so a
//! build can refuse what the official namespace would refuse. A template
//! holds firmware and blank zero-base disks only; operating systems on disk
//! images need their own licence decision.
//!
//! The firmware packages ([`FIRMWARE`], [`build_firmware`]) and the
//! installation sets ([`MEDIA`], [`build_media`]) are published by the
//! unemu-media repository, not by this build. The local builders here make
//! the same images from the same fixtures under the published names, and
//! their tests pin the published digests, so each pin cross-checks the two
//! repositories. A template build imports the packages it binds with
//! [`build_firmware`], the same packages `image pull` gives. A package holds
//! the source archive its fetch script builds (the archive the firmware
//! catalogue pins) and states a title, description, vendor, version and
//! labels. An installation set holds an official release's disks for one
//! floppy format, unmodified, with a licence index and the source archive,
//! open under the licences of its programs (machine-images §5.2).
//!
//! The machines are IBM 5170-class boards, as the IBM Personal Computer AT
//! Technical Reference, System Board, describes: an 80286 with 640 KiB
//! conventional and 1 MiB extended memory. The video BIOS and the fixed-disk
//! ROM take the adapter-ROM placement their catalogue entries give ("Adapters
//! with System-Accessible ROM Modules").

use crate::create::{self, Chs, Ems, Hardware, NewImage, PackageRom, Sb16, Serial, Video};
use crate::firmware_import::{self, Statement};
use crate::media_import::{self, ExtraSource, OpenStatement, Request, Source};
use crate::media_set::PcMediaFormats;
use crate::{invalid, PcProvider, Result};
use image_store::{
    media_set::{ExtraKind, Validator as MediaValidator},
    metadata::Title,
    ConfigValidator, Descriptor, Digest, Manifest, Store,
};
use std::path::{Path, PathBuf};

/// The version tag of this template generation. Version tags never move, so
/// a changed template needs a new version here.
pub const VERSION: &str = "1.2.0";
/// The moving tag that follows the newest version.
pub const ALIAS: &str = "latest";
/// The registry namespace every template lives in.
pub const NAMESPACE: &str = crate::OPEN_NAMESPACE;
/// The tag of every firmware package: its first revision, numbered as
/// `image firmware import` numbers them. A revision tag never moves, and a
/// package without a release has no alias (recipes.md §8.3).
pub const FIRMWARE_REVISION: &str = "r1";

/// One open firmware package of the templates: the catalogue entry, the
/// fixture files, relative to the vendor directory, that its fetch script
/// produces, and the descriptive fields the package states.
#[derive(Clone, Copy, Debug)]
pub struct FirmwareSource {
    pub catalogue_id: &'static str,
    pub rom: &'static str,
    pub source_archive: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub vendor: &'static str,
    /// The release tag or revision the catalogue provenance names.
    pub version: &'static str,
    pub labels: &'static [&'static str],
}

impl FirmwareSource {
    /// `unemu/firmware/<platform>/<catalogue id>`.
    pub fn repository(&self) -> String {
        crate::firmware_repository(NAMESPACE, self.catalogue_id)
    }

    /// What the build states for the package, as `image firmware import`
    /// states it with its metadata flags: the descriptive fields and the
    /// source archive at `archive`. Registry web UIs read the title,
    /// description, vendor and version from the manifest annotations this
    /// gives; without them they show the raw catalogue id.
    pub fn statement(&self, archive: PathBuf) -> Statement {
        Statement {
            title: Some(self.title.into()),
            description: Some(self.description.into()),
            vendor: Some(self.vendor.into()),
            version: Some(self.version.into()),
            labels: Some(self.labels.iter().map(|l| (*l).into()).collect()),
            source: Some(archive),
            ..Statement::default()
        }
    }
}

/// The system BIOS first, then the video BIOS and the fixed-disk ROM, in the
/// order a template binds them.
pub const FIRMWARE: [FirmwareSource; 3] = [
    FirmwareSource {
        catalogue_id: "unemu-at-bios-1.0.2-unemu.3",
        rom: "unemu-at-bios/bios-unemu-at.bin",
        source_archive: "unemu-at-bios/unemu-at-bios-1.0.2-unemu.3-src.tar",
        title: "unemu AT BIOS 1.0.2-unemu.3",
        description: "The system BIOS of the open AT templates: release 1.0.2-unemu.3 of unemu-at-bios, the unemu fork of 8088_bios 1.0.2 by Sergey Kiselev, its original author. Its unemu-at build is a 32 KiB ROM at F8000h-FFFFFh for IBM 5170-class 80286 boards with two 8259s, two 8237s, an 8042 and an MC146818. Unlike upstream it resumes through the CMOS shutdown byte, answers INT 15h AH=87h and AH=88h, measures extended memory into CMOS 30h-31h, clears the memory it finds above 1 MiB on every POST and checks the CMOS as the IBM PC/AT BIOS does. It serves diskettes only; fixed disks come from an option ROM such as the XTIDE Universal BIOS. Licensed under GPL-3.0-or-later; the package includes its corresponding source archive.",
        vendor: "unemu (fork of Sergey Kiselev's 8088_bios)",
        version: "1.0.2-unemu.3",
        labels: &["open-source", "pc-at", "system-bios"],
    },
    FirmwareSource {
        catalogue_id: "vgabios-0.9d-unemu.1-std",
        rom: "vgabios/vgabios-std.bin",
        source_archive: "vgabios/vgabios-0.9d-unemu.1-src.tar",
        title: "unemu VGA BIOS 0.9d-unemu.1, standard VGA",
        description: "The video BIOS of the open AT templates: the unemu fork of the LGPL VGABIOS 0.9d, the video BIOS Bochs and QEMU use, in its standard ISA VGA build with no VBE, PCI, Cirrus or Banshee code and 80286 instructions only. The 32 KiB ROM fills the C0000h-C7FFFh video-ROM window of an IBM 5170-class board with a register-level VGA card. Unlike upstream, teletype output sounds the PC speaker for BEL (07h): about 896 Hz for about 0.48 s, as the IBM PC/AT BIOS beeps. Licensed under LGPL-2.1-or-later; the package includes its corresponding source archive.",
        vendor: "unemu (fork of the LGPL VGABIOS project)",
        version: "0.9d-unemu.1",
        labels: &["open-source", "pc-at", "vga", "video-bios"],
    },
    FirmwareSource {
        catalogue_id: "xub-r638-xtp-ata1f0",
        rom: "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
        source_archive: "xtide-universal-bios/xtide-universal-bios-r638-src.tar",
        title: "XTIDE Universal BIOS r638, XT+ build for ATA at 1F0h",
        description: "The fixed-disk option ROM of the open AT templates: the XTIDE Universal BIOS r638 XT+ build (ide_xtp.bin) with its first controller set to 16-bit ATA at 1F0h (control block 3F0h), polled without an IRQ, and diskette A: as the boot drive; every other setting keeps its upstream default. The download is padded with zeros to its declared 8 KiB and its last byte makes the byte sum zero. It serves IBM 5170-class boards with a primary ATA channel, suggested at C8000h, on an 80186 or later. Licensed under GPL-2.0-only, as the project's binary download statement says; the package includes its corresponding source archive, the r638 Subversion trunk.",
        vendor: "XTIDE Universal BIOS project",
        version: "r638",
        labels: &["ata", "open-source", "option-rom", "pc-at"],
    },
];

/// One template machine.
#[derive(Clone, Copy, Debug)]
pub struct Template {
    /// The repository name inside the official namespace.
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// Sound Blaster 16, a serial mouse on COM1 and a Lo-tech EMS board.
    pub games: bool,
}

impl Template {
    /// `unemu/<name>`.
    pub fn repository(&self) -> String {
        format!("{NAMESPACE}/{}", self.name)
    }

    /// The hardware: a 5.25-inch 1.2M A: and a 3.5-inch 1.44M B: diskette
    /// drive, both with an empty catalogue (the B: drive also takes 720K
    /// disks), and one blank 504 MiB disk (1024/16/63, the largest INT 13h CHS
    /// geometry) on the primary ATA channel, which XTIDE serves. The games template adds
    /// the shipped open game profile's cards: SB16 at 220h, IRQ 5, DMA 1 and
    /// 5; a Microsoft mouse on the COM1 UART; a 2 MiB Lo-tech EMS board at
    /// port 260h with its page frame at D000h.
    pub fn hardware(&self) -> Result<Hardware> {
        let board = assembly::BoardModel::Ibm5170;
        let speed = create::speeds(board)
            .into_iter()
            .find(|s| s.calibrated)
            .ok_or_else(|| invalid("the board has no calibrated speed"))?;
        Ok(Hardware {
            board,
            rate: (speed.steps, speed.pit_clocks),
            fpu: false,
            conventional_kib: 640,
            extended_kib: 1024,
            video: Video::Vga,
            sound: self.games.then_some(Sb16 {
                base: 0x220,
                irq: 5,
                dma8: 1,
                dma16: 5,
            }),
            serial: if self.games {
                Serial::Mouse
            } else {
                Serial::Empty
            },
            ems: self.games.then_some(Ems {
                port: 0x260,
                frame: 0xD000,
                size_kib: 2048,
            }),
            floppies: [Some("1.2M".into()), Some("1.44M".into())],
            ide_port: 0x1F0,
            disks: [Some(Chs::new(1024, 16, 63)), None],
        })
    }
}

pub const TEMPLATES: [Template; 2] = [
    Template {
        name: "open-at",
        title: "Open AT 286",
        description: "An IBM 5170-class 80286 with open firmware only: the unemu AT BIOS (after Sergey Kiselev's 8088_bios), the unemu VGA BIOS and XTIDE Universal BIOS r638, each with its source archive. 640 KiB and 1 MiB of memory, VGA, a 1.2M A: and a 1.44M B: diskette drive and a blank 504 MiB fixed disk. No operating system.",
        games: false,
    },
    Template {
        name: "open-at-games",
        title: "Open AT 286 for games",
        description: "Open AT 286 plus a Sound Blaster 16 (220h, IRQ 5, DMA 1 and 5), a Microsoft serial mouse on COM1 and a 2 MiB Lo-tech EMS board (port 260h, page frame D000h). No operating system.",
        games: true,
    },
];

/// One file a media set of the templates takes from the fixtures: its path
/// relative to the vendor directory, the SHA-256 its fetch script pins, and
/// the disk label, or the extra's DOS 8.3 name.
#[derive(Clone, Copy, Debug)]
pub struct MediaFile {
    pub path: &'static str,
    pub sha256: &'static str,
    pub name: &'static str,
}
const fn file(path: &'static str, sha256: &'static str, name: &'static str) -> MediaFile {
    MediaFile { path, sha256, name }
}

/// An official operating-system release whose installation disks the
/// templates describe, one installation set per floppy format, and what the
/// sets share: the work's repository and title, the release's catalogue
/// fields, its licence statement and the extras every set carries.
#[derive(Clone, Copy, Debug)]
pub struct MediaRelease {
    /// The repository name under `unemu/media/<platform>/`: the work,
    /// whatever its release or format, which the tag names.
    pub work: &'static str,
    /// The work's title only; the version, the edition and the floppy format
    /// have fields and a tag of their own.
    pub title: &'static str,
    /// The script that fetches and verifies the fixtures.
    pub fetch: &'static str,
    pub publisher: &'static str,
    pub year: i64,
    /// The release's version and edition as the title record states them.
    pub release: &'static str,
    pub edition: &'static str,
    pub genres: &'static [&'static str],
    pub labels: &'static [&'static str],
    /// The conjunction of the licences of everything on the disks.
    pub license: &'static str,
    /// The extras every set carries, in order: the licence index
    /// ([`Self::terms`]) and the source archive ([`Self::source`]).
    pub extras: &'static [(MediaFile, ExtraKind)],
    pub terms: &'static str,
    pub source: &'static str,
}

/// FreeDOS 1.4 Floppy Edition, released by the FreeDOS Project on 2025-04-02
/// (`freedos/version.fdi` on its boot disks). The licence index comes from
/// this repository and the source archive holds the official source
/// packages of every program on the disks. The release documents are not
/// carried: they state no licence of their own.
pub const FREEDOS_1_4: MediaRelease = MediaRelease {
    work: "freedos",
    title: "FreeDOS",
    fetch: "vendor/freedos/fetch-floppy-1.4.sh",
    publisher: "The FreeDOS Project",
    year: 2025,
    release: "1.4",
    edition: "Floppy Edition",
    genres: &["operating system"],
    labels: &["freedos", "open-source", "operating-system"],
    license: "GPL-1.0-only AND GPL-1.0-or-later AND GPL-2.0-only AND GPL-2.0-or-later AND GPL-3.0-or-later AND LGPL-2.1-or-later AND MIT AND BSD-2-Clause AND MPL-2.0 AND Watcom-1.0 AND Info-ZIP AND Artistic-1.0 AND GFDL-1.1-or-later AND Zlib AND LicenseRef-FreeDOS-other",
    extras: &[
        (
            file(
                "freedos/floppy-1.4/LICENSES.TXT",
                "35d9478a2f5b3e54ded8d20b1b23feec10b0abd2e1e6715c77ddf314e7bf1a7f",
                "LICENSES.TXT",
            ),
            ExtraKind::Document,
        ),
        (
            file(
                "freedos/floppy-1.4/freedos-1.4-floppy-src.tar",
                "dfa7006e742cb9676a0a6401ce64214089923f055f4a7ba8f36960115b5362b6",
                "FD14SRC.TAR",
            ),
            ExtraKind::Archive,
        ),
    ],
    terms: "LICENSES.TXT",
    source: "freedos-1.4-floppy-src.tar",
};

/// One installation set of the templates: a release's disks for one floppy
/// format, unmodified and in installer order.
#[derive(Clone, Copy, Debug)]
pub struct MediaSource {
    /// The set's revision tag in its work's repository: the release (the
    /// version, then the floppy format unless it is the edition's own) and
    /// the revision. It never moves; the release alone is the alias that
    /// follows the newest revision (recipes.md §8.2).
    pub tag: &'static str,
    pub description: &'static str,
    /// The floppy format of every disk.
    pub format: &'static str,
    pub disks: &'static [MediaFile],
    pub release: &'static MediaRelease,
}

impl MediaSource {
    /// `unemu/media/<platform>/<work>`.
    pub fn repository(&self) -> String {
        crate::media_repository(NAMESPACE, self.release.work)
    }

    /// The release alias the set's revision moves: its tag without the
    /// revision.
    pub fn alias(&self) -> &'static str {
        image_store::tag::alias_of(self.tag).unwrap_or(self.tag)
    }
}

/// The FreeDOS 1.4 Floppy Edition in each format it ships: its own 3.5-inch
/// 1.44M, then 5.25-inch 1.2M and 3.5-inch 720K.
pub const MEDIA: [MediaSource; 3] = [
    MediaSource {
        tag: "1.4-r1",
        description: "The official FreeDOS 1.4 Floppy Edition on seven 3.5-inch 1.44M diskettes, unmodified: the boot disk with the floppy installer, then six disks of packages in a Slicer archive. It installs FreeDOS 1.4 with a set of programs suited to classic PCs, the 80286 included, on any PC that runs FreeDOS with EGA or better graphics. Released by the FreeDOS Project on 2 April 2025. GPL and other open source licences, listed per program in LICENSES.TXT; the set carries the official FreeDOS source packages of its programs.",
        format: "1.44M",
        disks: &[
            file(
                "freedos/floppy-1.4/144m/x86BOOT.img",
                "552f7cbb0625960c050a3e682a4d2121cc2ffdfa9dc9d592ccd49edf179333dc",
                "Disk 1 of 7 - boot disk and installer (x86BOOT.img)",
            ),
            file(
                "freedos/floppy-1.4/144m/x86DSK01.img",
                "b9dd5315f50a8b770b3d2ecb623b18ed067660e22f421504d199254f7f653516",
                "Disk 2 of 7 - packages (x86DSK01.img)",
            ),
            file(
                "freedos/floppy-1.4/144m/x86DSK02.img",
                "4a951f15da8dd98e54ea84f07fe2a7802e914aa8e66942f646d2e11ca3943e59",
                "Disk 3 of 7 - packages (x86DSK02.img)",
            ),
            file(
                "freedos/floppy-1.4/144m/x86DSK03.img",
                "fbfd172e1ac4bc4abb3f656cae94e2727d2b48b4d02cef0362564048f83f35da",
                "Disk 4 of 7 - packages (x86DSK03.img)",
            ),
            file(
                "freedos/floppy-1.4/144m/x86DSK04.img",
                "894d49c4dfdd294e33878ca317e19a0322f267f639b247a6b2a669799b4720f8",
                "Disk 5 of 7 - packages (x86DSK04.img)",
            ),
            file(
                "freedos/floppy-1.4/144m/x86DSK05.img",
                "0362e084497c5c995ccab1523c4e39f94b7139f988ffa3a4f7e871452c352f3c",
                "Disk 6 of 7 - packages (x86DSK05.img)",
            ),
            file(
                "freedos/floppy-1.4/144m/x86DSK06.img",
                "7a93d1d4e47f5edd5d0a9f17d57c3417ae135052db9449a67aa374a3b4316d80",
                "Disk 7 of 7 - packages (x86DSK06.img)",
            ),
        ],
        release: &FREEDOS_1_4,
    },
    MediaSource {
        tag: "1.4-1.2m-r1",
        description: "The official FreeDOS 1.4 Floppy Edition on eight 5.25-inch 1.2M diskettes, unmodified: the boot disk with the floppy installer, then seven disks of packages in a Slicer archive. It installs FreeDOS 1.4 with a set of programs suited to classic PCs, the 80286 included, on any PC that runs FreeDOS with EGA or better graphics. Released by the FreeDOS Project on 2 April 2025. GPL and other open source licences, listed per program in LICENSES.TXT; the set carries the official FreeDOS source packages of its programs.",
        format: "1.2M",
        disks: &[
            file(
                "freedos/floppy-1.4/120m/x86BOOT.img",
                "03df6088be016e57a6c44275f5bb9ab0244db71de1360957fd76ba83243b6a77",
                "Disk 1 of 8 - boot disk and installer (x86BOOT.img)",
            ),
            file(
                "freedos/floppy-1.4/120m/x86DSK01.img",
                "83505b32f4165f393e76d5de3e59610bf49ffad6c349fd68a2f4795e0d5153af",
                "Disk 2 of 8 - packages (x86DSK01.img)",
            ),
            file(
                "freedos/floppy-1.4/120m/x86DSK02.img",
                "05303837dbd336c3fdc20fe55ea3e2778de467ecf2a68dbce905c721b0ddb94c",
                "Disk 3 of 8 - packages (x86DSK02.img)",
            ),
            file(
                "freedos/floppy-1.4/120m/x86DSK03.img",
                "f82ebe5153a2fb5a37a1569c8927447c086b2d06c77c979bea4ad65b14b4b111",
                "Disk 4 of 8 - packages (x86DSK03.img)",
            ),
            file(
                "freedos/floppy-1.4/120m/x86DSK04.img",
                "ddbbeb52f90c86ef4952a6c4e8b923577071712427a9cbb1578964fa8c9c7487",
                "Disk 5 of 8 - packages (x86DSK04.img)",
            ),
            file(
                "freedos/floppy-1.4/120m/x86DSK05.img",
                "e1687f7672e707cc7e70ac4b11a4a0bb6a4e37a05f1d93914d6867d481338768",
                "Disk 6 of 8 - packages (x86DSK05.img)",
            ),
            file(
                "freedos/floppy-1.4/120m/x86DSK06.img",
                "57b2e375740c061438f8e1446afd202a718278f3c20713c444454cf379afc913",
                "Disk 7 of 8 - packages (x86DSK06.img)",
            ),
            file(
                "freedos/floppy-1.4/120m/x86DSK07.img",
                "b5b7cc6dc0f85ad9c7a961311c3ccf2d811ae0dd37104f44bee8c31a2bc06fe3",
                "Disk 8 of 8 - packages (x86DSK07.img)",
            ),
        ],
        release: &FREEDOS_1_4,
    },
    MediaSource {
        tag: "1.4-720k-r1",
        description: "The official FreeDOS 1.4 Floppy Edition on twelve 3.5-inch 720K diskettes, unmodified: the boot disk with the floppy installer, then eleven disks of packages in a Slicer archive. It installs FreeDOS 1.4 with a set of programs suited to classic PCs, the 80286 included, on any PC that runs FreeDOS with EGA or better graphics. Released by the FreeDOS Project on 2 April 2025. GPL and other open source licences, listed per program in LICENSES.TXT; the set carries the official FreeDOS source packages of its programs.",
        format: "720K",
        disks: &[
            file(
                "freedos/floppy-1.4/720k/x86BOOT.img",
                "cbca90627b944319f02043bc41f1715ff5366b23e5e8e75aa3ca23bb564e0789",
                "Disk 1 of 12 - boot disk and installer (x86BOOT.img)",
            ),
            file(
                "freedos/floppy-1.4/720k/x86DSK01.img",
                "02650e6d952aec1b7465574cc8ddb0f29058444029050f06ab29094589b2b889",
                "Disk 2 of 12 - packages (x86DSK01.img)",
            ),
            file(
                "freedos/floppy-1.4/720k/x86DSK02.img",
                "ad2c055b22312f217a76bf0af25be6bd09b5e208b416d4d51cd7cc8bf9d0cd57",
                "Disk 3 of 12 - packages (x86DSK02.img)",
            ),
            file(
                "freedos/floppy-1.4/720k/x86DSK03.img",
                "49d79c2a4d36bb026008f31ebf8e7597330a04a0f396111862e6d36ba2398613",
                "Disk 4 of 12 - packages (x86DSK03.img)",
            ),
            file(
                "freedos/floppy-1.4/720k/x86DSK04.img",
                "17f756003d3436b5371ab3108c37a73e78eb028ed89a75b067abde3f5ceb7e62",
                "Disk 5 of 12 - packages (x86DSK04.img)",
            ),
            file(
                "freedos/floppy-1.4/720k/x86DSK05.img",
                "c3aeb780e1a2cabc5c50e30d2078741a3026da9085ef052d4edaa42ee2b9685c",
                "Disk 6 of 12 - packages (x86DSK05.img)",
            ),
            file(
                "freedos/floppy-1.4/720k/x86DSK06.img",
                "d54ce27af491a15128d61f6ee9581cec92c4e823e59e79bb8875147494a0123f",
                "Disk 7 of 12 - packages (x86DSK06.img)",
            ),
            file(
                "freedos/floppy-1.4/720k/x86DSK07.img",
                "ea342ba1cb40ba253540d4a935b2ae2834d96d0a35e943f32caaa38684601425",
                "Disk 8 of 12 - packages (x86DSK07.img)",
            ),
            file(
                "freedos/floppy-1.4/720k/x86DSK08.img",
                "69ee59879182cdad0bf1a03b318a30dd69c11cd9c4985d875e4bf1587c922fec",
                "Disk 9 of 12 - packages (x86DSK08.img)",
            ),
            file(
                "freedos/floppy-1.4/720k/x86DSK09.img",
                "f344088ceb48f05c718d0158a902e584c1f66a7e0d8d67ab2adf9069e122051e",
                "Disk 10 of 12 - packages (x86DSK09.img)",
            ),
            file(
                "freedos/floppy-1.4/720k/x86DSK10.img",
                "de31af59860e1f59a67a2036113a0af4db8f72bebb4812fe5aa291c5faf3cce0",
                "Disk 11 of 12 - packages (x86DSK10.img)",
            ),
            file(
                "freedos/floppy-1.4/720k/x86DSK11.img",
                "70e82a287d4d89b40c975d6a62700633cdd6dac0d55da53953edfc37c69a6c0b",
                "Disk 12 of 12 - packages (x86DSK11.img)",
            ),
        ],
        release: &FREEDOS_1_4,
    },
];

/// What happened, or what a build would do, to one local tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The tag is new.
    Created,
    /// The tag already names this image.
    Present,
    /// The alias moves away from this digest, which only lower versions of
    /// the repository name.
    Moved(Digest),
    /// The alias names this other image, a higher version or one no version
    /// of the repository names, and stays.
    Kept(Digest),
    /// The version tag names this other image, so a build refuses; only
    /// [`plan_tags`] reports it.
    Conflict(Digest),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Firmware,
    Machine,
    /// An open installation set under `unemu/media/<platform>/`.
    Media,
}

/// One built image and its local tags.
#[derive(Clone, Debug)]
pub struct Built {
    pub kind: Kind,
    /// The registry repository, such as `unemu/open-at`.
    pub repository: String,
    /// The tag that never moves: [`VERSION`] for a template,
    /// [`FIRMWARE_REVISION`] for a package, a media set's revision tag.
    pub version: &'static str,
    pub descriptor: Descriptor,
    /// That tag, then its alias: `latest` for a template, the release for a
    /// set; a package without a release has none. Local tags are the short
    /// form a pull of the repository from the family default registry
    /// writes, so `image push TAG` reaches the repository.
    pub tags: Vec<(String, Outcome)>,
}

/// The local tag a pull of `repository:tag` from the family default registry
/// writes: no host, and without the official namespace when one name remains.
pub fn local_tag(repository: &str, tag: &str) -> String {
    let name = match repository.strip_prefix(&format!("{NAMESPACE}/")) {
        Some(rest) if !rest.contains('/') => rest,
        _ => repository,
    };
    format!("{name}:{tag}")
}

/// A version tag's precedence, Semantic Versioning 2.0.0 §11: the numeric
/// core, then a release above its pre-releases, whose dot-separated
/// identifiers compare numerically when numeric and as text otherwise,
/// numeric ones first and a shorter list first. Build metadata is ignored.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Precedence {
    core: [u64; 3],
    release: bool,
    pre: Vec<Identifier>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Identifier {
    Numeric(u64),
    Text(String),
}

impl Precedence {
    /// A release, not a pre-release.
    pub fn is_release(&self) -> bool {
        self.release
    }
}

/// The precedence of a version-shaped tag; `None` for any other tag.
pub fn precedence(tag: &str) -> Option<Precedence> {
    if !Store::is_version_tag(tag) {
        return None;
    }
    let tag = tag.split('+').next()?;
    let (core, pre) = match tag.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (tag, None),
    };
    let mut numbers = core.split('.').map(|n| n.parse::<u64>().ok());
    let core = [numbers.next()??, numbers.next()??, numbers.next()??];
    let pre = pre
        .map(|pre| {
            pre.split('.')
                .map(|id| match id.parse::<u64>() {
                    Ok(n) if id.bytes().all(|b| b.is_ascii_digit()) => Identifier::Numeric(n),
                    _ => Identifier::Text(id.to_owned()),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Precedence {
        core,
        release: tag.split_once('-').is_none(),
        pre,
    })
}

/// Asked with the image's reference, manifest and config bytes before the
/// image is tagged: `repository:VERSION` for a template, `repository:r1`
/// ([`FIRMWARE_REVISION`]) for a package, a set's revision tag for a set. An
/// error refuses the build.
pub type Guard<'a> = &'a dyn Fn(&str, &Manifest, &[u8]) -> std::result::Result<(), String>;

/// Build every template machine into `store`, in a fixed order, handing each
/// to `built` once its tags are written. The firmware packages it binds come
/// first from the open fixtures under `vendor` through [`build_firmware`],
/// as local copies of the published packages: neither `guard` nor `built`
/// sees them, and the guard judges their ROMs and source archives inside
/// each template. A tag that already names the same image is kept; a version
/// tag naming another image refuses the build. The alias moves by
/// compare-and-swap, and only away from an image that lower versions of the
/// repository name alone: never from a higher version, and never from an
/// image the build did not version.
pub fn build(
    store: &Store,
    vendor: &Path,
    guard: Guard<'_>,
    built: &mut dyn FnMut(&Built),
) -> Result<Vec<Built>> {
    let packages: Vec<Digest> = build_firmware(store, vendor, &|_, _, _| Ok(()), &mut |_| {})?
        .into_iter()
        .map(|package| package.descriptor.digest)
        .collect();
    let mut images = Vec::new();
    let stored = create::packages(store)?;
    let rom = |digest: &Digest, id: &str| -> Result<PackageRom> {
        let package = stored
            .iter()
            .find(|p| &p.descriptor.digest == digest)
            .ok_or_else(|| invalid(format!("the {id} package is not readable")))?;
        let rom = package
            .config
            .roms
            .iter()
            .find(|r| r.catalogue_id.as_deref() == Some(id))
            .ok_or_else(|| invalid(format!("the {id} package lacks its ROM")))?;
        Ok(PackageRom::new(package, rom))
    };
    for template in &TEMPLATES {
        let image = NewImage {
            hardware: template.hardware()?,
            firmware: FIRMWARE
                .iter()
                .zip(&packages)
                .map(|(source, digest)| rom(digest, source.catalogue_id))
                .collect::<Result<_>>()?,
            title: Some(template.title.into()),
            description: Some(template.description.into()),
        };
        let machine = build_machine(store, template, &image, guard)?;
        built(&machine);
        images.push(machine);
    }
    Ok(images)
}

/// Build every firmware package of [`FIRMWARE`] from the fixtures under
/// `vendor` into `store`, each under
/// `unemu/firmware/<platform>/<catalogue id>:r1` ([`FIRMWARE_REVISION`]) and
/// offered to `guard` before it is tagged, as [`build`] builds the
/// templates. A package's revision tag never moves, so another package under
/// it refuses the build.
pub fn build_firmware(
    store: &Store,
    vendor: &Path,
    guard: Guard<'_>,
    built: &mut dyn FnMut(&Built),
) -> Result<Vec<Built>> {
    let catalogue = crate::firmware::catalogue();
    let mut packages = Vec::new();
    for source in &FIRMWARE {
        let package = build_package(store, vendor, source, catalogue, guard)?;
        built(&package);
        packages.push(package);
    }
    Ok(packages)
}

/// Build every installation set of [`MEDIA`] from the fixtures under
/// `vendor` into `store`, each under its revision tag
/// `unemu/media/<platform>/<work>:<release>-r1`, whose release alias the
/// store moves with it, and offered to `guard` before it is tagged, as
/// [`build`] builds the templates. Every
/// fixture of every set is checked against its pin first
/// ([`check_media_fixtures`]), so a missing or changed file stops the build
/// before any set is tagged.
pub fn build_media(
    store: &Store,
    vendor: &Path,
    guard: Guard<'_>,
    built: &mut dyn FnMut(&Built),
) -> Result<Vec<Built>> {
    check_media_fixtures(vendor)?;
    let mut sets = Vec::new();
    for source in &MEDIA {
        let set = build_set(store, vendor, source, guard)?;
        built(&set);
        sets.push(set);
    }
    Ok(sets)
}

/// Every fixture the installation sets read, as [`MediaFile`]s: each set's
/// disks, then the extras their releases share.
pub fn media_files() -> Vec<&'static MediaFile> {
    let mut files: Vec<&MediaFile> = MEDIA.iter().flat_map(|set| set.disks).collect();
    files.extend(FREEDOS_1_4.extras.iter().map(|(file, _)| file));
    files
}

/// Check that every fixture of [`MEDIA`] exists and holds the bytes its
/// fetch script pins; the error names the first that does not and the
/// script to run.
pub fn check_media_fixtures(vendor: &Path) -> Result<()> {
    for file in media_files() {
        media_fixture(vendor, file, FREEDOS_1_4.fetch)?;
    }
    Ok(())
}

/// The path of one fixture, refused unless it holds the pinned bytes: the
/// licence statement covers these bytes only.
fn media_fixture(vendor: &Path, file: &MediaFile, fetch: &str) -> Result<PathBuf> {
    let path = vendor.join(file.path);
    if !path.is_file() {
        return Err(invalid(format!(
            "missing fixture {}; run {fetch}",
            path.display()
        )));
    }
    if Digest::of(&std::fs::read(&path)?).hex() != file.sha256 {
        return Err(invalid(format!(
            "{} is not the file {fetch} pins; run it again",
            path.display()
        )));
    }
    Ok(path)
}

fn build_package(
    store: &Store,
    vendor: &Path,
    source: &FirmwareSource,
    catalogue: &[assembly::FirmwareEntry],
    guard: Guard<'_>,
) -> Result<Built> {
    let entry = catalogue
        .iter()
        .find(|e| e.id == source.catalogue_id)
        .ok_or_else(|| invalid(format!("{} is not in the catalogue", source.catalogue_id)))?;
    let assembly::FirmwareDistribution::Open {
        fetch,
        source_archive_sha256: Some(pin),
        ..
    } = entry.distribution
    else {
        return Err(invalid(format!(
            "{} is not open firmware with a pinned source archive",
            entry.id
        )));
    };
    let fixture = |relative: &str| -> Result<PathBuf> {
        let path = vendor.join(relative);
        if path.is_file() {
            Ok(path)
        } else {
            Err(invalid(format!(
                "missing fixture {}; run {fetch}",
                path.display()
            )))
        }
    };
    let rom = fixture(source.rom)?;
    let archive = fixture(source.source_archive)?;
    // Another archive would keep the copyleft ROM user-supplied: only the one
    // the fetch script builds is open without a licence statement.
    if Digest::of(&std::fs::read(&archive)?).hex() != pin {
        return Err(invalid(format!(
            "{} is not the source archive the catalogue pins for {}; run {fetch}",
            archive.display(),
            entry.id
        )));
    }
    let scan = firmware_import::scan(std::slice::from_ref(&rom), catalogue)?;
    let candidate = scan
        .candidates
        .into_iter()
        .find(|c| c.catalogue_id == entry.id)
        .ok_or_else(|| {
            invalid(format!(
                "{} is not the {} build; run {fetch}",
                rom.display(),
                entry.id
            ))
        })?;
    let statement = source.statement(archive);
    let repository = source.repository();
    let preview = firmware_import::preview_manifest_with(&candidate, &[], &statement, catalogue)?;
    check(
        guard,
        &repository,
        FIRMWARE_REVISION,
        &preview.manifest,
        &preview.config.encode()?,
    )?;
    let judged = Digest::of(&preview.manifest.encode()?);
    let revision = local_tag(&repository, FIRMWARE_REVISION);
    let before = version_state(store, &revision, &judged, PACKAGE_CONFLICT)?;
    let descriptor = firmware_import::publish_as(
        store,
        &candidate,
        &[],
        &statement,
        catalogue,
        &revision,
        Some(&judged),
    )?;
    Ok(Built {
        kind: Kind::Firmware,
        repository,
        version: FIRMWARE_REVISION,
        tags: vec![(revision, before)],
        descriptor,
    })
}

fn build_machine(
    store: &Store,
    template: &Template,
    image: &NewImage,
    guard: Guard<'_>,
) -> Result<Built> {
    let repository = template.repository();
    let (config, manifest) = create::preview_with_config(image, store)?;
    check(guard, &repository, VERSION, &manifest, &config)?;
    let judged = Digest::of(&manifest.encode_with_types(PcProvider.payload_types())?);
    let version = local_tag(&repository, VERSION);
    let before = version_state(store, &version, &judged, TEMPLATE_CONFLICT)?;
    let descriptor = match before {
        Outcome::Present => store.tag_descriptor(&version)?,
        _ => create::create_judged(store, image, &version, None, &judged)?,
    };
    let alias = alias(store, &repository, VERSION, &descriptor, &PcProvider)?;
    Ok(Built {
        kind: Kind::Machine,
        repository,
        version: VERSION,
        tags: vec![(version, before), alias],
        descriptor,
    })
}

/// What an installation set is imported from under `tag`, as `image media
/// import` states it with its flags: the disks in installer order with their
/// labels and format, the extras with their DOS names and kinds, and the
/// catalogue fields. Every fixture is checked against its pin first.
fn media_request(vendor: &Path, source: &MediaSource, tag: &str) -> Result<Request> {
    let release = source.release;
    // Another file at a fixture path is refused, never imported under the
    // licence statement.
    let fixture = |file: &MediaFile| media_fixture(vendor, file, release.fetch);
    let media = source
        .disks
        .iter()
        .map(|disk| {
            Ok(Source {
                path: fixture(disk)?,
                label: Some(disk.name.into()),
                format: Some(source.format.into()),
                file_name: None,
            })
        })
        .collect::<Result<_>>()?;
    let extras = release
        .extras
        .iter()
        .map(|(file, kind)| {
            Ok(ExtraSource {
                path: fixture(file)?,
                dos_name: Some(file.name.into()),
                kind: Some(*kind),
            })
        })
        .collect::<Result<_>>()?;
    let title = Title {
        title: release.title.into(),
        description: Some(source.description.into()),
        publishers: vec![release.publisher.into()],
        developers: vec![release.publisher.into()],
        release_year: Some(release.year),
        genres: release.genres.iter().map(|g| (*g).into()).collect(),
        platform: Some("DOS".into()),
        version: Some(release.release.into()),
        edition: Some(release.edition.into()),
        ..Title::default()
    };
    Ok(Request {
        tag: tag.into(),
        title,
        labels: release.labels.iter().map(|l| (*l).into()).collect(),
        media,
        extras,
        compress: true,
    })
}

/// The statement that opens a release's sets: its licence, the licence index
/// as the licence's text and the source archive.
fn media_statement(release: &MediaRelease) -> OpenStatement {
    OpenStatement {
        license: release.license.into(),
        terms: release.terms.into(),
        source: Some(release.source.into()),
    }
}

/// The plan of one set under `tag`, open under its release's statement.
fn media_plan(vendor: &Path, source: &MediaSource, tag: &str) -> Result<media_import::Plan> {
    let request = media_request(vendor, source, tag)?;
    media_import::plan_with(&request, Some(&media_statement(source.release)))
        .map_err(|refusal| invalid(refusal.to_string()))
}

/// Import one installation set under its own tag. A new set is offered to
/// the guard before its manifest is stored; a tag that already names it is
/// compared without writing and offered as stored. The stored set must name
/// the pinned bytes, so a fixture replaced between the check and the import
/// is caught.
fn build_set(
    store: &Store,
    vendor: &Path,
    source: &MediaSource,
    guard: Guard<'_>,
) -> Result<Built> {
    let repository = source.repository();
    let release = source.release;
    let version = local_tag(&repository, source.tag);
    let alias = local_tag(&repository, source.alias());
    let alias_before = current(store, &alias)?;
    let plan = media_plan(vendor, source, &version)?;
    let validator = MediaValidator(&PcMediaFormats);
    let before = match current(store, &version)? {
        Some(_) => {
            media_import::publish(store, &plan, &|_, _| {}, &|| false)
                .map_err(|refusal| invalid(refusal.to_string()))?;
            let stored = store.resolve(&version, &validator)?;
            check(
                guard,
                &repository,
                source.tag,
                &stored.manifest,
                &stored.config,
            )?;
            Outcome::Present
        }
        None => {
            let judge = |manifest: &Manifest, config: &[u8]| {
                check(guard, &repository, source.tag, manifest, config).map_err(|e| e.to_string())
            };
            media_import::publish_checked(store, &plan, &|_, _| {}, &|| false, &judge)
                .map_err(|refusal| invalid(refusal.to_string()))?;
            Outcome::Created
        }
    };
    let descriptor = store.tag_descriptor(&version)?;
    let stored = store.resolve_descriptor(&descriptor, &validator)?;
    let config = image_store::media_set::Config::decode(&stored.config)?;
    let names: Vec<&str> = config
        .media
        .iter()
        .map(|m| m.blob.hex())
        .chain(config.extras.iter().map(|e| e.blob.hex()))
        .collect();
    let pins: Vec<&str> = source
        .disks
        .iter()
        .chain(release.extras.iter().map(|(file, _)| file))
        .map(|file| file.sha256)
        .collect();
    if names != pins {
        return Err(invalid(format!(
            "{version} does not hold the pinned fixtures; run {}",
            release.fetch
        )));
    }
    // The store moved the alias in the write of the revision, when that
    // went forward.
    let mut tags = vec![(version, before)];
    match (alias_before, current(store, &alias)?) {
        (_, None) => {}
        (None, Some(_)) => tags.push((alias, Outcome::Created)),
        (Some(old), Some(_)) if old == descriptor.digest => tags.push((alias, Outcome::Present)),
        (Some(old), Some(now)) if now == descriptor.digest => {
            tags.push((alias, Outcome::Moved(old)))
        }
        (Some(_), Some(now)) => tags.push((alias, Outcome::Kept(now))),
    }
    Ok(Built {
        kind: Kind::Media,
        repository,
        version: source.tag,
        tags,
        descriptor,
    })
}

fn check(
    guard: Guard<'_>,
    repository: &str,
    version: &str,
    manifest: &Manifest,
    config: &[u8],
) -> Result<()> {
    guard(&format!("{repository}:{version}"), manifest, config)
        .map_err(|reason| invalid(format!("{repository}:{version} refused: {reason}")))
}

/// What to do when a template's version tag names another image.
const TEMPLATE_CONFLICT: &str =
    "a version tag never moves, so give the changed template a new version";
/// What to do when a package's tag names another image: the build makes the
/// published package, so the local tag is the one in the way.
const PACKAGE_CONFLICT: &str =
    "a package's tag never moves; remove or rename that local tag, or build into a dedicated store";

/// Created when absent, Present when it names `digest`; another image under
/// the tag refuses the build, with `remedy`.
fn version_state(store: &Store, tag: &str, digest: &Digest, remedy: &str) -> Result<Outcome> {
    match planned_version(store, tag, digest)? {
        Outcome::Conflict(current) => Err(invalid(format!(
            "{tag} already names {current}, not {digest}; {remedy}"
        ))),
        outcome => Ok(outcome),
    }
}

/// Refuse, as [`build`] would, when a firmware package tag of `store` names
/// another image than the package [`build`] made in `staging`. `store` is
/// only read: a dry run's check of the tags it does not report.
pub fn check_package_tags(store: &Store, staging: &Store) -> Result<()> {
    for source in &FIRMWARE {
        let tag = local_tag(&source.repository(), FIRMWARE_REVISION);
        let digest = staging.tag_descriptor(&tag)?.digest;
        version_state(store, &tag, &digest, PACKAGE_CONFLICT)?;
    }
    Ok(())
}

fn current(store: &Store, tag: &str) -> Result<Option<Digest>> {
    match store.tag_descriptor(tag) {
        Ok(current) => Ok(Some(current.digest)),
        Err(image_store::Error::TagNotFound(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn planned_version(store: &Store, tag: &str, digest: &Digest) -> Result<Outcome> {
    Ok(match current(store, tag)? {
        None => Outcome::Created,
        Some(current) if &current == digest => Outcome::Present,
        Some(current) => Outcome::Conflict(current),
    })
}

/// What the alias of `repository` does for an image with `digest` under the
/// version tag `version`.
fn planned_alias(
    store: &Store,
    repository: &str,
    version: &str,
    digest: &Digest,
) -> Result<(String, Outcome)> {
    let tag = local_tag(repository, ALIAS);
    let Some(current) = current(store, &tag)? else {
        return Ok((tag, Outcome::Created));
    };
    if &current == digest {
        return Ok((tag, Outcome::Present));
    }
    let ours = precedence(version).ok_or_else(|| invalid(format!("{version} is not a version")))?;
    let prefix = local_tag(repository, "");
    let (mut lower, mut other) = (false, false);
    let mut after: Option<String> = None;
    loop {
        let page = store.list_tags(after.as_deref(), 1000)?;
        for named in page.tags.iter().filter(|t| t.manifest.digest == current) {
            match named.name.strip_prefix(&prefix).and_then(precedence) {
                Some(version) if version < ours => lower = true,
                Some(_) => other = true,
                None => {}
            }
        }
        match page.next_after {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    Ok(if lower && !other {
        (tag, Outcome::Moved(current))
    } else {
        (tag, Outcome::Kept(current))
    })
}

/// Point the alias at `descriptor` when [`planned_alias`] allows it, moving
/// it only from the digest read there.
fn alias(
    store: &Store,
    repository: &str,
    version: &str,
    descriptor: &Descriptor,
    validator: &dyn ConfigValidator,
) -> Result<(String, Outcome)> {
    let (tag, outcome) = planned_alias(store, repository, version, &descriptor.digest)?;
    match &outcome {
        Outcome::Created => store.update_tag(&tag, descriptor, None, validator)?,
        Outcome::Moved(old) => store.update_tag(&tag, descriptor, Some(old), validator)?,
        _ => {}
    }
    Ok((tag, outcome))
}

/// What a build of `built` would do to the tags of `store`, which is only
/// read: a dry run's report against the store it would write.
pub fn plan_tags(store: &Store, built: &Built) -> Result<Vec<(String, Outcome)>> {
    let digest = &built.descriptor.digest;
    let version = local_tag(&built.repository, built.version);
    let state = planned_version(store, &version, digest)?;
    let alias = match built.kind {
        Kind::Machine => Some(planned_alias(
            store,
            &built.repository,
            built.version,
            digest,
        )?),
        // The store moves a set's release alias with its revision, forward
        // only; a package without a release has no alias.
        Kind::Media | Kind::Firmware => {
            store
                .alias_plan(&version, digest)?
                .and_then(|(alias, current, follows)| {
                    let outcome = match (current, follows) {
                        (None, true) => Outcome::Created,
                        // A newer revision of the release is here, and the
                        // absent alias is not created for an older one.
                        (None, false) => return None,
                        (Some(current), _) if &current == digest => Outcome::Present,
                        (Some(current), true) => Outcome::Moved(current),
                        (Some(current), false) => Outcome::Kept(current),
                    };
                    Some((alias, outcome))
                })
        }
    };
    let mut tags = vec![(version, state)];
    tags.extend(alias);
    Ok(tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_registry_grammar_and_the_pull_short_form() {
        assert_eq!(local_tag("unemu/open-at", VERSION), "open-at:1.2.0");
        assert_eq!(
            local_tag(
                "unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0",
                FIRMWARE_REVISION
            ),
            "unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1"
        );
        for template in &TEMPLATES {
            image_store::Store::validate_tag_name(&local_tag(&template.repository(), VERSION))
                .unwrap();
        }
        assert!(image_store::Store::is_version_tag(VERSION));
        assert!(!image_store::Store::is_version_tag(ALIAS));
    }

    /// The local builds take the names the unemu-media repository publishes
    /// under, which are also the local tags `image pull` writes for them.
    #[test]
    fn packages_and_sets_take_their_published_names() {
        let packages: Vec<_> = FIRMWARE
            .iter()
            .map(|source| local_tag(&source.repository(), FIRMWARE_REVISION))
            .collect();
        assert_eq!(
            packages,
            [
                "unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1",
                "unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std:r1",
                "unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1",
            ]
        );
        let sets: Vec<_> = MEDIA
            .iter()
            .flat_map(|source| {
                [
                    local_tag(&source.repository(), source.tag),
                    local_tag(&source.repository(), source.alias()),
                ]
            })
            .collect();
        assert_eq!(
            sets,
            [
                "unemu/media/ibm-pc/freedos:1.4-r1",
                "unemu/media/ibm-pc/freedos:1.4",
                "unemu/media/ibm-pc/freedos:1.4-1.2m-r1",
                "unemu/media/ibm-pc/freedos:1.4-1.2m",
                "unemu/media/ibm-pc/freedos:1.4-720k-r1",
                "unemu/media/ibm-pc/freedos:1.4-720k",
            ]
        );
        for tag in packages.iter().chain(&sets) {
            image_store::Store::validate_tag_name(tag).unwrap();
        }
        // Revision tags, which never move; a package without a release has
        // no alias, a set's release is its alias. Neither is a version.
        assert_eq!(image_store::tag::alias_of(FIRMWARE_REVISION), None);
        assert!(image_store::tag::is_revision(FIRMWARE_REVISION));
        for source in &MEDIA {
            assert!(image_store::tag::is_revision(source.tag));
            assert!(!image_store::Store::is_version_tag(source.tag));
            assert!(!image_store::Store::is_version_tag(source.alias()));
        }
    }

    /// The statement in the code is the conjunction the licence index, the
    /// sets' licence text, closes with.
    #[test]
    fn the_licence_expression_is_the_one_the_licence_index_states() {
        let index = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../vendor/freedos/floppy-1.4-licenses.txt"),
        )
        .unwrap();
        let (_, tail) = index
            .split_once("The conjunction of every licence above is:")
            .expect("the index states its conjunction");
        let stated = tail
            .split("\n\n")
            .next()
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(stated, FREEDOS_1_4.license);
        assert!(index.starts_with("SPDX-License-Identifier: CC0-1.0\n"));
    }

    #[test]
    fn the_media_sets_state_a_copyleft_licence_with_its_text_and_source() {
        use image_store::license::{Expression, ACTIVE};
        let release = &FREEDOS_1_4;
        let license = Expression::parse(release.license).unwrap();
        assert_eq!(license.as_str(), release.license);
        assert!(license.needs_source(ACTIVE));
        let named = |name: &str| {
            release
                .extras
                .iter()
                .filter(|(file, _)| file.path.rsplit('/').next() == Some(name))
                .count()
        };
        assert_eq!((named(release.terms), named(release.source)), (1, 1));
        for (file, _) in release.extras {
            assert!(crate::media_set::valid_dos_name(file.name), "{}", file.name);
        }
        assert!(image_store::media_set::valid_text(release.title));
        for (source, disks) in MEDIA.iter().zip([7, 8, 12]) {
            assert_eq!(source.disks.len(), disks, "{}", source.tag);
            assert!(crate::media_import::format_names().contains(&source.format));
            assert!(source.disks[0].path.ends_with("/x86BOOT.img"));
            for (index, disk) in source.disks.iter().enumerate() {
                assert!(
                    image_store::media_set::valid_text(disk.name),
                    "{}",
                    disk.name
                );
                assert!(disk
                    .name
                    .starts_with(&format!("Disk {} of {disks} - ", index + 1)));
            }
        }
    }

    #[test]
    fn versions_order_by_semantic_version_precedence() {
        let order = [
            "0.9.0",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.10.0",
            "2.0.0",
        ];
        for pair in order.windows(2) {
            assert!(
                precedence(pair[0]).unwrap() < precedence(pair[1]).unwrap(),
                "{pair:?}"
            );
        }
        assert_eq!(precedence("1.0.0+b1"), precedence("1.0.0"));
        assert_eq!(precedence(ALIAS), None);
    }

    #[test]
    fn the_games_template_adds_the_open_game_profile_cards_only() {
        let plain = TEMPLATES[0].hardware().unwrap();
        let games = TEMPLATES[1].hardware().unwrap();
        assert_eq!(plain.sound, None);
        assert_eq!(plain.ems, None);
        assert_eq!(plain.serial, Serial::Empty);
        assert_eq!(games.serial, Serial::Mouse);
        assert_eq!(
            Hardware {
                sound: None,
                serial: Serial::Empty,
                ems: None,
                ..games.clone()
            },
            plain
        );
        assert_eq!(plain.rate, (8, 13));
        assert_eq!(plain.disks, [Some(Chs::new(1024, 16, 63)), None]);
        assert_eq!(
            plain.floppies,
            [Some("1.2M".to_owned()), Some("1.44M".to_owned())]
        );
    }
}
