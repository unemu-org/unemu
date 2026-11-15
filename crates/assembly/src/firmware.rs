// SPDX-License-Identifier: AGPL-3.0-or-later
//! Firmware identities and board-owned BIOS and adapter ROM requirements.
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

/// The purpose of a catalogue image.
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
    /// Intel 80186 instruction set used by XT+ firmware.
    I80186,
    /// The currently configured Intel 80286 core.
    I80286,
    /// A catalogue requirement that the current core cannot satisfy.
    I80386,
}

impl std::fmt::Display for FirmwareCpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::I80186 => "80186",
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
    /// A 128-byte CMOS RTC with distinct addresses above 3Fh.
    /// Dallas Semiconductor DS12887 datasheet, Address Map. The AT MC146818's
    /// six-bit decoder aliases firmware RAM-size and CPU-count fields to disk
    /// geometry, so only an RTC configured with 128 bytes satisfies this requirement.
    RtcCmos128,
    /// Master and slave interrupt controllers.
    PicCascade,
    /// A video ROM at the conventional expansion address.
    VideoRom,
    /// Primary ATA channel using the AT fixed-disk wiring: 1F0h and IRQ 14.
    /// IBM Personal Computer AT Technical Reference, Section 1 "System Board",
    /// I/O Address Map and System Interrupts; see the firmware design §2.2.
    AtaPrimary,
    /// Register-level VGA, rather than the fixed text-only compatibility card.
    VgaRegisters,
}

impl std::fmt::Display for FirmwarePart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Kbc8042 => "kbc-8042",
            Self::RtcCmos => "rtc-cmos",
            Self::RtcCmos128 => "rtc-cmos-128",
            Self::PicCascade => "pic-cascade",
            Self::VideoRom => "a video-bios option ROM at 0xC0000",
            Self::AtaPrimary => "ata@1F0/14",
            Self::VgaRegisters => "register-level vga",
        })
    }
}

impl FirmwarePart {
    fn present(self, config: &MachineConfig) -> bool {
        let has =
            |predicate: fn(&DeviceKind) -> bool| config.devices.iter().any(|d| predicate(&d.kind));
        match self {
            Self::Kbc8042 => has(|kind| matches!(kind, DeviceKind::Keyboard(_))),
            Self::RtcCmos => has(|kind| matches!(kind, DeviceKind::Rtc(_))),
            Self::RtcCmos128 => has(
                |kind| matches!(kind, DeviceKind::Rtc(cfg) if cfg.ram == crate::CmosRam::Bytes128),
            ),
            Self::PicCascade => {
                has(|kind| matches!(kind, DeviceKind::Pic(p) if p.cascade))
                    && has(|kind| matches!(kind, DeviceKind::Pic(p) if !p.cascade))
            }
            Self::VideoRom => config
                .images
                .iter()
                .any(|image| image.kind == ImageKind::Rom && image.base == 0xC0000),
            Self::VgaRegisters => has(|kind| matches!(kind, DeviceKind::VgaRegisters(_))),
            Self::AtaPrimary => config.devices.iter().any(|device| {
                matches!(device.kind, DeviceKind::Ide(cfg)
                    if cfg.base == 0x1F0 && cfg.control == 0x3F6
                        && device.ports == cfg.port_windows())
                    && device.irq == Some(14)
                    && device.dma.is_none()
            }),
        }
    }
}

/// Hardware access and configuration conventions supported by a reviewed build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareCapability {
    /// A diagnostic system ROM tests this controller directly instead of
    /// offering its BIOS service. Only a matched system image can waive the
    /// corresponding device requirement; this does not provide INT 13h.
    /// Landmark Research International, ROM POST User's Manual v1.2,
    /// pp. 16–36; see the firmware design §6, rule 10.
    DirectHardwareTest(FirmwareService),
    /// IBM AT CMOS fixed-disk types select the system BIOS disk parameters.
    /// IBM Personal Computer AT Technical Reference (September 1985),
    /// Real-Time Clock/CMOS RAM Information, Fixed Disk Type Byte (Hex 12)
    /// and Drive C/D Extended Bytes (Hex 19/1A).
    CmosDriveTypes,
    /// QEMU/Bochs CMOS RAM, boot order and type-47 IDE geometry conventions.
    /// READ: QEMU hw/i386/pc.c, pc_cmos_init_late and cmos_init_hd;
    /// SeaBIOS src/fw/paravirt.c, qemu_preinit and qemu_cfg_e820.
    /// Requires a 128-byte CMOS RTC to prevent extended-address aliasing;
    /// mutually exclusive with the IBM AT fixed-disk table convention.
    CmosQemuLayout,
}

impl std::fmt::Display for FirmwareCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectHardwareTest(service) => write!(f, "direct-hardware-test-{service}"),
            Self::CmosDriveTypes => f.write_str("cmos-drive-types"),
            Self::CmosQemuLayout => f.write_str("cmos-qemu-layout"),
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
        /// SHA-256 of the corresponding source archive the fetch script builds
        /// reproducibly; only that archive is open source for this image
        /// without the user's own licence statement.
        source_archive_sha256: Option<&'static str>,
    },
    /// Identified by hash, supplied separately by the user.
    UserSupplied,
}

/// Source archive pins, one per fetch script (`SOURCE_ARCHIVE_SHA256`): the
/// Kiselev archive holds the source of both Xi8088 builds, the XTIDE archive
/// that of both r638 builds.
const KISELEV_SOURCE_ARCHIVE: &str =
    "ced63707661747fc045d1e6e565a5cc9be73f95a9c944accedd77b19467fb5f9";
const VGABIOS_SOURCE_ARCHIVE: &str =
    "80eaa4d73bb221d39257f89bc99798aa4b1d2c0cbae878cacee6e0d0c8d32cb4";
const XTIDE_SOURCE_ARCHIVE: &str =
    "7bbd5de269f4e1d52ae30b52531fcda842709b9ae49166ac249f9b37187b91e8";

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

/// One reviewed firmware build. Its services override an omitted declaration;
/// a nonempty restatement must name the same set; see `backlog/docs/design/machine-config.md` §Firmware catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareEntry {
    /// Stable versioned identifier.
    pub id: &'static str,
    /// Firmware purpose.
    pub kind: FirmwareKind,
    /// Full lowercase SHA-256 of the complete file or assembled chip bytes.
    pub sha256: &'static str,
    /// Exact file length in bytes, including any unmapped container prefix.
    pub size: u64,
    /// System BIOS images end at the top of the first MiB. Adapter addresses
    /// are suggestions; explicit placements must fit the system BIOS scan.
    pub base: FirmwareBase,
    /// Exclusive adapter-ROM scan bounds of a system BIOS, if any.
    pub scan_window: Option<(u64, u64)>,
    /// Separate video-ROM POST scan, with an exclusive end.
    pub video_scan_window: Option<(u64, u64)>,
    /// Unmapped prefix in an identified EPROM container (zero for raw ROMs).
    pub rom_offset: usize,
    /// Services documented for this build.
    pub provides: &'static [FirmwareService],
    /// Hardware access and configuration conventions supported by this build.
    pub capabilities: &'static [FirmwareCapability],
    /// Required modelled board parts.
    pub requires: &'static [FirmwarePart],
    /// Minimum CPU generation.
    pub cpu_min: FirmwareCpu,
    /// Compatible board identifiers, including boards reserved for future work.
    pub boards: &'static [&'static str],
    /// Fetch, licence and provenance information.
    pub distribution: FirmwareDistribution,
    /// Known alternative chip layouts; open firmware has none.
    pub parts: &'static [&'static [KnownPart]],
    /// Execution evidence.
    pub status: FirmwareStatus,
}

impl FirmwareEntry {
    /// IBM Display Adapter (ISA VGA, 1987), full 32 KiB EPROM container.
    /// The supplied identity covers the unused 8 KiB prefix as well as the
    /// 24 KiB ROM. See firmware-and-machine-classes.md §Video ROM placement.
    pub const IBM_VGA_1987: Self = Self {
        id: "ibm-vga-bios-1987-32k",
        kind: FirmwareKind::VideoBios,
        sha256: "51b49761f72780a696c36f3d2e74a8168c1c99b865070f7f90b8e621a8b5ae17",
        size: 32768,
        base: FirmwareBase::Suggested(0xC0000),
        scan_window: None,
        video_scan_window: None,
        rom_offset: 0x2000,
        provides: &[],
        capabilities: &[],
        requires: &[FirmwarePart::VgaRegisters],
        cpu_min: FirmwareCpu::I80286,
        boards: &["ibm-5170"],
        distribution: FirmwareDistribution::UserSupplied,
        parts: &[],
        status: FirmwareStatus::Tested,
    };

    /// Open standard-VGA video BIOS: the unemu fork of the LGPL VGABIOS 0.8a,
    /// built for ISA VGA with 80286 instructions only (no VBE or PCI header).
    ///
    /// IBM VGA/XGA Technical Reference, Video BIOS: INT 10h services for the
    /// register-level card. IBM Personal Computer AT Technical Reference,
    /// "Adapters with System-Accessible ROM Modules": the raw 32 KiB image
    /// starts with 55AAh and length byte 40h and fills the whole C0000h–C7FFFh
    /// video-ROM window. See `vendor/vgabios/README.md` for the release pin.
    pub const VGABIOS_0_8A_UNEMU_1_STD: Self = Self {
        id: "vgabios-0.8a-unemu.1-std",
        kind: FirmwareKind::VideoBios,
        sha256: "7570ef9c66fd386450bb35fd2b6c91aba6f8ec69a565a1d7747e6e6716ea5389",
        size: 32768,
        base: FirmwareBase::Suggested(0xC0000),
        scan_window: None,
        video_scan_window: None,
        rom_offset: 0,
        provides: &[],
        capabilities: &[],
        requires: &[FirmwarePart::VgaRegisters],
        cpu_min: FirmwareCpu::I80286,
        boards: &["ibm-5170"],
        distribution: FirmwareDistribution::Open {
            fetch: "vendor/vgabios/fetch.sh",
            license: "LGPL-2.1-or-later",
            provenance: "unemu-org/vgabios v0.8a-unemu.1, 8e154f04c5b1c8029d1ac3a3827f077352d579b7 (std build)",
            source_archive_sha256: Some(VGABIOS_SOURCE_ARCHIVE),
        },
        parts: &[],
        status: FirmwareStatus::Tested,
    };

    /// XTIDE Universal BIOS XT+ build configured for primary 16-bit ATA.
    ///
    /// XTIDE Universal BIOS, RomVars.inc, IDEVARS and DEVICE_16BIT_ATA;
    /// FloppyDrive.asm, XT build: installs the original floppy vector safely.
    /// See `backlog/docs/design/firmware-and-machine-classes.md` §2.2.
    pub const XUB_R638_XTP_ATA1F0: Self = Self {
        id: "xub-r638-xtp-ata1f0",
        cpu_min: FirmwareCpu::I80186,
        sha256: "54975501e6887c836b8d57e7aad78421e745a9b71e250f94d5d1f9631eab1513",
        status: FirmwareStatus::Tested,
        distribution: FirmwareDistribution::Open {
            fetch: "vendor/xtide-universal-bios/fetch.sh",
            license: "GPL-2.0-only",
            provenance: "XTIDE Universal BIOS r638 ide_xtp.bin (16-bit ATA, polling, boot A:)",
            source_archive_sha256: Some(XTIDE_SOURCE_ARCHIVE),
        },
        ..Self::XUB_R638_AT
    };

    /// Padded and checksummed XTIDE Universal BIOS AT build.
    ///
    /// XTIDE Universal BIOS, Main.asm, USE_AT build: primary ATA task file at
    /// 1F0h and IRQ 14. See `backlog/docs/design/firmware-and-machine-classes.md`
    /// §2.2 and `vendor/xtide-universal-bios/README.md` for the binary provenance.
    /// The fixed-disk boot profile is qualified separately from ROM validation.
    pub const XUB_R638_AT: Self = Self {
        id: "xub-r638-at",
        kind: FirmwareKind::OptionRom,
        sha256: "6409de912eab620ea144b79e723bed478814047dc2766fc19fd6e2e3690662d0",
        size: 8192,
        base: FirmwareBase::Suggested(0xC8000),
        scan_window: None,
        video_scan_window: None,
        rom_offset: 0,
        provides: &[FirmwareService::FixedDisk],
        capabilities: &[],
        requires: &[FirmwarePart::AtaPrimary],
        cpu_min: FirmwareCpu::I80286,
        boards: &["ibm-5170"],
        distribution: FirmwareDistribution::Open {
            fetch: "vendor/xtide-universal-bios/fetch.sh",
            license: "GPL-2.0-only",
            provenance: "XTIDE Universal BIOS r638 ide_at.bin (AT build)",
            source_archive_sha256: Some(XTIDE_SOURCE_ARCHIVE),
        },
        parts: &[],
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
    // Kiselev 8088_bios bios.asm, video initialization: DX=C000h, BX=C800h;
    // detect_rom_ext: DX=C800h, BX=F800h; both call extension_scan.
    scan_window: Some((0xC8000, 0xF8000)),
    video_scan_window: Some((0xC0000, 0xC8000)),
    rom_offset: 0,
    provides: &[FirmwareService::Floppy],
    // Kiselev 8088_bios rtc.inc, rtc_init/nvram_save: fixed-disk bytes are
    // checksummed but never interpreted as a system BIOS disk configuration.
    capabilities: &[],
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
        source_archive_sha256: Some(KISELEV_SOURCE_ARCHIVE),
    },
    parts: &[],
    status: FirmwareStatus::Tested,
};

/// Xi 8088 with AT diskette reset semantics, built by the same pinned recipe.
/// IBM PC AT Technical Reference, BIOS listing, DSKETTE (OK_FUNC/OK_DRV)
/// permits reset/status calls regardless of DL. See `vendor/kiselev-bios/README.md`.
pub const KISELEV_XI8088_ATRESET: FirmwareEntry = FirmwareEntry {
    id: "kiselev-xi8088-1.0.2-atreset",
    sha256: "61589573c9809273198b137d9419c87242b0a5f63c2ad834a9565950c3d6becc",
    distribution: FirmwareDistribution::Open {
        fetch: "vendor/kiselev-bios/fetch.sh",
        license: "GPL-3.0-or-later",
        provenance: "skiselev/8088_bios a4af458a72bc33c4be066b33bd76e8af2b24111c (MACHINE_XI8088), at-reset.patch",
        source_archive_sha256: Some(KISELEV_SOURCE_ARCHIVE),
    },
    status: FirmwareStatus::Tested,
    ..KISELEV_XI8088
};

/// Choose the board display switch once the system BIOS bytes are available.
///
/// The two pinned Kiselev Xi8088 builds interpret the AT display switch with
/// inverted polarity, so Colour selects their MDA console. IBM Personal
/// Computer AT Technical Reference, System Board, Keyboard Controller,
/// "Input-Port Bit Definitions", defines the model's unchanged AT polarity.
/// Other images retain Auto. See machine-config.md §Primary display.
/// Both in-memory presets and file loaders use this policy after reading bytes.
pub fn bios_display_switch(bytes: &[u8]) -> crate::DisplaySwitch {
    if [KISELEV_XI8088, KISELEV_XI8088_ATRESET]
        .iter()
        .any(|entry| entry.identifies(bytes))
    {
        crate::DisplaySwitch::Colour
    } else {
        crate::DisplaySwitch::Auto
    }
}

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
    if let Some(image) = config.images.iter().find(|image| is_system_bios(image)) {
        return Ok(image);
    }
    if let Some(image) = config
        .system_bios_image
        .and_then(|index| config.images.get(index))
        .filter(|image| image.kind == ImageKind::Rom)
    {
        return Ok(image);
    }
    let mut roms = config
        .images
        .iter()
        .filter(|image| image.kind == ImageKind::Rom);
    if let Some(image) = roms.next() {
        if roms.next().is_none() {
            return Ok(image);
        }
    }
    Err(ConfigError::MissingSystemBios {
        board: config.board.id(),
    })
}

pub(crate) fn unresolved_roms(config: &MachineConfig) -> bool {
    config.images.iter().any(|image| {
        image.kind == ImageKind::Rom
            && matches!(
                image.source,
                crate::ImageSource::File(_) | crate::ImageSource::Interleaved(_)
            )
    })
}

pub(crate) fn is_system_bios(image: &LoadImage) -> bool {
    image.kind == ImageKind::Rom
        && image
            .source
            .known_len()
            .and_then(|size| image.base.checked_add(size))
            == Some(0x100000)
}

/// Retain the complete file identity while mapping an adapter-owned suffix.
/// Adapter socket layouts must never crop the CPU reset firmware.
pub(crate) fn rom_source(
    bytes: Vec<u8>,
    kind: ImageKind,
    system_bios: bool,
    catalogue: &[FirmwareEntry],
) -> crate::ImageSource {
    if kind == ImageKind::Rom && !system_bios {
        if let Some(entry) = catalogue.iter().find(|entry| {
            entry.kind != FirmwareKind::SystemBios
                && entry.rom_offset != 0
                && entry.identifies(&bytes)
        }) {
            return crate::ImageSource::RomContainer {
                bytes,
                offset: entry.rom_offset,
            };
        }
    }
    crate::ImageSource::Inline(bytes)
}

pub(crate) fn matched_entry<'a>(
    image: &LoadImage,
    catalogue: &'a [FirmwareEntry],
) -> Option<&'a FirmwareEntry> {
    let (bytes, offset) = match &image.source {
        crate::ImageSource::Inline(bytes) => (bytes, 0),
        crate::ImageSource::RomContainer { bytes, offset } => (bytes, *offset),
        _ => return None,
    };
    let system = is_system_bios(image);
    let hash = format!("{:x}", Sha256::digest(bytes));
    catalogue.iter().find(|entry| {
        (entry.kind == FirmwareKind::SystemBios) == system
            && entry.rom_offset == offset
            && entry.size == bytes.len() as u64
            && entry.sha256 == hash
    })
}

pub(crate) fn scan_firmware(config: &MachineConfig) -> Result<&FirmwareEntry, ConfigError> {
    scan_firmware_from(config, config.board.firmware_catalogue())
}

fn scan_firmware_from<'a>(
    config: &MachineConfig,
    catalogue: &'a [FirmwareEntry],
) -> Result<&'a FirmwareEntry, ConfigError> {
    let bios = system_bios(config)?;
    // An unlisted experimental BIOS inherits the board's memory isolation.
    matched_entry(bios, catalogue)
        .or_else(|| {
            catalogue
                .iter()
                .find(|entry| entry.kind == FirmwareKind::SystemBios)
        })
        .ok_or(ConfigError::MissingSystemBios {
            board: config.board.id(),
        })
}

fn service_set(services: &[FirmwareService]) -> String {
    let mut names: Vec<_> = services.iter().map(ToString::to_string).collect();
    names.sort_unstable();
    names.dedup();
    format!("{names:?}")
}

fn entry_services<'a>(
    image_index: usize,
    image: &'a LoadImage,
    entry: &FirmwareEntry,
) -> Result<&'a [FirmwareService], ConfigError> {
    let declared = service_set(&image.provides);
    let expected = service_set(entry.provides);
    if !image.provides.is_empty() && declared != expected {
        return Err(ConfigError::FirmwareProvidesMismatch {
            image_index,
            firmware: entry.id,
            declared,
            expected,
        });
    }
    Ok(entry.provides)
}

/// Resolve catalogue-owned services; defer unread files until resolution.
pub(crate) fn image_services(
    config: &MachineConfig,
    index: usize,
) -> Result<Option<&[FirmwareService]>, ConfigError> {
    image_services_from(config, index, config.board.firmware_catalogue())
}

pub(crate) fn image_services_from<'a>(
    config: &'a MachineConfig,
    index: usize,
    catalogue: &[FirmwareEntry],
) -> Result<Option<&'a [FirmwareService]>, ConfigError> {
    let image = &config.images[index];
    if let Some(entry) = matched_entry(image, catalogue) {
        return entry_services(index, image, entry).map(Some);
    }
    if matches!(
        image.source,
        crate::ImageSource::File(_) | crate::ImageSource::Interleaved(_)
    ) && image.provides.is_empty()
    {
        return Ok(None);
    }
    Ok(Some(&image.provides))
}

/// Validate adapter ROM format and compatibility before mapping.
///
/// IBM Personal Computer AT Technical Reference (March 1984), System BIOS,
/// "Adapters with System-Accessible ROM Modules", p. 5-11: 55AAh signature,
/// 512-byte length units, modulo-256 checksum, 2 KiB scan steps. Its documented
/// C8000h–E0000h extension range differs from Kiselev 8088_bios bios.asm,
/// detect_rom_ext / extension_scan (C8000h–F8000h, exclusive end). Both BIOSes
/// separately scan C0000h–C7FFFh for video ROMs (IBM TEST1; Kiselev video init).
/// See `backlog/docs/design/firmware-and-machine-classes.md` §6.
pub(crate) fn validate_option_roms(config: &MachineConfig) -> Result<(), ConfigError> {
    validate_option_roms_with_catalogue(config, config.board.firmware_catalogue())
}

pub(crate) fn validate_option_roms_with_catalogue(
    config: &MachineConfig,
    catalogue: &[FirmwareEntry],
) -> Result<(), ConfigError> {
    if catalogue.is_empty() {
        return Ok(());
    }
    if system_bios(config).is_err() && unresolved_roms(config) {
        return Ok(());
    }
    let firmware = scan_firmware_from(config, catalogue)?;
    let bios = system_bios(config)?;
    for (image_index, image) in config.images.iter().enumerate() {
        if image.kind != ImageKind::Rom || std::ptr::eq(image, bios) {
            continue;
        }
        let video_address = (0xC0000..0xC8000).contains(&image.base);
        let entry = matched_entry(image, catalogue);
        let video_rom =
            video_address || entry.is_some_and(|entry| entry.kind == FirmwareKind::VideoBios);
        // Only reviewed BIOS bytes establish the separate video scan. A file
        // must be identified before this check; the old extension fallback
        // remains available for unlisted experimental system firmware.
        if video_rom
            && matches!(
                bios.source,
                crate::ImageSource::File(_) | crate::ImageSource::Interleaved(_)
            )
        {
            continue;
        }
        if let Some(entry) =
            entry.filter(|entry| video_address && entry.kind == FirmwareKind::OptionRom)
        {
            return Err(ConfigError::OptionRomInVideoWindow {
                image_index,
                firmware: entry.id,
            });
        }
        let (start, end) = if video_rom {
            let known_bios = matched_entry(bios, catalogue);
            known_bios.and_then(|bios| bios.video_scan_window).ok_or(
                ConfigError::FirmwareDoesNotScanVideoRoms {
                    firmware: known_bios.map_or("unlisted", |bios| bios.id),
                    image_index,
                },
            )?
        } else {
            firmware
                .scan_window
                .ok_or(ConfigError::FirmwareDoesNotScanOptionRoms {
                    firmware: firmware.id,
                    image_index,
                })?
        };
        if !image.base.is_multiple_of(2048) {
            return Err(ConfigError::OptionRomMisaligned {
                image_index,
                base: image.base,
            });
        }
        if image.base < start
            || image.base >= end
            || image
                .source
                .known_len()
                .is_some_and(|size| image.base.saturating_add(size) > end)
        {
            return Err(ConfigError::OptionRomOutsideScanWindow {
                image_index,
                base: image.base,
                start,
                end,
                firmware: firmware.id,
            });
        }
        let bytes =
            match &image.source {
                crate::ImageSource::Inline(bytes) => bytes.as_slice(),
                crate::ImageSource::RomContainer { bytes, offset } => bytes
                    .get(*offset..)
                    .ok_or_else(|| ConfigError::OptionRomBadHeader {
                        image_index,
                        name: format!("at {:#X}", image.base),
                        reason: "container offset exceeds image length".into(),
                    })?,
                crate::ImageSource::File(_) | crate::ImageSource::Interleaved(_) => continue,
                crate::ImageSource::Blank { .. } => {
                    return Err(ConfigError::OptionRomBadHeader {
                        image_index,
                        name: format!("at {:#X}", image.base),
                        reason: "expected 55AAh signature and a length byte".into(),
                    })
                }
            };
        let name = entry.map_or_else(|| format!("at {:#X}", image.base), |entry| entry.id.into());
        let reason = if bytes.len() < 3 || bytes[..2] != [0x55, 0xAA] {
            Some("expected 55AAh signature and a length byte".to_string())
        } else if bytes[2] == 0 || usize::from(bytes[2]) * 512 != bytes.len() {
            Some(format!(
                "length byte declares {} bytes, image has {} (must match and be nonzero)",
                usize::from(bytes[2]) * 512,
                bytes.len()
            ))
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(ConfigError::OptionRomBadHeader {
                image_index,
                name,
                reason,
            });
        }
        let sum = bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        if sum != 0 {
            return Err(ConfigError::OptionRomBadChecksum {
                image_index,
                name,
                sum,
            });
        }
        if let Some(entry) = entry {
            entry_services(image_index, image, entry)?;
            check_entry_requirements(config, config.board, entry, Some(image_index))?;
        }
    }
    Ok(())
}

/// Check resolved adapter identities against the board catalogue. The run-only
/// escape relaxes membership alone; format and compatibility stay mandatory.
pub fn check_option_rom_catalogue(
    config: &MachineConfig,
    options: AssemblyOptions,
) -> Result<Vec<UnlistedFirmware>, ConfigError> {
    let catalogue = config.board.firmware_catalogue();
    if catalogue.is_empty() {
        return Ok(Vec::new());
    }
    validate_option_roms(config)?;
    let bios = system_bios(config)?;
    let mut unlisted = Vec::new();
    for (image_index, image) in config.images.iter().enumerate() {
        if image.kind != ImageKind::Rom || std::ptr::eq(image, bios) {
            continue;
        }
        let bytes = match &image.source {
            crate::ImageSource::Inline(bytes) | crate::ImageSource::RomContainer { bytes, .. } => {
                bytes
            }
            _ => continue,
        };
        if matched_entry(image, catalogue).is_some() {
            continue;
        }
        let sha256 = format!("{:x}", Sha256::digest(bytes));
        if !options.allow_unlisted_firmware {
            return Err(ConfigError::OptionRomNotListed {
                image_index,
                sha256,
                board: config.board.id(),
                known: catalogue
                    .iter()
                    .filter(|entry| entry.kind != FirmwareKind::SystemBios)
                    .map(FirmwareEntry::listing)
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
        unlisted.push(UnlistedFirmware { sha256 });
    }
    Ok(unlisted)
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
    if let Some(entry) = catalogue
        .iter()
        .find(|entry| entry.kind == FirmwareKind::VideoBios && entry.identifies(bytes))
    {
        return Err(ConfigError::VideoBiosAsSystemBios { firmware: entry.id });
    }
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
                .filter(|entry| entry.kind == FirmwareKind::SystemBios)
                .map(FirmwareEntry::listing)
                .collect::<Vec<_>>()
                .join(", "),
        });
    };
    if let Some(index) = config.images.iter().position(is_system_bios) {
        entry_services(index, &config.images[index], entry)?;
    }
    check_entry_requirements(config, board, entry, None)?;
    Ok(None)
}

pub(crate) fn check_entry_requirements(
    config: &MachineConfig,
    board: BoardModel,
    entry: &FirmwareEntry,
    option_index: Option<usize>,
) -> Result<(), ConfigError> {
    check_cmos_capabilities(entry)?;
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
    let required_cmos = entry
        .capabilities
        .contains(&FirmwareCapability::CmosQemuLayout)
        .then_some(FirmwarePart::RtcCmos128);
    for part in required_cmos.iter().chain(entry.requires) {
        if !part.present(config) {
            if let (FirmwarePart::AtaPrimary, Some(image_index)) = (part, option_index) {
                return Err(ConfigError::OptionRomCardMismatch {
                    image_index,
                    firmware: entry.id,
                });
            }
            return Err(ConfigError::FirmwareRequiresDevice {
                firmware: entry.id,
                part: *part,
            });
        }
    }
    Ok(())
}

/// Reject conflicting interpretations of the same fixed-disk CMOS bytes.
pub(crate) fn check_cmos_capabilities(entry: &FirmwareEntry) -> Result<(), ConfigError> {
    if entry
        .capabilities
        .contains(&FirmwareCapability::CmosDriveTypes)
        && entry
            .capabilities
            .contains(&FirmwareCapability::CmosQemuLayout)
    {
        return Err(ConfigError::ConflictingCmosCapabilities { firmware: entry.id });
    }
    Ok(())
}

/// Expected chip identity in one alternative layout of a user-supplied BIOS.
/// MAME src/mame/pc/at.cpp, ibm5170/ibm5170a: U27 even and U47 odd.
/// The four 27128 dumps in U17/U37-low, U27/U47-high order reproduce the
/// MAME-listed 27256 pair's image (measured); see firmware design §3.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownPart {
    /// Board socket label.
    pub label: &'static str,
    /// IBM chip part number.
    pub part_number: &'static str,
    /// Byte lane in the assembled ROM.
    pub lane: crate::Lane,
    /// Offset in assembled bytes.
    pub offset: u64,
    /// Chip dump size.
    pub size: u64,
    /// SHA-256 of the chip dump.
    pub sha256: &'static str,
}
impl FirmwareEntry {
    /// Compare a complete file or assembled chips with the catalogue identity.
    pub fn identifies(&self, bytes: &[u8]) -> bool {
        self.size == bytes.len() as u64 && self.sha256 == format!("{:x}", Sha256::digest(bytes))
    }

    /// Human-readable catalogue membership diagnostic.
    pub fn listing(&self) -> String {
        if self.distribution != FirmwareDistribution::UserSupplied || self.parts.is_empty() {
            return format!("{} ({})", self.id, self.distribution);
        }
        let layouts = self
            .parts
            .iter()
            .map(|layout| {
                layout
                    .iter()
                    .map(|p| {
                        let offset = if layout.len() > 2 {
                            format!("@{}", p.offset)
                        } else {
                            String::new()
                        };
                        format!("{} {} {}{offset}", p.label, p.part_number, p.lane)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .collect::<Vec<_>>()
            .join(" or ");
        format!(
            "{} (user-supplied: {}; see vendor/user-roms/README.md)",
            self.id, layouts
        )
    }
}

/// Diagnose chip assignments only after the assembled identity is unlisted.
/// Returns `Ok(())` for a listed assembled hash and under the unlisted-firmware
/// option. Otherwise per-chip hashes explain wrong dumps and assignments without
/// making filenames authoritative.
pub fn check_firmware_parts(
    parts: &[(crate::Lane, u64, Vec<u8>)],
    bytes: &[u8],
    catalogue: &[FirmwareEntry],
    options: AssemblyOptions,
) -> Result<(), ConfigError> {
    let hash = format!("{:x}", Sha256::digest(bytes));
    if options.allow_unlisted_firmware
        || catalogue.iter().any(|e| {
            e.kind == FirmwareKind::SystemBios && e.sha256 == hash && e.size == bytes.len() as u64
        })
    {
        return Ok(());
    }
    let hashes: Vec<_> = parts
        .iter()
        .map(|(lane, offset, bytes)| {
            (
                *lane,
                *offset,
                bytes.len() as u64,
                format!("{:x}", Sha256::digest(bytes)),
            )
        })
        .collect();
    let mut best = None;
    for entry in catalogue
        .iter()
        .filter(|e| e.kind == FirmwareKind::SystemBios)
    {
        for layout in entry
            .parts
            .iter()
            .filter(|layout| layout.len() == parts.len())
        {
            let count = hashes
                .iter()
                .filter(|(_, _, size, hash)| {
                    layout
                        .iter()
                        .any(|known| known.size == *size && known.sha256 == hash)
                })
                .count();
            if count > 0 && best.as_ref().is_none_or(|(n, _, _)| count > *n) {
                best = Some((count, entry, *layout));
            }
        }
    }
    if let Some((_, entry, layout)) = best {
        let uses: Vec<_> = layout
            .iter()
            .map(|known| {
                hashes
                    .iter()
                    .filter(|(_, _, size, hash)| known.size == *size && known.sha256 == hash)
                    .count()
            })
            .collect();
        let has_duplicates = uses.iter().any(|count| *count > 1);
        let mut mismatches = Vec::new();
        if has_duplicates {
            for (known, count) in layout.iter().zip(&uses) {
                if *count > 1 {
                    mismatches.push(format!(
                        "{} {} appears {count} times",
                        known.label, known.part_number
                    ));
                } else if *count == 0 {
                    mismatches.push(format!(
                        "{} {} ({} lane offset {}) is missing",
                        known.label, known.part_number, known.lane, known.offset
                    ));
                }
            }
        }
        let mut lanes_swapped = uses.iter().all(|count| *count == 1);
        for (lane, offset, size, hash) in &hashes {
            let expected = layout
                .iter()
                .find(|known| known.lane == *lane && known.offset == *offset);
            if expected.is_none() {
                lanes_swapped = false;
                mismatches.push(format!(
                    "{lane} lane offset {offset} is not part of {} layout",
                    entry.id
                ));
            }
            if let Some(known) = layout
                .iter()
                .find(|known| known.size == *size && known.sha256 == hash)
            {
                let repeated = hashes
                    .iter()
                    .filter(|(_, _, size, hash)| known.size == *size && known.sha256 == hash)
                    .count()
                    > 1;
                if repeated || (known.lane == *lane && known.offset == *offset) {
                    continue;
                }
                lanes_swapped &= known.offset == *offset && known.lane != *lane;
                mismatches.push(format!(
                    "{lane} lane offset {offset} holds {} {}, which belongs in the {} lane at offset {}",
                    known.label, known.part_number, known.lane, known.offset
                ));
            } else {
                lanes_swapped = false;
                if let Some(expected) = expected {
                    mismatches.push(format!(
                        "{lane} lane offset {offset}: expected {} {} sha256 {}, actual {hash} (bad dump or wrong chip)",
                        expected.label, expected.part_number, expected.sha256
                    ));
                }
            }
        }
        if !mismatches.is_empty() {
            if lanes_swapped {
                return Err(ConfigError::FirmwarePartsSwapped {
                    firmware: entry.id,
                    chips: mismatches.join("; "),
                });
            }
            return Err(ConfigError::FirmwarePartsMismatch {
                firmware: entry.id,
                mismatches: mismatches.join("; "),
            });
        }
    }
    Ok(())
}

/// IBM PC AT system BIOS, identified without distributing ROM bytes.
/// IBM PC AT Technical Reference BIOS listing, TEST2.ASM, "CHECK FOR OPTIONAL
/// ROM FROM C800->E000 IN 2K BLOCKS" (MOV DX,0C800H ... CMP DX,0E000H);
/// DSKETTE and DISK supply floppy and AT fixed-disk services.
/// MAME src/mame/pc/at.cpp, commit cabb909b38, ibm5170/ibm5170a:
/// U27 loads even bytes and U47 odd bytes (ROM_SKIP(1)). The four-chip 1984
/// alternative was measured to assemble to the same image. See firmware design §3.5.
pub const IBM_5170_1984_01_10: FirmwareEntry = FirmwareEntry {
    id: "ibm-5170-bios-1984-01-10",
    sha256: "74e7b36b4ec0adc5ac3277a887579996c1d2aa755b9892ef3afe7485c10ce04f",
    size: 65536,
    kind: FirmwareKind::SystemBios,
    base: FirmwareBase::TopOfFirstMib,
    scan_window: Some((0xC8000, 0xE0000)),
    video_scan_window: Some((0xC0000, 0xC8000)),
    rom_offset: 0,
    provides: &[FirmwareService::Floppy, FirmwareService::FixedDisk],
    capabilities: &[FirmwareCapability::CmosDriveTypes],
    requires: &[
        FirmwarePart::Kbc8042,
        FirmwarePart::RtcCmos,
        FirmwarePart::PicCascade,
    ],
    cpu_min: FirmwareCpu::I80286,
    boards: &["ibm-5170"],
    distribution: FirmwareDistribution::UserSupplied,
    status: FirmwareStatus::Listed,
    parts: &[
        &[
            KnownPart {
                label: "U27",
                part_number: "6181028",
                lane: crate::Lane::Even,
                offset: 0,
                size: 32768,
                sha256: "a7d0f995c25a29f2556fce563939cbd101f9225f71103debe441e3b6458cf933",
            },
            KnownPart {
                label: "U47",
                part_number: "6181029",
                lane: crate::Lane::Odd,
                offset: 0,
                size: 32768,
                sha256: "fadb3a5c949b0096b2f753c76bac2edf73040066c1959eeee8f7d092c836b6c5",
            },
        ],
        &[
            KnownPart {
                label: "U17",
                part_number: "6181024",
                lane: crate::Lane::Even,
                offset: 0,
                size: 16384,
                sha256: "b9a9aa7a32c4d50ac16034a489962cbd06455ab14cbb78a6d2bbf519b22c8a73",
            },
            KnownPart {
                label: "U37",
                part_number: "6181026",
                lane: crate::Lane::Odd,
                offset: 0,
                size: 16384,
                sha256: "a54d9546361464489a118e6d7a7f9f959e6678013ab4f4e8a98993ac7bdc916d",
            },
            KnownPart {
                label: "U27",
                part_number: "6181025",
                lane: crate::Lane::Even,
                offset: 32768,
                size: 16384,
                sha256: "87340797ab2f2eb86185f5289bd0a2bc2e9f14e0633770a87bf4cb0624208a5c",
            },
            KnownPart {
                label: "U47",
                part_number: "6181027",
                lane: crate::Lane::Odd,
                offset: 32768,
                size: 16384,
                sha256: "a1106cc91a86251c524db72934f7eaa5abb39c2a986a5ac3477a017da7aac6e3",
            },
        ],
    ],
};

/// IBM PC AT system BIOS, identified without distributing ROM bytes.
/// IBM PC AT Technical Reference BIOS listing, TEST2.ASM, "CHECK FOR OPTIONAL
/// ROM FROM C800->E000 IN 2K BLOCKS" (MOV DX,0C800H ... CMP DX,0E000H);
/// DSKETTE and DISK supply floppy and AT fixed-disk services.
/// MAME src/mame/pc/at.cpp, commit cabb909b38, ibm5170/ibm5170a:
/// U27 loads even bytes and U47 odd bytes (ROM_SKIP(1)). See firmware design §3.5.
pub const IBM_5170_1985_06_10: FirmwareEntry = FirmwareEntry {
    id: "ibm-5170-bios-1985-06-10",
    sha256: "6fb653b58e678cb69b0f943a105c6c803b18a6db23d13c0abccec5b66c6ee652",
    size: 65536,
    kind: FirmwareKind::SystemBios,
    base: FirmwareBase::TopOfFirstMib,
    scan_window: Some((0xC8000, 0xE0000)),
    video_scan_window: Some((0xC0000, 0xC8000)),
    rom_offset: 0,
    provides: &[FirmwareService::Floppy, FirmwareService::FixedDisk],
    capabilities: &[FirmwareCapability::CmosDriveTypes],
    requires: &[
        FirmwarePart::Kbc8042,
        FirmwarePart::RtcCmos,
        FirmwarePart::PicCascade,
    ],
    cpu_min: FirmwareCpu::I80286,
    boards: &["ibm-5170"],
    distribution: FirmwareDistribution::UserSupplied,
    status: FirmwareStatus::Listed,
    parts: &[&[
        KnownPart {
            label: "U27",
            part_number: "6480090",
            lane: crate::Lane::Even,
            offset: 0,
            size: 32768,
            sha256: "c8654ed7a922644db2b2cce54c540fcfa7cbbae8c514d7eb3dbfbb8187471bfd",
        },
        KnownPart {
            label: "U47",
            part_number: "6480091",
            lane: crate::Lane::Odd,
            offset: 0,
            size: 32768,
            sha256: "470ab30f9eb7d56c24f5ce10c7d307c539b9f923ea97dd536d2d5ba7e36d471b",
        },
    ]],
};

/// IBM PC AT system BIOS, identified without distributing ROM bytes.
/// IBM PC AT Technical Reference BIOS listing, TEST2.ASM, "CHECK FOR OPTIONAL
/// ROM FROM C800->E000 IN 2K BLOCKS" (MOV DX,0C800H ... CMP DX,0E000H);
/// DSKETTE and DISK supply floppy and AT fixed-disk services.
/// MAME src/mame/pc/at.cpp, commit cabb909b38, ibm5170/ibm5170a:
/// U27 loads even bytes and U47 odd bytes (ROM_SKIP(1)). See firmware design §3.5.
pub const IBM_5170_1985_11_15: FirmwareEntry = FirmwareEntry {
    id: "ibm-5170-bios-1985-11-15",
    sha256: "872febf7d7034bfadf72ed1936e06600eca72456b9ded41d15b7a284de20337d",
    size: 65536,
    kind: FirmwareKind::SystemBios,
    base: FirmwareBase::TopOfFirstMib,
    scan_window: Some((0xC8000, 0xE0000)),
    video_scan_window: Some((0xC0000, 0xC8000)),
    rom_offset: 0,
    provides: &[FirmwareService::Floppy, FirmwareService::FixedDisk],
    capabilities: &[FirmwareCapability::CmosDriveTypes],
    requires: &[
        FirmwarePart::Kbc8042,
        FirmwarePart::RtcCmos,
        FirmwarePart::PicCascade,
    ],
    cpu_min: FirmwareCpu::I80286,
    boards: &["ibm-5170"],
    distribution: FirmwareDistribution::UserSupplied,
    status: FirmwareStatus::Listed,
    parts: &[&[
        KnownPart {
            label: "U27",
            part_number: "61X9266",
            lane: crate::Lane::Even,
            offset: 0,
            size: 32768,
            sha256: "3b921a451d78efcd91232a01cd6936d6ba12dad4374466655aff8ef3a5dc764e",
        },
        KnownPart {
            label: "U47",
            part_number: "61X9265",
            lane: crate::Lane::Odd,
            offset: 0,
            size: 32768,
            sha256: "aec1759a8245d50be93c9b081ab1ac7de58dd8141f603c7745ae66d2d4c29e4d",
        },
    ]],
};

/// BIOS-socket AT diagnostic, identified without distributing firmware bytes.
/// Landmark Research International, ROM POST User's Manual v1.2, pp. 16–36:
/// tests AT hardware directly and supplies no boot services. MAME at.cpp,
/// ibm5170 landmark set: U27 even and U47 odd are 27256 EPROMs interleaved
/// at F0000h. See `backlog/docs/design/compatibility-diagnostics.md` §2–3.
pub const LANDMARK_ROM_POST_AT_1_2_0: FirmwareEntry = FirmwareEntry {
    id: "landmark-rom-post-at-1.2.0",
    kind: FirmwareKind::SystemBios,
    sha256: "a8310f83de0f3e6b71e4a3ff82a01ffe3c3b5a957677aed2fad17b96bce2e577",
    size: 65536,
    base: FirmwareBase::TopOfFirstMib,
    scan_window: None,
    video_scan_window: None,
    rom_offset: 0,
    provides: &[],
    capabilities: &[
        FirmwareCapability::DirectHardwareTest(FirmwareService::Floppy),
        FirmwareCapability::DirectHardwareTest(FirmwareService::FixedDisk),
    ],
    requires: &[
        FirmwarePart::Kbc8042,
        FirmwarePart::RtcCmos,
        FirmwarePart::PicCascade,
    ],
    cpu_min: FirmwareCpu::I80286,
    boards: &["ibm-5170"],
    distribution: FirmwareDistribution::UserSupplied,
    parts: &[&[
        KnownPart {
            label: "U27",
            part_number: "27256",
            lane: crate::Lane::Even,
            offset: 0,
            size: 32768,
            sha256: "3677eb9f1b8c39daef82a99337649f43633a46db8726bc6a3805076ba3b25916",
        },
        KnownPart {
            label: "U47",
            part_number: "27256",
            lane: crate::Lane::Odd,
            offset: 0,
            size: 32768,
            sha256: "6a74a12bc1cda8c67c0526e061c62b8d7e1b622afdf3e9442950355571580313",
        },
    ]],
    status: FirmwareStatus::Listed,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{assemble, assemble_with_options, AssemblyError, ImageSource};

    const KNOWN: &str = "kiselev-xi8088-1.0.2 (open, vendor/kiselev-bios/fetch.sh), kiselev-xi8088-1.0.2-atreset (open, vendor/kiselev-bios/fetch.sh), ibm-5170-bios-1984-01-10 (user-supplied: U27 6181028 even, U47 6181029 odd or U17 6181024 even@0, U37 6181026 odd@0, U27 6181025 even@32768, U47 6181027 odd@32768; see vendor/user-roms/README.md), ibm-5170-bios-1985-06-10 (user-supplied: U27 6480090 even, U47 6480091 odd; see vendor/user-roms/README.md), ibm-5170-bios-1985-11-15 (user-supplied: U27 61X9266 even, U47 61X9265 odd; see vendor/user-roms/README.md), landmark-rom-post-at-1.2.0 (user-supplied: U27 27256 even, U47 27256 odd; see vendor/user-roms/README.md)";

    fn config() -> MachineConfig {
        crate::bios(vec![0; 32768], Some(vec![0; 1_474_560]))
    }

    fn synthetic() -> FirmwareEntry {
        FirmwareEntry {
            sha256: "c35020473aed1b4642cd726cad727b63fff2824ad68cedd7ffb73c7cbd890479",
            ..KISELEV_XI8088
        }
    }

    fn with_misplaced_bios_and_option_rom() -> MachineConfig {
        let mut config = config();
        config.images[0].base = 0xC0000;
        config.images.push(LoadImage {
            base: 0xD0000,
            source: ImageSource::Inline(vec![0; 2048]),
            kind: ImageKind::Rom,
            provides: Vec::new(),
        });
        config
    }

    #[test]
    fn production_catalogues_have_no_conflicting_cmos_capabilities() {
        for board in [BoardModel::Ibm5170, BoardModel::Bare] {
            for entry in board.firmware_catalogue() {
                check_cmos_capabilities(entry).unwrap_or_else(|error| {
                    panic!("{} catalogue: {error}", board.id());
                });
            }
        }
    }

    #[test]
    fn landmark_catalogue_membership_preserves_its_absent_scan_window() {
        let bytes = vec![0; 65536];
        let mut config = config();
        config.images[0].base = 0xF0000;
        config.images[0].source = ImageSource::Inline(bytes.clone());
        let diagnostic = FirmwareEntry {
            sha256: Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str()),
            ..LANDMARK_ROM_POST_AT_1_2_0
        };
        let listed = [KISELEV_XI8088, diagnostic];
        let selected = scan_firmware_from(&config, &listed).unwrap();
        assert_eq!(selected.id, diagnostic.id);
        assert_eq!(selected.scan_window, None);
        let unlisted = [KISELEV_XI8088];
        let fallback = scan_firmware_from(&config, &unlisted).unwrap();
        assert_eq!(fallback.id, KISELEV_XI8088.id);
        assert_eq!(fallback.scan_window, Some((0xC8000, 0xF8000)));
    }

    #[test]
    fn landmark_at_catalogue_exact_identity_and_layout() {
        let entry = BoardModel::Ibm5170
            .firmware_catalogue()
            .iter()
            .find(|entry| entry.id == "landmark-rom-post-at-1.2.0")
            .expect("Landmark AT diagnostic must be catalogued");
        assert_eq!(
            *entry,
            FirmwareEntry {
                id: "landmark-rom-post-at-1.2.0",
                kind: FirmwareKind::SystemBios,
                sha256: "a8310f83de0f3e6b71e4a3ff82a01ffe3c3b5a957677aed2fad17b96bce2e577",
                size: 65536,
                base: FirmwareBase::TopOfFirstMib,
                scan_window: None,
                video_scan_window: None,
                rom_offset: 0,
                provides: &[],
                capabilities: &[
                    FirmwareCapability::DirectHardwareTest(FirmwareService::Floppy),
                    FirmwareCapability::DirectHardwareTest(FirmwareService::FixedDisk),
                ],
                requires: &[
                    FirmwarePart::Kbc8042,
                    FirmwarePart::RtcCmos,
                    FirmwarePart::PicCascade,
                ],
                cpu_min: FirmwareCpu::I80286,
                boards: &["ibm-5170"],
                distribution: FirmwareDistribution::UserSupplied,
                parts: &[&[
                    KnownPart {
                        label: "U27",
                        part_number: "27256",
                        lane: crate::Lane::Even,
                        offset: 0,
                        size: 32768,
                        sha256: "3677eb9f1b8c39daef82a99337649f43633a46db8726bc6a3805076ba3b25916",
                    },
                    KnownPart {
                        label: "U47",
                        part_number: "27256",
                        lane: crate::Lane::Odd,
                        offset: 0,
                        size: 32768,
                        sha256: "6a74a12bc1cda8c67c0526e061c62b8d7e1b622afdf3e9442950355571580313",
                    },
                ]],
                status: FirmwareStatus::Listed,
            }
        );
    }

    #[test]
    fn bios_selection_prefers_top_placement_over_a_declaration() {
        let mut config = with_misplaced_bios_and_option_rom();
        config.images[1].base = 0xF8000;
        config.images[1].source = ImageSource::Inline(vec![0; 32768]);
        config.system_bios_image = Some(0);
        assert!(std::ptr::eq(
            system_bios(&config).unwrap(),
            &config.images[1]
        ));
        config.images.reverse();
        config.system_bios_image = Some(1);
        assert!(std::ptr::eq(
            system_bios(&config).unwrap(),
            &config.images[0]
        ));
    }

    #[test]
    fn bios_selection_uses_a_declared_misplaced_image_without_guessing_by_address() {
        for reverse in [false, true] {
            let mut config = with_misplaced_bios_and_option_rom();
            if reverse {
                config.images.reverse();
            }
            config.system_bios_image = Some(usize::from(reverse));
            for allow in [false, true] {
                let error = assembly_error(&config, allow);
                assert_eq!(
                    error,
                    ConfigError::BiosPlacement {
                        base: 0xC0000,
                        size: 32768
                    }
                );
                assert_eq!(error.to_string(), "system BIOS of 32 KiB at 0xc0000 must have a positive size that is a multiple of 8 KiB and end at 0x100000");
            }
        }
    }

    #[test]
    fn bios_selection_reports_placement_for_a_sole_undeclared_rom() {
        let mut config = config();
        config.system_bios_image = None;
        config.images[0].source = ImageSource::Inline(vec![0; 32767]);
        config.images.insert(
            0,
            LoadImage {
                base: 0x500,
                source: ImageSource::Inline(vec![0]),
                kind: ImageKind::RamLoad,
                provides: Vec::new(),
            },
        );
        for allow in [false, true] {
            assert_eq!(
                assembly_error(&config, allow),
                ConfigError::BiosPlacement {
                    base: 0xF8000,
                    size: 32767
                }
            );
        }
    }

    #[test]
    fn bios_selection_reports_missing_when_there_are_no_roms() {
        let mut config = config();
        config.images[0].kind = ImageKind::RamLoad;
        for marker in [None, Some(0), Some(99)] {
            config.system_bios_image = marker;
            for allow in [false, true] {
                assert_eq!(
                    assembly_error(&config, allow),
                    ConfigError::MissingSystemBios { board: "ibm-5170" }
                );
            }
        }
    }

    #[test]
    fn bios_selection_reports_missing_for_multiple_undeclared_misplaced_roms() {
        let mut config = with_misplaced_bios_and_option_rom();
        for marker in [None, Some(99)] {
            config.system_bios_image = marker;
            for allow in [false, true] {
                assert_eq!(
                    assembly_error(&config, allow),
                    ConfigError::MissingSystemBios { board: "ibm-5170" }
                );
            }
        }
    }

    fn video_rom() -> Vec<u8> {
        let mut bytes = vec![0; 24 * 1024];
        bytes[..4].copy_from_slice(&[0x55, 0xAA, 0x30, 0xCB]);
        let sum = bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        bytes[24 * 1024 - 1] = 0u8.wrapping_sub(sum);
        bytes
    }

    #[test]
    fn video_window_accepts_ibm_at_rom_and_rejects_absent_scan_or_overrun() {
        let mut config = config();
        config.images.push(LoadImage {
            base: 0xC0000,
            source: ImageSource::Inline(video_rom()),
            kind: ImageKind::Rom,
            provides: Vec::new(),
        });
        let ibm = FirmwareEntry {
            video_scan_window: IBM_5170_1985_06_10.video_scan_window,
            ..synthetic()
        };
        validate_option_roms_with_catalogue(&config, &[ibm]).unwrap();
        let no_video = FirmwareEntry {
            video_scan_window: None,
            ..ibm
        };
        assert!(matches!(
            validate_option_roms_with_catalogue(&config, &[no_video]),
            Err(ConfigError::FirmwareDoesNotScanVideoRoms {
                firmware: "kiselev-xi8088-1.0.2",
                ..
            })
        ));
        config.images[1].base = 0xC2800;
        assert!(matches!(
            validate_option_roms_with_catalogue(&config, &[ibm]),
            Err(ConfigError::OptionRomOutsideScanWindow {
                start: 0xC0000,
                end: 0xC8000,
                ..
            })
        ));
        config.images[1].base = 0xC8000;
        validate_option_roms_with_catalogue(&config, &[ibm]).unwrap();
        config.images[1].base = 0xF8000;
        assert!(matches!(
            validate_option_roms_with_catalogue(&config, &[ibm]),
            Err(ConfigError::OptionRomOutsideScanWindow {
                start: 0xC8000,
                end: 0xF8000,
                ..
            })
        ));
    }

    #[test]
    fn unlisted_system_bios_reports_no_known_video_scan() {
        let mut config = config();
        config.images.push(LoadImage {
            base: 0xC0000,
            source: ImageSource::Inline(video_rom()),
            kind: ImageKind::Rom,
            provides: Vec::new(),
        });
        assert!(matches!(
            validate_option_roms_with_catalogue(&config, &[KISELEV_XI8088]),
            Err(ConfigError::FirmwareDoesNotScanVideoRoms {
                firmware: "unlisted",
                ..
            })
        ));
        config.images[0].source = ImageSource::File("system.bin".into());
        validate_option_roms_with_catalogue(&config, &[KISELEV_XI8088]).unwrap();
    }

    #[test]
    fn video_container_keeps_file_identity_and_validates_only_mapped_bytes() {
        let mut bytes = vec![0; 8192];
        bytes.extend(video_rom());
        let video = FirmwareEntry {
            sha256: Box::leak(format!("{:x}", Sha256::digest(&bytes)).into_boxed_str()),
            requires: &[],
            ..FirmwareEntry::IBM_VGA_1987
        };
        let mut config = config();
        config.images.push(LoadImage {
            base: 0xC0000,
            source: rom_source(bytes.clone(), ImageKind::Rom, false, &[video]),
            kind: ImageKind::Rom,
            provides: Vec::new(),
        });
        assert_eq!(config.images[1].source.known_len(), Some(24576));
        assert_eq!(matched_entry(&config.images[1], &[video]), Some(&video));
        validate_option_roms_with_catalogue(&config, &[synthetic(), video]).unwrap();
        config.images[1].source = ImageSource::Inline(bytes);
        assert!(matches!(
            validate_option_roms_with_catalogue(&config, &[synthetic(), video]),
            Err(ConfigError::OptionRomBadHeader { .. })
        ));
    }

    #[test]
    fn system_bios_container_entry_never_applies_an_adapter_offset() {
        let bytes = vec![0; 32768];
        let entry = FirmwareEntry {
            rom_offset: 8192,
            ..synthetic()
        };
        assert!(matches!(
            rom_source(bytes.clone(), ImageKind::Rom, false, &[entry]),
            ImageSource::Inline(_)
        ));
        let video = FirmwareEntry {
            sha256: entry.sha256,
            ..FirmwareEntry::IBM_VGA_1987
        };
        assert!(matches!(
            rom_source(bytes, ImageKind::Rom, true, &[video]),
            ImageSource::Inline(_)
        ));
    }

    #[test]
    fn video_bios_cannot_be_used_as_the_system_bios_even_with_the_escape() {
        let bytes = vec![0; 32768];
        let video = FirmwareEntry {
            sha256: synthetic().sha256,
            ..FirmwareEntry::IBM_VGA_1987
        };
        for allow_unlisted_firmware in [false, true] {
            let error = check_system_bios(
                &config(),
                BoardModel::Ibm5170,
                &[video],
                0xF8000,
                &bytes,
                AssemblyOptions {
                    allow_unlisted_firmware,
                },
            )
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("video BIOS {} cannot be used as the system BIOS", video.id)
            );
        }
    }

    #[test]
    fn missing_video_scan_names_the_actual_bios_or_unlisted() {
        let mut config = config();
        config.images.push(LoadImage {
            base: 0xC0000,
            source: ImageSource::Inline(video_rom()),
            kind: ImageKind::Rom,
            provides: vec![],
        });
        let no_video = FirmwareEntry {
            video_scan_window: None,
            ..synthetic()
        };
        for (catalogue, name) in [
            (vec![no_video], no_video.id),
            (vec![KISELEV_XI8088], "unlisted"),
        ] {
            let error = validate_option_roms_with_catalogue(&config, &catalogue).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("system BIOS {name} has no known video-ROM scan at C0000h-C7FFFh")
            );
        }
    }

    #[test]
    fn listed_video_and_extension_rom_kinds_use_their_own_windows() {
        let bytes = video_rom();
        let video = FirmwareEntry {
            sha256: Box::leak(format!("{:x}", Sha256::digest(&bytes)).into_boxed_str()),
            size: bytes.len() as u64,
            rom_offset: 0,
            requires: &[],
            ..FirmwareEntry::IBM_VGA_1987
        };
        let mut config = config();
        config.images.push(LoadImage {
            base: 0xC8000,
            source: ImageSource::Inline(bytes),
            kind: ImageKind::Rom,
            provides: vec![],
        });
        let error =
            validate_option_roms_with_catalogue(&config, &[synthetic(), video]).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::OptionRomOutsideScanWindow {
                start: 0xC0000,
                end: 0xC8000,
                ..
            }
        ));
        config.images[1].base = 0xC0000;
        validate_option_roms_with_catalogue(&config, &[synthetic(), video]).unwrap();
        let extension = FirmwareEntry {
            kind: FirmwareKind::OptionRom,
            ..video
        };
        let error =
            validate_option_roms_with_catalogue(&config, &[synthetic(), extension]).unwrap_err();
        assert!(error
            .to_string()
            .contains("is an extension option ROM and cannot occupy the video-ROM window"));
    }

    #[test]
    fn ibm_video_catalogue_is_container_only_and_requires_register_vga() {
        let entry = FirmwareEntry::IBM_VGA_1987;
        assert_eq!(entry.rom_offset, 0x2000);
        assert_eq!(entry.size, 32768);
        assert_eq!(entry.kind, FirmwareKind::VideoBios);
        assert_eq!(
            entry.sha256,
            "51b49761f72780a696c36f3d2e74a8168c1c99b865070f7f90b8e621a8b5ae17"
        );
        assert_eq!(entry.distribution, FirmwareDistribution::UserSupplied);
        assert_eq!(entry.status, FirmwareStatus::Tested);
        assert!(entry.parts.is_empty());
        assert!(matches!(
            check_entry_requirements(&config(), BoardModel::Ibm5170, &entry, Some(1)),
            Err(ConfigError::FirmwareRequiresDevice {
                part: FirmwarePart::VgaRegisters,
                ..
            })
        ));
        assert!(BoardModel::Ibm5170.firmware_catalogue().contains(&entry));
    }

    #[test]
    fn open_vga_catalogue_entry_matches_recipe_and_fixture() {
        let entry = FirmwareEntry::VGABIOS_0_8A_UNEMU_1_STD;
        assert_eq!(entry.id, "vgabios-0.8a-unemu.1-std");
        assert_eq!(entry.kind, FirmwareKind::VideoBios);
        assert_eq!(entry.size, 32768);
        assert_eq!(entry.base, FirmwareBase::Suggested(0xC0000));
        assert_eq!(entry.rom_offset, 0);
        assert!(entry.provides.is_empty());
        assert_eq!(entry.requires, &[FirmwarePart::VgaRegisters]);
        assert_eq!(entry.cpu_min, FirmwareCpu::I80286);
        assert_eq!(entry.boards, &["ibm-5170"]);
        assert_eq!(entry.status, FirmwareStatus::Tested);
        assert!(entry.parts.is_empty());
        let FirmwareDistribution::Open {
            fetch,
            license,
            provenance,
            ..
        } = entry.distribution
        else {
            panic!("the open VGA BIOS must be fetchable");
        };
        assert_eq!(fetch, "vendor/vgabios/fetch.sh");
        assert_eq!(license, "LGPL-2.1-or-later");
        let catalogue = BoardModel::Ibm5170.firmware_catalogue();
        assert!(catalogue.contains(&entry));
        let script = include_str!("../../../vendor/vgabios/fetch.sh");
        let pin = |prefix: &str| {
            script
                .lines()
                .find_map(|line| line.strip_prefix(prefix)?.strip_suffix('"'))
                .expect("fixture recipe pins its release")
        };
        assert_eq!(entry.sha256, pin("ROM_SHA256=\""));
        assert!(provenance.contains(pin("TAG=\"")));
        assert!(provenance.contains(pin("REVISION=\"")));

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/vgabios/vgabios-std.bin");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                assert!(
                    std::env::var_os("UNEMUPC98_REQUIRE_FIXTURES").as_deref()
                        != Some(std::ffi::OsStr::new("1")),
                    "missing fixture {}; run vendor/vgabios/fetch.sh",
                    path.display()
                );
                eprintln!("skipping: missing fixture {}", path.display());
                return;
            }
            Err(error) => panic!("cannot read {}: {error}", path.display()),
        };
        assert_eq!(bytes.len() as u64, entry.size);
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), entry.sha256);
        assert_eq!(&bytes[..3], &[0x55, 0xAA, 0x40]);
        assert_eq!(
            bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
            0
        );
        // A raw image maps every byte, unlike the IBM EPROM container.
        let source = rom_source(bytes, ImageKind::Rom, false, catalogue);
        assert!(matches!(source, ImageSource::Inline(_)));
        let mut config = config();
        config.images.push(LoadImage {
            base: 0xC0000,
            source,
            kind: ImageKind::Rom,
            provides: Vec::new(),
        });
        assert_eq!(matched_entry(&config.images[1], catalogue), Some(&entry));
        let listed = [synthetic(), entry];
        assert!(matches!(
            validate_option_roms_with_catalogue(&config, &listed),
            Err(ConfigError::FirmwareRequiresDevice {
                part: FirmwarePart::VgaRegisters,
                ..
            })
        ));
        for device in &mut config.devices {
            if matches!(device.kind, DeviceKind::Vga(_)) {
                device.kind = DeviceKind::VgaRegisters(crate::VgaCfg {});
            }
        }
        validate_option_roms_with_catalogue(&config, &listed).unwrap();
        // 32 KiB fills the video window, so no other start address fits.
        config.images[1].base = 0xC0800;
        assert!(matches!(
            validate_option_roms_with_catalogue(&config, &listed),
            Err(ConfigError::OptionRomOutsideScanWindow {
                start: 0xC0000,
                end: 0xC8000,
                ..
            })
        ));
    }

    #[test]
    fn firmware_without_a_scan_window_refuses_any_option_rom() {
        let mut config = config();
        let entry = FirmwareEntry {
            scan_window: None,
            video_scan_window: None,
            rom_offset: 0,
            ..synthetic()
        };
        assert_eq!(
            validate_option_roms_with_catalogue(&config, &[entry]),
            Ok(())
        );
        config.images.push(LoadImage {
            base: 0xC8000,
            source: ImageSource::Inline(vec![0; 2048]),
            kind: ImageKind::Rom,
            provides: Vec::new(),
        });
        let error = validate_option_roms_with_catalogue(&config, &[entry]).unwrap_err();
        assert_eq!(
            error,
            ConfigError::FirmwareDoesNotScanOptionRoms {
                firmware: entry.id,
                image_index: 1
            }
        );
        assert_eq!(
            error.to_string(),
            "firmware kiselev-xi8088-1.0.2 does not scan for option ROMs"
        );
    }

    #[test]
    fn option_rom_catalogue_board_cpu_and_non_ata_requirements_are_mandatory() {
        let baseline = FirmwareEntry::XUB_R638_AT;
        for (entry, message) in [
            (FirmwareEntry { boards: &["ibm-5160"], ..baseline }, "firmware \"xub-r638-at\" is built for ibm-5160, not ibm-5170"),
            (FirmwareEntry { cpu_min: FirmwareCpu::I80386, ..baseline }, "firmware \"xub-r638-at\" needs an 80386 or later; cpu is 80286"),
            (FirmwareEntry { requires: &[FirmwarePart::VideoRom], ..baseline }, "firmware \"xub-r638-at\" requires a video-bios option ROM at 0xC0000; none is configured"),
        ] {
            assert_eq!(check_entry_requirements(&config(), BoardModel::Ibm5170, &entry, Some(1)).unwrap_err().to_string(), message);
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
                source_archive_sha256: Some(XTIDE_SOURCE_ARCHIVE),
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
    fn open_entries_pin_the_source_archive_their_fetch_script_builds() {
        let pin = |script: &str| {
            script
                .lines()
                .find_map(|line| {
                    line.strip_prefix("SOURCE_ARCHIVE_SHA256=\"")?
                        .strip_suffix('"')
                })
                .expect("fixture recipe pins its source archive")
                .to_owned()
        };
        let scripts = [
            (
                "vendor/kiselev-bios/fetch.sh",
                pin(include_str!("../../../vendor/kiselev-bios/fetch.sh")),
            ),
            (
                "vendor/vgabios/fetch.sh",
                pin(include_str!("../../../vendor/vgabios/fetch.sh")),
            ),
            (
                "vendor/xtide-universal-bios/fetch.sh",
                pin(include_str!(
                    "../../../vendor/xtide-universal-bios/fetch.sh"
                )),
            ),
        ];
        let mut open = 0;
        for entry in BoardModel::Ibm5170.firmware_catalogue() {
            let FirmwareDistribution::Open {
                fetch,
                source_archive_sha256,
                ..
            } = entry.distribution
            else {
                continue;
            };
            open += 1;
            let (_, expected) = scripts
                .iter()
                .find(|(script, _)| *script == fetch)
                .unwrap_or_else(|| panic!("{}: unknown fetch script {fetch}", entry.id));
            assert_eq!(
                source_archive_sha256,
                Some(expected.as_str()),
                "{}",
                entry.id
            );
        }
        assert_eq!(open, 5);
    }

    #[test]
    fn configured_xtp_catalogue_entry_matches_recipe_and_fixture() {
        let entry = FirmwareEntry::XUB_R638_XTP_ATA1F0;
        assert_eq!(entry.id, "xub-r638-xtp-ata1f0");
        assert_eq!(entry.kind, FirmwareKind::OptionRom);
        assert_eq!(entry.size, 8192);
        assert_eq!(entry.base, FirmwareBase::Suggested(0xC8000));
        assert_eq!(entry.cpu_min, FirmwareCpu::I80186);
        assert_eq!(entry.provides, &[FirmwareService::FixedDisk]);
        assert_eq!(entry.requires, &[FirmwarePart::AtaPrimary]);
        assert_eq!(entry.boards, &["ibm-5170"]);
        assert_eq!(entry.status, FirmwareStatus::Tested);
        assert!(BoardModel::Ibm5170.firmware_catalogue().contains(&entry));
        let script = include_str!("../../../vendor/xtide-universal-bios/fetch.sh");
        let hash = script
            .lines()
            .find_map(|line| line.strip_prefix("XTP_OUTPUT_SHA256=\"")?.strip_suffix('"'))
            .expect("fixture recipe pins its configured XT+ output");
        assert_eq!(entry.sha256, hash);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                assert!(
                    std::env::var_os("UNEMUPC98_REQUIRE_FIXTURES").as_deref()
                        != Some(std::ffi::OsStr::new("1")),
                    "missing fixture {}; run vendor/xtide-universal-bios/fetch.sh",
                    path.display()
                );
                eprintln!("skipping: missing fixture {}", path.display());
                return;
            }
            Err(error) => panic!("cannot read {}: {error}", path.display()),
        };
        assert_eq!(bytes.len() as u64, entry.size);
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), entry.sha256);
        assert_eq!(&bytes[..3], &[0x55, 0xAA, 16]);
        assert_eq!(bytes[0x49], 0, "boot from floppy, not the data-only HDD");
        assert_eq!(&bytes[0x4E..0x54], &[0xF0, 1, 0xF0, 3, 0, 0]);
        assert_eq!(bytes[0x40] & 1, 0, "preserve upstream lite mode");
        assert_eq!(bytes[0x41] & 0x10, 0, "XT+ has no IRQ module");
        assert!(bytes[6629..8191].iter().all(|&byte| byte == 0));
        assert_eq!(bytes[8191], 0x77);
        assert_eq!(
            bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
            0
        );
    }

    #[test]
    fn atreset_catalogue_entry_matches_recipe_and_fixture() {
        let entry = KISELEV_XI8088_ATRESET;
        assert_eq!(entry.id, "kiselev-xi8088-1.0.2-atreset");
        assert!(BoardModel::Ibm5170.firmware_catalogue().contains(&entry));
        assert_eq!(entry.kind, KISELEV_XI8088.kind);
        assert_eq!(entry.size, KISELEV_XI8088.size);
        assert_eq!(entry.base, KISELEV_XI8088.base);
        assert_eq!(entry.scan_window, KISELEV_XI8088.scan_window);
        assert_eq!(entry.provides, KISELEV_XI8088.provides);
        assert_eq!(entry.capabilities, KISELEV_XI8088.capabilities);
        assert_eq!(entry.requires, KISELEV_XI8088.requires);
        assert_eq!(entry.cpu_min, KISELEV_XI8088.cpu_min);
        assert_eq!(entry.boards, KISELEV_XI8088.boards);
        let script = include_str!("../../../vendor/kiselev-bios/fetch.sh");
        let hash = script
            .lines()
            .find_map(|line| {
                line.strip_prefix("ATRESET_OUTPUT_SHA256=\"")?
                    .strip_suffix('"')
            })
            .expect("fixture recipe pins its patched BIOS output");
        assert_eq!(entry.sha256, hash);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/kiselev-bios/bios-xi8088-atreset.bin");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                assert!(
                    std::env::var_os("UNEMUPC98_REQUIRE_FIXTURES").as_deref()
                        != Some(std::ffi::OsStr::new("1")),
                    "missing fixture {}; run vendor/kiselev-bios/fetch.sh",
                    path.display()
                );
                eprintln!("skipping: missing fixture {}", path.display());
                return;
            }
            Err(error) => panic!("cannot read {}: {error}", path.display()),
        };
        assert_eq!(bytes.len() as u64, entry.size);
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), entry.sha256);
        check_entry_requirements(&config(), BoardModel::Ibm5170, &entry, None).unwrap();
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
    fn primary_ata_requirement_is_unmet_without_an_ide_channel() {
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
                FirmwarePart::VideoRom
                | FirmwarePart::AtaPrimary
                | FirmwarePart::RtcCmos128
                | FirmwarePart::VgaRegisters => {
                    unreachable!()
                }
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
    #[test]
    fn catalogue_services_reject_forged_fixed_disk_claims() {
        let Some(bytes) = fixture() else {
            return;
        };
        let mut config = config();
        config.images[0].source = ImageSource::Inline(bytes);
        config.images[0].provides.push(FirmwareService::FixedDisk);
        let cfg = crate::IdeCfg {
            model: crate::AtaControllerModel::Ata,
            base: 0x1F0,
            control: 0x3F6,
        };
        config.devices.push(
            crate::DeviceConfig::new(
                crate::DeviceId::Ide,
                DeviceKind::Ide(cfg),
                cfg.port_windows(),
            )
            .with_irq(14),
        );
        config
            .devices
            .iter_mut()
            .find(|device| device.id == crate::DeviceId::Fdc)
            .unwrap()
            .ports = crate::FdcCfg::port_windows(true);
        let error = match assemble(&config) {
            Err(AssemblyError::Config(error)) => error,
            _ => panic!("forged services must fail"),
        };
        assert_eq!(error.to_string(), "firmware \"kiselev-xi8088-1.0.2\" declares provides [\"fixed-disk\", \"floppy\"]; catalogue provides [\"floppy\"]");
        assert!(matches!(
            error,
            ConfigError::FirmwareProvidesMismatch { image_index: 0, .. }
        ));
        assert!(matches!(
            assemble_with_options(
                &config,
                AssemblyOptions {
                    allow_unlisted_firmware: true
                }
            ),
            Err(AssemblyError::Config(
                ConfigError::FirmwareProvidesMismatch { .. }
            ))
        ));
    }

    #[test]
    fn catalogue_services_supply_omitted_or_equal_declarations() {
        let Some(bytes) = fixture() else {
            return;
        };
        let mut config = config();
        for source in [
            ImageSource::Inline(bytes),
            ImageSource::File(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../vendor/kiselev-bios/bios-xi8088.bin"),
            ),
        ] {
            config.images[0].source = source;
            for declared in [Vec::new(), vec![FirmwareService::Floppy]] {
                config.images[0].provides = declared;
                assert_eq!(config.validate(), Ok(()));
                assert!(assemble(&config).unwrap().unlisted_firmware.is_empty());
            }
        }
    }
    #[test]
    fn primary_ata_requirement_matches_the_assembled_channel_contract() {
        let cfg = crate::IdeCfg {
            model: crate::AtaControllerModel::Ata,
            base: 0x1F0,
            control: 0x3F6,
        };
        let device = crate::DeviceConfig::new(
            crate::DeviceId::Ide,
            DeviceKind::Ide(cfg),
            cfg.port_windows(),
        )
        .with_irq(14);
        let mut config = config();
        config.devices.push(device.clone());
        assert!(FirmwarePart::AtaPrimary.present(&config));
        for malformed in 0..5 {
            let mut other = config.clone();
            let ide = other.devices.last_mut().unwrap();
            match malformed {
                0 => ide.irq = Some(15),
                1 => ide.ports.clear(),
                2 => {
                    let secondary = crate::IdeCfg {
                        model: crate::AtaControllerModel::Ata,
                        base: 0x170,
                        control: 0x376,
                    };
                    ide.kind = DeviceKind::Ide(secondary);
                    ide.ports = secondary.port_windows();
                    ide.irq = Some(15);
                }
                3 => {
                    // Hold control and IRQ fixed to check the base independently.
                    let mixed = crate::IdeCfg {
                        model: crate::AtaControllerModel::Ata,
                        base: 0x170,
                        control: 0x3F6,
                    };
                    ide.kind = DeviceKind::Ide(mixed);
                    ide.ports = mixed.port_windows();
                    ide.irq = Some(14);
                }
                _ => ide.kind = DeviceKind::Post(crate::PostPortCfg { port: 0x1F0 }),
            }
            assert!(!FirmwarePart::AtaPrimary.present(&other));
        }
    }
}
