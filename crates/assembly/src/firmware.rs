// SPDX-License-Identifier: AGPL-3.0-or-later
//! Firmware identities and board-owned system BIOS load requirements.
//!
//! IBM Personal Computer AT Technical Reference, Section 1 "System Board",
//! Memory Map: system ROM occupies the top of the first MiB. The catalogue
//! additionally restricts images to reproducible, reviewed builds. See
//! `backlog/docs/design/firmware-and-machine-classes.md` §3 and §6.

use sha2::{Digest, Sha256};

use crate::{
    BoardModel, ConfigError, CpuChip, DeviceKind, FirmwareService, ImageKind, LoadImage,
    MachineConfig,
};

/// Run-only permissions, deliberately outside machine identity and files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssemblyOptions {
    /// Accept unknown hashes, recording each one for a frontend warning.
    pub allow_unlisted_firmware: bool,
}

/// The purpose of a catalogue image; option-ROM loading is not implemented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareKind {
    /// Reset and motherboard initialization firmware.
    SystemBios,
    /// Expansion firmware.
    OptionRom,
    /// Video initialization firmware.
    VideoBios,
}

/// Physical load rule belonging to the image, rather than the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareBase {
    /// End at physical 0x100000.
    TopOfFirstMib,
    /// Suggested expansion-ROM address.
    Suggested(u64),
}

/// Minimum processor generation; this does not enable another CPU backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FirmwareCpu {
    /// The currently configured Intel 80286 core.
    I80286,
    /// A catalogue requirement that the current core cannot satisfy.
    I80386,
}

impl std::fmt::Display for FirmwareCpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::I80286 => "80286",
            Self::I80386 => "80386",
        })
    }
}

/// Board parts whose presence can be checked without executing firmware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwarePart {
    /// AT keyboard controller.
    Kbc8042,
    /// AT RTC and CMOS RAM.
    RtcCmos,
    /// Master and slave interrupt controllers.
    PicCascade,
    /// A video ROM at the conventional expansion address.
    VideoRom,
    /// Primary ATA channel using the AT fixed-disk wiring: 1F0h and IRQ 14.
    /// IBM Personal Computer AT Technical Reference, Section 1 "System Board",
    /// I/O Address Map and System Interrupts; see the firmware design §2.2.
    AtaPrimary,
}

impl std::fmt::Display for FirmwarePart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Kbc8042 => "kbc-8042",
            Self::RtcCmos => "rtc-cmos",
            Self::PicCascade => "pic-cascade",
            Self::VideoRom => "a video-bios option ROM at 0xC0000",
            Self::AtaPrimary => "ata@1F0/14",
        })
    }
}

impl FirmwarePart {
    fn present(self, config: &MachineConfig) -> bool {
        let has =
            |predicate: fn(DeviceKind) -> bool| config.devices.iter().any(|d| predicate(d.kind));
        match self {
            Self::Kbc8042 => has(|kind| matches!(kind, DeviceKind::Keyboard(_))),
            Self::RtcCmos => has(|kind| matches!(kind, DeviceKind::Rtc(_))),
            Self::PicCascade => {
                has(|kind| matches!(kind, DeviceKind::Pic(p) if p.cascade))
                    && has(|kind| matches!(kind, DeviceKind::Pic(p) if !p.cascade))
            }
            Self::VideoRom => config
                .images
                .iter()
                .any(|image| image.kind == ImageKind::Rom && image.base == 0xC0000),
            // No current device kind models ATA; a matching port window alone
            // cannot establish that the required controller exists.
            Self::AtaPrimary => false,
        }
    }
}

/// How a known image can be obtained, with licence and source provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareDistribution {
    /// Rebuilt by a repository fixture script; no ROM bytes are distributed.
    Open {
        /// Repository-relative fetch script.
        fetch: &'static str,
        /// SPDX licence of the upstream firmware.
        license: &'static str,
        /// Upstream project and pinned revision.
        provenance: &'static str,
    },
    /// Identified by hash, supplied separately by the user.
    UserSupplied,
}

impl std::fmt::Display for FirmwareDistribution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open { fetch, .. } => write!(f, "open, {fetch}"),
            Self::UserSupplied => f.write_str("user-supplied"),
        }
    }
}

/// Evidence supporting a catalogue entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareStatus {
    /// Executed by a pinned golden.
    Tested,
    /// Identified but not yet executed by a golden.
    Listed,
}

/// One reviewed firmware build. Service declarations remain independent of this
/// metadata; see `backlog/docs/design/machine-config.md` §Firmware catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareEntry {
    /// Stable versioned identifier.
    pub id: &'static str,
    /// Firmware purpose.
    pub kind: FirmwareKind,
    /// Full lowercase SHA-256 of the raw loadable bytes.
    pub sha256: &'static str,
    /// Exact raw image length in bytes.
    pub size: u64,
    /// Informational for system BIOS entries: the shared top-of-first-MiB
    /// constraint is checked before lookup, even for unlisted images. Suggested
    /// option-ROM addresses await the option-ROM loader.
    pub base: FirmwareBase,
    /// Services documented for this build.
    pub provides: &'static [FirmwareService],
    /// Required modelled board parts.
    pub requires: &'static [FirmwarePart],
    /// Minimum CPU generation.
    pub cpu_min: FirmwareCpu,
    /// Compatible board identifiers, including boards reserved for future work.
    pub boards: &'static [&'static str],
    /// Fetch, licence and provenance information.
    pub distribution: FirmwareDistribution,
    /// Execution evidence.
    pub status: FirmwareStatus,
}

impl FirmwareEntry {
    /// Padded and checksummed XTIDE Universal BIOS AT build.
    ///
    /// XTIDE Universal BIOS, Main.asm, USE_AT build: primary ATA task file at
    /// 1F0h and IRQ 14. See `backlog/docs/design/firmware-and-machine-classes.md`
    /// §2.2 and `vendor/xtide-universal-bios/README.md` for the binary provenance.
    /// This is listed data; option-ROM validation and execution are separate.
    pub const XUB_R638_AT: Self = Self {
        id: "xub-r638-at",
        kind: FirmwareKind::OptionRom,
        sha256: "6409de912eab620ea144b79e723bed478814047dc2766fc19fd6e2e3690662d0",
        size: 8192,
        base: FirmwareBase::Suggested(0xC8000),
        provides: &[FirmwareService::FixedDisk],
        requires: &[FirmwarePart::AtaPrimary],
        cpu_min: FirmwareCpu::I80286,
        boards: &["ibm-5170"],
        distribution: FirmwareDistribution::Open {
            fetch: "vendor/xtide-universal-bios/fetch.sh",
            license: "GPL-2.0-only",
            provenance: "XTIDE Universal BIOS r638 ide_at.bin (AT build)",
        },
        status: FirmwareStatus::Listed,
    };
}

/// Unmodified Xi 8088 build pinned by `vendor/kiselev-bios/fetch.sh`.
/// The catalogue promises the tested 286 machine, rather than upstream's wider
/// CPU support. See `vendor/kiselev-bios/README.md`, Image size and mapping.
pub const KISELEV_XI8088: FirmwareEntry = FirmwareEntry {
    id: "kiselev-xi8088-1.0.2",
    kind: FirmwareKind::SystemBios,
    sha256: "1ad458acb1f22dee6b3d9b6bd95e83218a7ddc5389fa2b2c67a4001e689c81a8",
    size: 32768,
    base: FirmwareBase::TopOfFirstMib,
    provides: &[FirmwareService::Floppy],
    requires: &[
        FirmwarePart::Kbc8042,
        FirmwarePart::RtcCmos,
        FirmwarePart::PicCascade,
    ],
    cpu_min: FirmwareCpu::I80286,
    boards: &["ibm-5170"],
    distribution: FirmwareDistribution::Open {
        fetch: "vendor/kiselev-bios/fetch.sh",
        license: "GPL-3.0-or-later",
        provenance: "skiselev/8088_bios a4af458a72bc33c4be066b33bd76e8af2b24111c (MACHINE_XI8088)",
    },
    status: FirmwareStatus::Tested,
};

/// A hash accepted only through the explicit run option.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnlistedFirmware {
    /// Full SHA-256, so the warning identifies the bytes that actually ran.
    pub sha256: String,
}

impl std::fmt::Display for UnlistedFirmware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "warning: running unlisted firmware (sha256 {})",
            self.sha256
        )
    }
}

pub(crate) fn system_bios(config: &MachineConfig) -> Result<&LoadImage, ConfigError> {
    // Until option-ROM configuration lands, the first ROM is the system BIOS.
    config
        .images
        .iter()
        .find(|image| image.kind == ImageKind::Rom)
        .ok_or(ConfigError::MissingSystemBios {
            board: config.board.id(),
        })
}

pub(crate) fn check_bios_placement(base: u64, size: u64) -> Result<(), ConfigError> {
    if size == 0 || !size.is_multiple_of(8192) || base.checked_add(size) != Some(0x100000) {
        return Err(ConfigError::BiosPlacement { base, size });
    }
    Ok(())
}

/// Check resolved system BIOS bytes using the selected board's own catalogue.
///
/// This checks image requirements only, allowing the historical partial POST
/// rig to retain its hardware. Full assembly also validates the board's memory
/// layout and power-on entry. Bare boards have no firmware constraints.
pub fn check_board_system_bios(
    config: &MachineConfig,
    board: BoardModel,
    base: u64,
    bytes: &[u8],
    options: AssemblyOptions,
) -> Result<Option<UnlistedFirmware>, ConfigError> {
    let catalogue = board.firmware_catalogue();
    if catalogue.is_empty() {
        return Ok(None);
    }
    check_system_bios(config, board, catalogue, base, bytes, options)
}

/// Check resolved BIOS bytes against a supplied board-owned catalogue.
///
/// The separate catalogue argument allows compatibility tests without adding
/// untested firmware to production data. Bare assembly never calls this check.
/// A frontend retaining the historical partial POST rig can explicitly check
/// its BIOS against the intended board without changing that rig's hardware.
pub fn check_system_bios(
    config: &MachineConfig,
    board: BoardModel,
    catalogue: &[FirmwareEntry],
    base: u64,
    bytes: &[u8],
    options: AssemblyOptions,
) -> Result<Option<UnlistedFirmware>, ConfigError> {
    check_bios_placement(base, bytes.len() as u64)?;
    let hash = format!("{:x}", Sha256::digest(bytes));
    let Some(entry) = catalogue.iter().find(|entry| {
        entry.kind == FirmwareKind::SystemBios
            && entry.sha256 == hash
            && entry.size == bytes.len() as u64
    }) else {
        if options.allow_unlisted_firmware {
            return Ok(Some(UnlistedFirmware { sha256: hash }));
        }
        return Err(ConfigError::FirmwareNotListed {
            sha256: hash,
            board: board.id(),
            known: catalogue
                .iter()
                .map(|entry| format!("{} ({})", entry.id, entry.distribution))
                .collect::<Vec<_>>()
                .join(", "),
        });
    };
    if !entry.boards.contains(&board.id()) {
        return Err(ConfigError::FirmwareBoardMismatch {
            firmware: entry.id,
            boards: entry.boards.join(", "),
            board: board.id(),
        });
    }
    let cpu = match config.cpu.chip {
        CpuChip::I80286 => FirmwareCpu::I80286,
    };
    if cpu < entry.cpu_min {
        return Err(ConfigError::FirmwareNeedsCpu {
            firmware: entry.id,
            minimum: entry.cpu_min,
            cpu,
        });
    }
    for part in entry.requires {
        if !part.present(config) {
            return Err(ConfigError::FirmwareRequiresDevice {
                firmware: entry.id,
                part: *part,
            });
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{assemble, assemble_with_options, AssemblyError, ImageSource};

    const KNOWN: &str = "kiselev-xi8088-1.0.2 (open, vendor/kiselev-bios/fetch.sh)";

    fn config() -> MachineConfig {
        crate::bios(vec![0; 32768], Some(vec![0; 1_474_560]))
    }

    fn synthetic() -> FirmwareEntry {
        FirmwareEntry {
            sha256: "c35020473aed1b4642cd726cad727b63fff2824ad68cedd7ffb73c7cbd890479",
            ..KISELEV_XI8088
        }
    }

    #[test]
    fn xub_catalogue_entry_matches_the_pinned_fixture_recipe() {
        let entry = FirmwareEntry::XUB_R638_AT;
        assert_eq!(entry.id, "xub-r638-at");
        assert_eq!(entry.kind, FirmwareKind::OptionRom);
        assert_eq!(entry.size, 8192);
        assert_eq!(entry.base, FirmwareBase::Suggested(0xC8000));
        assert_eq!(entry.provides, &[FirmwareService::FixedDisk]);
        assert_eq!(entry.requires, &[FirmwarePart::AtaPrimary]);
        assert_eq!(entry.cpu_min, FirmwareCpu::I80286);
        assert_eq!(entry.boards, &["ibm-5170"]);
        assert_eq!(entry.status, FirmwareStatus::Listed);
        assert_eq!(
            entry.distribution,
            FirmwareDistribution::Open {
                fetch: "vendor/xtide-universal-bios/fetch.sh",
                license: "GPL-2.0-only",
                provenance: "XTIDE Universal BIOS r638 ide_at.bin (AT build)",
            }
        );
        let script = include_str!("../../../vendor/xtide-universal-bios/fetch.sh");
        let hash = script
            .lines()
            .find_map(|line| line.strip_prefix("OUTPUT_SHA256=\"")?.strip_suffix('"'))
            .expect("fixture script must pin its output hash");
        assert_eq!(entry.sha256, hash);
    }

    #[test]
    fn built_xub_fixture_matches_the_catalogue() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/xtide-universal-bios/ide_at-r638-8k.bin");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let message = format!(
                    "missing fixture {}; run vendor/xtide-universal-bios/fetch.sh",
                    path.display()
                );
                assert!(
                    std::env::var_os("UNEMUPC98_REQUIRE_FIXTURES").as_deref()
                        != Some(std::ffi::OsStr::new("1")),
                    "{message}"
                );
                eprintln!("skipping: {message}");
                return;
            }
            Err(error) => panic!("cannot read {}: {error}", path.display()),
        };
        let entry = FirmwareEntry::XUB_R638_AT;
        assert_eq!(bytes.len() as u64, entry.size);
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), entry.sha256);
        assert_eq!(&bytes[..3], &[0x55, 0xAA, 16]);
        assert!(bytes[6646..8191].iter().all(|&byte| byte == 0));
        assert_eq!(bytes[8191], 0x50);
        assert_eq!(
            bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
            0
        );
    }

    #[test]
    fn primary_ata_requirement_is_unmet_by_current_device_kinds() {
        let entry = FirmwareEntry {
            requires: &[FirmwarePart::AtaPrimary],
            ..synthetic()
        };
        let error = check(entry, AssemblyOptions::default()).unwrap_err();
        assert_eq!(
            error,
            ConfigError::FirmwareRequiresDevice {
                firmware: entry.id,
                part: FirmwarePart::AtaPrimary,
            }
        );
        assert_eq!(
            error.to_string(),
            "firmware \"kiselev-xi8088-1.0.2\" requires ata@1F0/14; none is configured"
        );
    }

    fn check(
        entry: FirmwareEntry,
        options: AssemblyOptions,
    ) -> Result<Option<UnlistedFirmware>, ConfigError> {
        check_system_bios(
            &config(),
            BoardModel::Ibm5170,
            &[entry],
            0xF8000,
            &[0; 32768],
            options,
        )
    }

    #[test]
    fn firmware_not_listed_exact_message() {
        let error = match assemble(&config()) {
            Err(AssemblyError::Config(error)) => error,
            _ => panic!("must reject unknown BIOS"),
        };
        assert!(matches!(error, ConfigError::FirmwareNotListed { .. }));
        assert_eq!(error.to_string(), format!("bios image (sha256 c35020473aed1b4642cd726cad727b63fff2824ad68cedd7ffb73c7cbd890479) is not a known image for board \"ibm-5170\"; known: {KNOWN}"));
    }

    #[test]
    fn unknown_message_lists_every_distribution() {
        let entries = [
            KISELEV_XI8088,
            FirmwareEntry {
                id: "ibm-5170-bios-1986",
                distribution: FirmwareDistribution::UserSupplied,
                ..KISELEV_XI8088
            },
        ];
        let error = check_system_bios(
            &config(),
            BoardModel::Ibm5170,
            &entries,
            0xF8000,
            &[0; 32768],
            AssemblyOptions::default(),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "bios image (sha256 c35020473aed1b4642cd726cad727b63fff2824ad68cedd7ffb73c7cbd890479) is not a known image for board \"ibm-5170\"; known: kiselev-xi8088-1.0.2 (open, vendor/kiselev-bios/fetch.sh), ibm-5170-bios-1986 (user-supplied)");
    }

    #[test]
    fn firmware_board_mismatch_exact_message_even_with_escape() {
        let entry = FirmwareEntry {
            id: "glabios-0.4.3-5160",
            boards: &["ibm-5160"],
            ..synthetic()
        };
        for allow_unlisted_firmware in [false, true] {
            let error = check(
                entry,
                AssemblyOptions {
                    allow_unlisted_firmware,
                },
            )
            .unwrap_err();
            assert!(matches!(error, ConfigError::FirmwareBoardMismatch { .. }));
            assert_eq!(
                error.to_string(),
                "firmware \"glabios-0.4.3-5160\" is built for ibm-5160, not ibm-5170"
            );
        }
    }

    #[test]
    fn firmware_needs_cpu_exact_message_even_with_escape() {
        let entry = FirmwareEntry {
            id: "seabios-1.17.0-isapc",
            cpu_min: FirmwareCpu::I80386,
            ..synthetic()
        };
        for allow_unlisted_firmware in [false, true] {
            let error = check(
                entry,
                AssemblyOptions {
                    allow_unlisted_firmware,
                },
            )
            .unwrap_err();
            assert!(matches!(error, ConfigError::FirmwareNeedsCpu { .. }));
            assert_eq!(
                error.to_string(),
                "firmware \"seabios-1.17.0-isapc\" needs an 80386 or later; cpu is 80286"
            );
        }
    }

    #[test]
    fn firmware_requires_device_exact_message_even_with_escape() {
        let entry = FirmwareEntry {
            id: "bochs-rombios-legacy",
            requires: &[FirmwarePart::VideoRom],
            ..synthetic()
        };
        for allow_unlisted_firmware in [false, true] {
            let error = check(
                entry,
                AssemblyOptions {
                    allow_unlisted_firmware,
                },
            )
            .unwrap_err();
            assert!(matches!(error, ConfigError::FirmwareRequiresDevice { .. }));
            assert_eq!(error.to_string(), "firmware \"bochs-rombios-legacy\" requires a video-bios option ROM at 0xC0000; none is configured");
        }
    }

    #[test]
    fn every_required_board_part_is_checked() {
        for part in [
            FirmwarePart::Kbc8042,
            FirmwarePart::RtcCmos,
            FirmwarePart::PicCascade,
        ] {
            let mut config = config();
            config.devices.retain(|device| match part {
                FirmwarePart::Kbc8042 => !matches!(device.kind, DeviceKind::Keyboard(_)),
                FirmwarePart::RtcCmos => !matches!(device.kind, DeviceKind::Rtc(_)),
                FirmwarePart::PicCascade => {
                    !matches!(device.kind, DeviceKind::Pic(p) if !p.cascade)
                }
                FirmwarePart::VideoRom | FirmwarePart::AtaPrimary => unreachable!(),
            });
            let entry = synthetic();
            let result = check_system_bios(
                &config,
                config.board,
                &[entry],
                0xF8000,
                &[0; 32768],
                AssemblyOptions::default(),
            );
            assert_eq!(
                result,
                Err(ConfigError::FirmwareRequiresDevice {
                    firmware: entry.id,
                    part
                })
            );
        }
        assert_eq!(check(synthetic(), AssemblyOptions::default()), Ok(None));
    }

    fn assembly_error(config: &MachineConfig, allow_unlisted_firmware: bool) -> ConfigError {
        match assemble_with_options(
            config,
            AssemblyOptions {
                allow_unlisted_firmware,
            },
        ) {
            Err(AssemblyError::Config(error)) => error,
            _ => panic!("invalid firmware layout must fail before execution"),
        }
    }

    #[test]
    fn missing_or_ram_loaded_bios_is_refused_even_with_escape() {
        for ram_load in [false, true] {
            let mut config = config();
            config.drives.clear();
            if ram_load {
                config.images[0].kind = ImageKind::RamLoad;
                config.images[0].provides.clear();
                config
                    .memory
                    .ram
                    .push(crate::RamRegion::new(0xF8000, 32768));
            } else {
                config.images.clear();
            }
            for allow in [false, true] {
                let error = assembly_error(&config, allow);
                assert_eq!(error, ConfigError::MissingSystemBios { board: "ibm-5170" });
                assert_eq!(
                    error.to_string(),
                    "board \"ibm-5170\" requires a system BIOS ROM"
                );
            }
        }
    }

    #[test]
    fn ram_cannot_shadow_any_part_of_the_system_bios_even_with_escape() {
        for base in [0xF8000, 0xFFFF0] {
            let mut config = config();
            config.memory.ram.push(crate::RamRegion::new(base, 1));
            for allow in [false, true] {
                let error = assembly_error(&config, allow);
                assert_eq!(
                    error,
                    ConfigError::SystemBiosOverlapsRam {
                        board: "ibm-5170",
                        addr: base
                    }
                );
                assert_eq!(
                    error.to_string(),
                    format!("system BIOS for board \"ibm-5170\" overlaps RAM at {base:#x}")
                );
            }
        }
    }

    #[test]
    fn other_images_cannot_cover_the_bios_or_reset_byte_even_with_escape() {
        for kind in [ImageKind::Rom, ImageKind::RamLoad] {
            for (base, size) in [(0xF8000, 32768), (0xFFFF0, 1)] {
                let mut config = config();
                config.images.push(LoadImage {
                    base,
                    source: crate::ImageSource::Blank { size },
                    kind,
                    provides: Vec::new(),
                });
                for allow in [false, true] {
                    let error = assembly_error(&config, allow);
                    assert_eq!(
                        error,
                        ConfigError::ImageOverlap {
                            a: 0,
                            b: 1,
                            addr: base
                        }
                    );
                    assert_eq!(
                        error.to_string(),
                        format!("images 0 and 1 overlap at {base:#x}")
                    );
                }
            }
        }
    }

    #[test]
    fn custom_reset_entries_are_refused_even_with_escape() {
        for (reset, display) in [
            (
                crate::ResetSpec::Entry { cs: 0, ip: 0x500 },
                "Entry 0000:0500",
            ),
            (
                crate::ResetSpec::Entry {
                    cs: 0xF000,
                    ip: 0xFFF0,
                },
                "Entry F000:FFF0",
            ),
            (
                crate::ResetSpec::ResetVector { cs: 0, ip: 0x500 },
                "ResetVector 0000:0500",
            ),
        ] {
            let mut config = config();
            config.cpu.reset = reset;
            for allow in [false, true] {
                let error = assembly_error(&config, allow);
                assert_eq!(
                    error,
                    ConfigError::FirmwareReset {
                        board: "ibm-5170",
                        reset
                    }
                );
                assert_eq!(
                    error.to_string(),
                    format!(
                        "board \"ibm-5170\" requires power-on ResetVector F000:FFF0; got {display}"
                    )
                );
            }
        }
    }

    #[test]
    fn listed_bios_cannot_be_bypassed_by_a_ram_entry() {
        let Some(rom) = fixture() else { return };
        let mut config = crate::bios(rom, Some(vec![0; 1_474_560]));
        config.images.push(LoadImage {
            base: 0x500,
            source: ImageSource::Inline(vec![0xFA, 0xF4]),
            kind: ImageKind::RamLoad,
            provides: Vec::new(),
        });
        config.cpu.reset = crate::ResetSpec::Entry { cs: 0, ip: 0x500 };
        for allow in [false, true] {
            assert_eq!(
                assembly_error(&config, allow).to_string(),
                "board \"ibm-5170\" requires power-on ResetVector F000:FFF0; got Entry 0000:0500"
            );
        }
        config.board = BoardModel::Bare;
        let mut assembled = assemble(&config).unwrap();
        assert_eq!(
            assembled.machine.run_bounded(Some(10)).unwrap(),
            machine::RunOutcome::Halted
        );
    }

    #[test]
    fn direct_catalogue_check_enforces_placement_even_with_escape() {
        for allow_unlisted_firmware in [false, true] {
            let error = check_system_bios(
                &config(),
                BoardModel::Ibm5170,
                &[synthetic()],
                0xF8000,
                &vec![0; 32769],
                AssemblyOptions {
                    allow_unlisted_firmware,
                },
            )
            .unwrap_err();
            assert_eq!(
                error,
                ConfigError::BiosPlacement {
                    base: 0xF8000,
                    size: 32769
                }
            );
            assert_eq!(error.to_string(), "system BIOS of 32769 bytes at 0xf8000 must have a positive size that is a multiple of 8 KiB and end at 0x100000");
        }
    }

    #[test]
    fn pic_cascade_requires_the_master_as_well_as_the_slave() {
        let mut config = config();
        config
            .devices
            .retain(|device| !matches!(device.kind, DeviceKind::Pic(pic) if pic.cascade));
        assert!(config
            .devices
            .iter()
            .any(|device| matches!(device.kind, DeviceKind::Pic(pic) if !pic.cascade)));
        for allow_unlisted_firmware in [false, true] {
            let error = check_system_bios(
                &config,
                config.board,
                &[synthetic()],
                0xF8000,
                &[0; 32768],
                AssemblyOptions {
                    allow_unlisted_firmware,
                },
            )
            .unwrap_err();
            assert_eq!(
                error,
                ConfigError::FirmwareRequiresDevice {
                    firmware: KISELEV_XI8088.id,
                    part: FirmwarePart::PicCascade
                }
            );
            assert_eq!(
                error.to_string(),
                "firmware \"kiselev-xi8088-1.0.2\" requires pic-cascade; none is configured"
            );
        }
    }

    #[test]
    fn bios_placement_exact_message() {
        let mut config = config();
        config.images[0].source = ImageSource::Inline(vec![0; 24 * 1024]);
        let error = config.validate().unwrap_err();
        assert_eq!(
            error,
            ConfigError::BiosPlacement {
                base: 0xF8000,
                size: 24 * 1024
            }
        );
        assert_eq!(
            error.to_string(),
            "system BIOS of 24 KiB at 0xf8000 must have a positive size that is a multiple of 8 KiB and end at 0x100000"
        );
    }

    #[test]
    fn escape_accepts_and_reports_unknown_but_enforces_placement() {
        let options = AssemblyOptions {
            allow_unlisted_firmware: true,
        };
        let mut config = config();
        let original = config.clone();
        let assembled = assemble_with_options(&config, options).unwrap();
        assert_eq!(config, original);
        assert_eq!(
            assembled.unlisted_firmware,
            vec![UnlistedFirmware {
                sha256: synthetic().sha256.into()
            }]
        );
        assert_eq!(assembled.unlisted_firmware[0].to_string(), "warning: running unlisted firmware (sha256 c35020473aed1b4642cd726cad727b63fff2824ad68cedd7ffb73c7cbd890479)");
        for (base, size) in [
            (0x100000, 0),
            (0xF8001, 32767),
            (0xF7FFF, 32769),
            (0xF0000, 32768),
            (u64::MAX, 8192),
        ] {
            config.images[0].base = base;
            config.images[0].source = ImageSource::Blank { size };
            assert!(matches!(
                assemble_with_options(&config, options),
                Err(AssemblyError::Config(ConfigError::BiosPlacement { .. }))
            ));
        }
        assert_eq!(check_bios_placement(0xFA000, 24 * 1024), Ok(()));
    }

    #[test]
    fn bare_boards_do_not_check_firmware_or_placement() {
        let mut config = crate::bios(vec![0; 17], None);
        assert_eq!(config.board, BoardModel::Bare);
        config.images[0].base = 0xF8000;
        assert!(config.board.firmware_catalogue().is_empty());
        assert!(assemble(&config).unwrap().unlisted_firmware.is_empty());
    }

    #[test]
    fn catalogue_identity_requires_size_and_system_bios_kind() {
        for entry in [
            FirmwareEntry {
                size: 8192,
                ..synthetic()
            },
            FirmwareEntry {
                kind: FirmwareKind::OptionRom,
                ..synthetic()
            },
        ] {
            assert!(matches!(
                check(entry, AssemblyOptions::default()),
                Err(ConfigError::FirmwareNotListed { .. })
            ));
        }
    }

    fn fixture() -> Option<Vec<u8>> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/kiselev-bios/bios-xi8088.bin");
        match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let message = format!(
                    "missing fixture {}; run vendor/kiselev-bios/fetch.sh",
                    path.display()
                );
                assert!(
                    std::env::var_os("UNEMUPC98_REQUIRE_FIXTURES").as_deref()
                        != Some(std::ffi::OsStr::new("1")),
                    "{message}"
                );
                eprintln!("skipping: {message}");
                None
            }
            Err(error) => panic!("cannot read {}: {error}", path.display()),
        }
    }

    #[test]
    fn real_kiselev_image_passes_inline_and_file_catalogue_checks() {
        let Some(bytes) = fixture() else { return };
        assert_eq!(bytes.len(), 32768);
        let mut config = config();
        for source in [
            ImageSource::Inline(bytes),
            ImageSource::File(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../vendor/kiselev-bios/bios-xi8088.bin"),
            ),
        ] {
            config.images[0].source = source;
            assert!(assemble(&config).unwrap().unlisted_firmware.is_empty());
        }
    }

    #[test]
    fn one_byte_changed_kiselev_is_rejected_with_full_hash_and_known_entries() {
        let Some(mut bytes) = fixture() else { return };
        bytes[0] ^= 1;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(hash.len(), 64);
        let mut config = config();
        config.images[0].source = ImageSource::Inline(bytes);
        let error = match assemble(&config) {
            Err(AssemblyError::Config(error)) => error,
            _ => panic!("changed image must be rejected"),
        };
        assert_eq!(
            error,
            ConfigError::FirmwareNotListed {
                sha256: hash.clone(),
                board: "ibm-5170",
                known: KNOWN.into()
            }
        );
        assert_eq!(error.to_string(), format!("bios image (sha256 {hash}) is not a known image for board \"ibm-5170\"; known: {KNOWN}"));
    }
}
