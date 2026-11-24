// SPDX-License-Identifier: AGPL-3.0-or-later
//! New empty machine images built from chosen hardware; machine-images §§2, 4
//! and 5.3, File › New machine….
//!
//! Hardware rules stay with machine-file lowering and assembly validation; this
//! module only writes a schema-1 machine document, binds the chosen ROMs of
//! firmware packages already in the store (by digest, never copied) and gives
//! every fixed disk a zero base. The generated configuration is our own file
//! and therefore open; the image's redistribution class still follows its
//! firmware blobs. A ROM that is open under a licence obliging its source
//! travels with its package's source archive (the same blob), as in a machine
//! image import, so an all-open image can be published with its source. The IBM Personal Computer AT Technical Reference, Fixed Disk
//! and Diskette Drive Adapter, describes the controller and drive positions
//! the document names.

use crate::firmware::{Fit, PcFirmwareRules};
use crate::*;
use image_store::{
    firmware::{Config as PackageConfig, Rom, Validator},
    license, media_type as mt, BlobSource, ConfigValidator, Manifest, Store, MAX_SMALL_BLOB,
};
use std::collections::{BTreeMap, BTreeSet};
use toml::{Table, Value};

/// One class of the closed list of PC machine classes, in its documented order.
#[derive(Clone, Copy, Debug)]
pub struct MachineClass {
    /// Provider fit vocabulary, as in firmware package `fits.machine_classes`.
    pub id: &'static str,
    pub name: &'static str,
    pub cpu: &'static str,
    pub examples: &'static str,
    /// The project phase that brings the class.
    pub phase: &'static str,
    /// Executable board models of the class; empty until the class exists.
    pub boards: &'static [assembly::BoardModel],
}

/// The seven PC classes this emulator will model; nothing outside the list.
pub const MACHINE_CLASSES: &[MachineClass] = &[
    MachineClass {
        id: "xt",
        name: "PC/XT, 8-bit ISA",
        cpu: "8088, NEC V20",
        examples: "IBM 5150, 5160, Turbo XT clones (Xi8088)",
        phase: "later: XT work starts in phase A′",
        boards: &[],
    },
    MachineClass {
        id: "xt16",
        name: "16-bit XT",
        cpu: "8086, NEC V30",
        examples: "a generic 8086 XT clone",
        phase: "later: after the PC/XT class",
        boards: &[],
    },
    MachineClass {
        id: "tandy1000",
        name: "Tandy 1000 (SX/TX)",
        cpu: "8088/8086",
        examples: "Tandy 1000 graphics and SN76489 sound",
        phase: "later: the last of the 8-bit-bus classes",
        boards: &[],
    },
    MachineClass {
        id: "at",
        name: "AT 286",
        cpu: "80286",
        examples: "IBM 5170, 5162 (XT/286), AMI and Award clones",
        phase: "available: phases A and A′",
        boards: &[assembly::BoardModel::Ibm5170],
    },
    MachineClass {
        id: "isa386",
        name: "386/486 ISA",
        cpu: "386SX/DX, 486",
        examples: "generic ISA boards",
        phase: "phase B, a Windows 3.x milestone",
        boards: &[],
    },
    MachineClass {
        id: "pci-p5",
        name: "Pentium PCI",
        cpu: "Pentium, MMX",
        examples: "i430-class chipset",
        phase: "phase C",
        boards: &[],
    },
    MachineClass {
        id: "p2-voodoo",
        name: "Pentium II/III with Voodoo",
        cpu: "Pentium II/III",
        examples: "i440BX-class chipset, 3dfx Voodoo",
        phase: "phase D, the target",
        boards: &[],
    },
];

/// Class of an executable board, in fit vocabulary.
pub fn class_of(board: assembly::BoardModel) -> Option<&'static MachineClass> {
    MACHINE_CLASSES.iter().find(|c| c.boards.contains(&board))
}

/// A CPU speed choice: instruction throughput per PIT clock, not cycle timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Speed {
    pub mhz: u32,
    pub steps: u32,
    pub pit_clocks: u32,
    /// The board's calibrated rate; other entries scale it by clock ratio.
    pub calibrated: bool,
}

/// Speeds offered for a board: its nominal clocks, each the board's calibrated
/// default rate scaled by clock ratio (for the IBM 5170, 6 MHz is 8/13 and
/// 8 MHz 32/39 steps per PIT clock).
pub fn speeds(board: assembly::BoardModel) -> Vec<Speed> {
    let (calibrated, offered) = board.cpu_clocks_mhz();
    let clock = board.default_clock();
    offered
        .iter()
        .filter_map(|&mhz| {
            let steps = clock.retired_per_pit_clock * u64::from(mhz);
            let pit = clock.retired_per_pit_clock_denominator * u64::from(calibrated);
            let divisor = gcd(steps, pit);
            Some(Speed {
                mhz,
                steps: u32::try_from(steps / divisor).ok()?,
                pit_clocks: u32::try_from(pit / divisor).ok()?,
                calibrated: mhz == calibrated,
            })
        })
        .collect()
}
fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a.max(1)
    } else {
        gcd(b, a % b)
    }
}

/// Cylinders, heads and sectors per track of a blank fixed disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chs {
    pub cylinders: u32,
    pub heads: u32,
    pub sectors: u32,
}
impl Chs {
    pub const fn new(cylinders: u32, heads: u32, sectors: u32) -> Self {
        Self {
            cylinders,
            heads,
            sectors,
        }
    }
    pub fn bytes(self) -> u64 {
        u64::from(self.cylinders) * u64::from(self.heads) * u64::from(self.sectors) * 512
    }
}
/// Blank-disk sizes offered first. Any CHS geometry is possible with an option
/// ROM such as XTIDE; 1024/16/63 (504 MiB) is the largest INT 13h CHS disk.
pub const BLANK_DISKS: &[Chs] = &[
    Chs::new(615, 4, 17),
    Chs::new(940, 8, 17),
    Chs::new(256, 16, 63),
    Chs::new(512, 16, 63),
    Chs::new(1024, 16, 63),
];

/// The display adapter card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Video {
    Vga,
    VgaText,
    Mda,
}
impl Video {
    pub const ALL: [Self; 3] = [Self::Vga, Self::VgaText, Self::Mda];
    pub const fn card(self) -> &'static str {
        match self {
            Self::Vga => "vga",
            Self::VgaText => "vga-text",
            Self::Mda => "mda",
        }
    }
}
/// What hangs on the 16550 UART at COM1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Serial {
    Mouse,
    Empty,
    None,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sb16 {
    pub base: u16,
    pub irq: u8,
    pub dma8: u8,
    pub dma16: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ems {
    pub port: u16,
    pub frame: u16,
    pub size_kib: u32,
}

/// Chosen hardware, written as a schema-1 machine document. Values are not
/// checked here: lowering and the capabilities API own every rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hardware {
    pub board: assembly::BoardModel,
    /// Retired instructions per PIT clocks.
    pub rate: (u32, u32),
    pub fpu: bool,
    pub conventional_kib: u32,
    pub extended_kib: u32,
    pub video: Video,
    pub sound: Option<Sb16>,
    pub serial: Serial,
    pub ems: Option<Ems>,
    /// Diskette drive types for units 0 and 1 (`1.2M`, `1.44M`, …).
    pub floppies: [Option<String>; 2],
    /// Task-file base of the fixed-disk adapter: 1F0h primary, 170h secondary.
    pub ide_port: u16,
    /// Master and slave blank disks.
    pub disks: [Option<Chs>; 2],
}

impl Hardware {
    /// Hardware tables without firmware; `ata` selects plain ATA task-file
    /// semantics rather than the IBM fixed-disk adapter latches. The fixed-disk
    /// adapter is fitted when `adapter` is set or a disk is chosen: an adapter
    /// always needs a fixed-disk service, which a floppy-only BIOS lacks.
    pub fn table(&self, ata: bool, adapter: bool) -> Table {
        let mut root = Table::new();
        root.insert("schema".into(), 1.into());
        root.insert(
            "board".into(),
            Table::from_iter([("model".into(), self.board.id().into())]).into(),
        );
        root.insert(
            "cpu".into(),
            Table::from_iter([
                ("model".into(), "80286".into()),
                (
                    "rate".into(),
                    format!("{}/{}", self.rate.0, self.rate.1).into(),
                ),
                ("fpu".into(), if self.fpu { "80287" } else { "none" }.into()),
            ])
            .into(),
        );
        root.insert(
            "memory".into(),
            Table::from_iter([
                (
                    "conventional_kib".into(),
                    i64::from(self.conventional_kib).into(),
                ),
                ("extended_kib".into(), i64::from(self.extended_kib).into()),
            ])
            .into(),
        );
        let card = |kind: &str| Table::from_iter([("kind".into(), Value::from(kind))]);
        let mut cards = vec![card(self.video.card())];
        if let Some(sb) = self.sound {
            let mut t = card("sb16");
            t.insert("base".into(), i64::from(sb.base).into());
            t.insert("irq".into(), i64::from(sb.irq).into());
            t.insert("dma8".into(), i64::from(sb.dma8).into());
            t.insert("dma16".into(), i64::from(sb.dma16).into());
            cards.push(t);
        }
        if self.serial != Serial::None {
            let mut t = card("uart-16550");
            t.insert(
                "port".into(),
                i64::from(assembly::descriptors::UART_PORT.default).into(),
            );
            t.insert(
                "mouse".into(),
                if self.serial == Serial::Mouse {
                    "microsoft"
                } else {
                    "none"
                }
                .into(),
            );
            cards.push(t);
        }
        let mut fdc = card("fdc-765");
        let floppies: Vec<Value> = self
            .floppies
            .iter()
            .enumerate()
            .filter_map(|(unit, kind)| {
                kind.as_ref().map(|kind| {
                    Table::from_iter([
                        ("unit".into(), (unit as i64).into()),
                        ("type".into(), kind.clone().into()),
                    ])
                    .into()
                })
            })
            .collect();
        if !floppies.is_empty() {
            fdc.insert("drives".into(), floppies.into());
        }
        cards.push(fdc);
        let fitted = adapter || self.disks.iter().any(Option::is_some);
        let mut ide = card("ide");
        ide.insert(
            "controller_model".into(),
            if ata { "ata" } else { "at-adapter" }.into(),
        );
        ide.insert("port".into(), i64::from(self.ide_port).into());
        let disks: Vec<Value> = self
            .disks
            .iter()
            .enumerate()
            .filter_map(|(unit, chs)| {
                chs.map(|chs| {
                    Table::from_iter([
                        (
                            "channel".into(),
                            if unit == 0 { "master" } else { "slave" }.into(),
                        ),
                        (
                            "geometry".into(),
                            Table::from_iter([
                                ("cylinders".into(), i64::from(chs.cylinders).into()),
                                ("heads".into(), i64::from(chs.heads).into()),
                                ("sectors".into(), i64::from(chs.sectors).into()),
                            ])
                            .into(),
                        ),
                    ])
                    .into()
                })
            })
            .collect();
        if !disks.is_empty() {
            ide.insert("drives".into(), disks.into());
        }
        if fitted {
            cards.push(ide);
        }
        if let Some(ems) = self.ems {
            let mut t = card("lotech-ems");
            t.insert("port".into(), i64::from(ems.port).into());
            t.insert("frame".into(), i64::from(ems.frame).into());
            t.insert("size_kib".into(), i64::from(ems.size_kib).into());
            cards.push(t);
        }
        root.insert(
            "card".into(),
            Value::Array(cards.into_iter().map(Value::Table).collect()),
        );
        root
    }
}

/// A firmware package in the local store, read from its manifest and config only.
#[derive(Clone, Debug)]
pub struct Package {
    /// The first local tag that names the package.
    pub reference: String,
    pub descriptor: Descriptor,
    pub manifest: Manifest,
    pub config: PackageConfig,
    pub compatibility_levels: image_store::compat::Levels,
}
impl Package {
    /// Provider fit claims; malformed claims fit nothing.
    pub fn fit(&self) -> Fit {
        Fit::decode(&self.config.fits).unwrap_or_default()
    }
    /// Whether every blob of a ROM is open: marked open with a valid licence,
    /// and not bytes the firmware catalogue lists as proprietary.
    pub fn open(&self, rom: &Rom) -> bool {
        !locked(rom)
            && rom_blobs(rom)
                .iter()
                .all(|digest| self.layer(digest).and_then(license::open_license).is_some())
    }
    fn layer(&self, digest: &Digest) -> Option<&Descriptor> {
        self.manifest.layers.iter().find(|d| d.names(digest))
    }
    /// The package's source-archive layer a ROM travels with: the ROM is open
    /// under a licence that obliges its corresponding source, and the package
    /// links an archive for it.
    pub fn source_archive(&self, rom: &Rom) -> Option<&Descriptor> {
        let source = self.config.source_of(&rom.id)?;
        let copyleft = self.open(rom)
            && rom_blobs(rom).iter().all(|digest| {
                self.layer(digest)
                    .and_then(license::open_license)
                    .is_some_and(|l| l.needs_source(license::ACTIVE))
            });
        copyleft.then(|| self.layer(&source.blob)).flatten()
    }
}
/// Bytes the firmware catalogue lists as proprietary stay user-supplied,
/// whatever a package says about them.
fn locked(rom: &Rom) -> bool {
    let catalogue = crate::firmware::catalogue();
    crate::firmware::proprietary(catalogue, &rom.sha256, rom.size).is_some()
        || rom
            .parts
            .iter()
            .any(|p| crate::firmware::proprietary(catalogue, &p.sha256, p.size).is_some())
}
/// The layer descriptor of one blob (`digest`) of a package ROM as an image
/// binds it: marked user-supplied when the catalogue lists the bytes as
/// proprietary, and linked to the package's source archive only when that
/// archive travels with the ROM ([`Package::source_archive`]).
pub(crate) fn rom_blob(package: &Package, rom: &Rom, digest: &Digest) -> Result<Descriptor> {
    let mut d = package
        .manifest
        .layers
        .iter()
        .find(|d| d.names(digest) && d.decoded_media_type() == mt::FIRMWARE)
        .ok_or_else(|| invalid("firmware package lacks a ROM blob"))?
        .clone();
    if locked(rom) {
        license::mark(&mut d, None);
    }
    if package.source_archive(rom).is_none() {
        d.annotations.remove(license::SOURCE);
    }
    Ok(d)
}
pub(crate) fn rom_blobs(rom: &Rom) -> Vec<Digest> {
    rom.blob
        .iter()
        .cloned()
        .chain(rom.parts.iter().map(|p| p.blob.clone()))
        .collect()
}

/// PC firmware packages of the local store, without reading ROM bytes.
/// Unreadable or incompatible entries are skipped so only usable packages are offered.
pub fn packages(store: &Store) -> Result<Vec<Package>> {
    read_packages(store, true)
}

/// Revisions need newer packages too, so their inherited requirements cannot disappear.
pub(crate) fn packages_for_revision(store: &Store) -> Result<Vec<Package>> {
    read_packages(store, false)
}

fn read_packages(store: &Store, compatible_only: bool) -> Result<Vec<Package>> {
    // Standalone callers use the same PC levels as package creation does.
    let standalone;
    let store = if store.compatibility_configured() {
        store
    } else {
        standalone = store
            .clone()
            .with_compatibility(crate::compat::compatibility(
                image_store::compat::EXTRACTION_BASELINE,
            ));
        &standalone
    };
    let mut seen = BTreeSet::new();
    let mut found = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = store.list_tags(after.as_deref(), 1000)?;
        for tag in page.tags {
            if !seen.insert(tag.manifest.digest.clone()) {
                continue;
            }
            let package = (|| -> Result<Option<Package>> {
                let bytes = store.read_verified(&tag.manifest, MAX_SMALL_BLOB)?;
                if compatible_only {
                    store.compatibility().check(&bytes, &tag.name)?;
                }
                if Manifest::decode_for_summary(&bytes)?.artifact_type() != mt::FIRMWARE {
                    return Ok(None);
                }
                let manifest = Manifest::decode(&bytes)?;
                let config =
                    PackageConfig::decode(&store.read_verified(&manifest.config, MAX_SMALL_BLOB)?)?;
                Ok(crate::is_pc(&config.platform).then(|| Package {
                    compatibility_levels: crate::compat::source_levels(store),
                    reference: tag.name.clone(),
                    descriptor: tag.manifest.clone(),
                    manifest,
                    config,
                }))
            })();
            if let Ok(Some(package)) = package {
                found.push(package);
            }
        }
        match page.next_after {
            Some(next) => after = Some(next),
            None => return Ok(found),
        }
    }
}

/// One logical ROM of a stored firmware package.
#[derive(Clone, Debug)]
pub struct PackageRom {
    pub package: Package,
    pub rom: String,
}
impl PackageRom {
    pub fn new(package: &Package, rom: &Rom) -> Self {
        Self {
            package: package.clone(),
            rom: rom.id.clone(),
        }
    }
    pub fn rom(&self) -> Result<&Rom> {
        self.package
            .config
            .roms
            .iter()
            .find(|r| r.id == self.rom)
            .ok_or_else(|| invalid("firmware package has no such ROM"))
    }
    /// The reviewed catalogue entry the ROM names, if any.
    pub fn entry(&self) -> Option<&'static assembly::FirmwareEntry> {
        let id = self.rom().ok()?.catalogue_id.as_deref()?;
        crate::firmware::catalogue().iter().find(|e| e.id == id)
    }
    fn names(&self, index: usize) -> Result<Vec<(String, Digest)>> {
        let rom = self.rom()?;
        Ok(match &rom.blob {
            Some(blob) => vec![(format!("assets/firmware-{index}.bin"), blob.clone())],
            None => rom
                .parts
                .iter()
                .enumerate()
                .map(|(i, p)| (format!("assets/firmware-{index}-{i}.bin"), p.blob.clone()))
                .collect(),
        })
    }
    /// The machine-file ROM table, with logical asset names. Option ROMs take
    /// their placement and services from the catalogue entry they name.
    pub fn table(&self, index: usize) -> Result<Table> {
        let rom = self.rom()?;
        let names = self.names(index)?;
        let mut table = Table::new();
        if rom.parts.is_empty() {
            table.insert("image".into(), names[0].0.clone().into());
        } else {
            let parts: Vec<Value> = rom
                .parts
                .iter()
                .zip(&names)
                .map(|(part, (name, _))| {
                    Table::from_iter([
                        ("image".into(), Value::from(name.clone())),
                        ("lane".into(), part.lane.clone().into()),
                        (
                            "offset".into(),
                            i64::try_from(part.offset).unwrap_or(i64::MAX).into(),
                        ),
                        ("sha256".into(), part.sha256.clone().into()),
                    ])
                    .into()
                })
                .collect();
            table.insert("parts".into(), parts.into());
        }
        table.insert("sha256".into(), rom.sha256.clone().into());
        if rom.role != "system-bios" {
            if let Some(entry) = self.entry() {
                let binding = assembly::capabilities::firmware_binding(entry);
                table.insert(
                    "base".into(),
                    i64::try_from(binding.base).unwrap_or(i64::MAX).into(),
                );
                if binding.offset != 0 {
                    table.insert("offset".into(), (binding.offset as i64).into());
                }
                if !binding.provides.is_empty() {
                    table.insert(
                        "provides".into(),
                        binding
                            .provides
                            .iter()
                            .map(|s| Value::from(s.to_string()))
                            .collect::<Vec<_>>()
                            .into(),
                    );
                }
            }
        }
        Ok(table)
    }
}

/// Everything File › New machine… needs to build an empty image.
#[derive(Clone, Debug)]
pub struct NewImage {
    pub hardware: Hardware,
    /// The system BIOS first, then option ROMs (video BIOS, disk adapter ROM).
    pub firmware: Vec<PackageRom>,
    /// Library title; stored as the manifest title, never in the configuration.
    pub title: Option<String>,
    /// A one-paragraph summary; stored as the manifest description, never in
    /// the configuration.
    pub description: Option<String>,
}

impl NewImage {
    /// The complete machine document: hardware and firmware tables, drives
    /// with their positions and geometry only. Capabilities queries describe it.
    pub fn document(&self) -> Result<Table> {
        let adapter = self
            .firmware
            .iter()
            .skip(1)
            .any(|rom| rom.rom().is_ok_and(|r| r.role == "option-rom"));
        let mut root = self.hardware.table(self.ata()?, adapter);
        let mut firmware = Table::new();
        let mut options = Vec::new();
        for (index, rom) in self.firmware.iter().enumerate() {
            let table = rom.table(index)?;
            if index == 0 {
                if rom.rom()?.role != "system-bios" {
                    return Err(invalid("the first firmware binding must be a system BIOS"));
                }
                firmware.insert("bios".into(), table.into());
            } else {
                options.push(Value::Table(table));
            }
        }
        if firmware.is_empty() {
            firmware.insert("bios".into(), Table::new().into());
        }
        if !options.is_empty() {
            firmware.insert("option_roms".into(), options.into());
        }
        root.insert("firmware".into(), firmware.into());
        Ok(root)
    }
    /// Plain ATA semantics unless the system BIOS itself serves the fixed
    /// disks, as the IBM BIOS does through the IBM adapter's latches (see the
    /// shipped IBM and open AT profiles); an option ROM such as XTIDE needs ATA.
    fn ata(&self) -> Result<bool> {
        let serves = |rom: &PackageRom| {
            rom.entry()
                .is_some_and(|e| e.provides.contains(&assembly::FirmwareService::FixedDisk))
        };
        let option_serves = self.firmware.iter().skip(1).any(serves);
        Ok(option_serves || !self.firmware.first().is_some_and(serves))
    }
}

fn config_table(config: &Config) -> Result<Table> {
    toml::Value::try_from(config)
        .map_err(|e| invalid(e.to_string()))?
        .as_table()
        .cloned()
        .ok_or_else(|| invalid("config is not a table"))
}

/// The machine configuration and closure of a new image, before any write.
struct Plan {
    config: Config,
    descriptors: Vec<Descriptor>,
    /// Package manifests in first-use order, for provenance.
    sources: Vec<Digest>,
    /// Zero base descriptors, which exist only in memory until Create.
    zero: BTreeMap<Digest, Vec<u8>>,
}

fn plan(image: &NewImage) -> Result<Plan> {
    if image.firmware.is_empty() {
        return Err(invalid("a system BIOS is required"));
    }
    let mut machine = image.document()?;
    let mut descriptors = Vec::new();
    let mut names = BTreeMap::new();
    let mut archives = BTreeMap::new();
    let mut sources = Vec::new();
    for (index, rom) in image.firmware.iter().enumerate() {
        if !sources.contains(&rom.package.descriptor.digest) {
            sources.push(rom.package.descriptor.digest.clone());
        }
        let package_rom = rom.rom()?;
        let archive = rom.package.source_archive(package_rom).cloned();
        for (name, digest) in rom.names(index)? {
            descriptors.push(rom_blob(&rom.package, package_rom, &digest)?);
            if let Some(a) = &archive {
                archives.insert(name.clone(), a.decoded_digest()?);
            }
            names.insert(name, digest);
        }
        if let Some(a) = archive {
            if !descriptors.contains(&a) {
                descriptors.push(a);
            }
        }
    }
    let mut config = Config {
        schema: 1,
        platform: crate::PLATFORM.into(),
        machine: Table::new(),
        firmware: Vec::new(),
        media: Vec::new(),
        slots: Vec::new(),
        nvram: None,
    };
    firmware_images(&mut machine, |field, rom| {
        let name = rom
            .get("image")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("unnamed firmware image"))?;
        let blob = names
            .get(name)
            .ok_or_else(|| invalid("unbound firmware image"))?;
        config.firmware.push(Firmware {
            key: field.into(),
            source: name.into(),
            blob: blob.to_config()?,
            source_archive: archives.get(name).map(Digest::to_config).transpose()?,
        });
        Ok(())
    })?;
    let mut zero = BTreeMap::new();
    drive_tables(&mut machine, |controller, kind, drive| {
        let unit = unit(kind, drive)?;
        if kind != "ide" {
            config.slots.push(Slot {
                controller: controller.into(),
                unit,
                catalogue: Vec::new(),
                catalogue_mode: true,
                inserted: None,
            });
            return Ok(());
        }
        let g = drive
            .get("geometry")
            .and_then(Value::as_table)
            .ok_or_else(|| invalid("fixed disk without geometry"))?;
        let number = |key: &str| {
            g.get(key)
                .and_then(Value::as_integer)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| invalid("invalid fixed-disk geometry"))
        };
        let base =
            crate::zero::ZeroBase::new(number("cylinders")?, number("heads")?, number("sectors")?)?;
        let bytes = base.encode()?;
        // Disks of one geometry share one zero base descriptor.
        let d = generated(Descriptor::new(
            DISK_ZERO,
            Digest::of(&bytes),
            bytes.len() as u64,
        ));
        zero.insert(d.digest.clone(), bytes);
        let id = format!("medium-{}", config.media.len());
        config.media.push(Medium {
            id: id.clone(),
            source: format!("assets/{id}.img"),
            head: Some(d.digest.to_config()?),
            zero: None,
            name: None,
            policy: Policy::Journal,
            geometry: Geometry {
                cylinders: base.geometry.cylinders,
                heads: base.geometry.heads,
                sectors: base.geometry.sectors,
                sector_bytes: 512,
            },
        });
        config.slots.push(Slot {
            controller: controller.into(),
            unit,
            catalogue: vec![id.clone()],
            catalogue_mode: false,
            inserted: Some(id),
        });
        if !descriptors.contains(&d) {
            descriptors.push(d);
        }
        Ok(())
    })?;
    config.machine = machine;
    Ok(Plan {
        config,
        descriptors,
        sources,
        zero,
    })
}

/// Serves in-memory zero base descriptors before the store holds them.
struct Overlay<'a> {
    blobs: &'a dyn BlobSource,
    zero: &'a BTreeMap<Digest, Vec<u8>>,
}
impl BlobSource for Overlay<'_> {
    fn read_verified(&self, descriptor: &Descriptor, limit: u64) -> image_store::Result<Vec<u8>> {
        match self.zero.get(&descriptor.digest) {
            Some(bytes) if bytes.len() as u64 == descriptor.size && descriptor.size <= limit => {
                Ok(bytes.clone())
            }
            Some(_) => Err(image_store::Error::Invalid(
                "zero base descriptor size mismatch".into(),
            )),
            None => self.blobs.read_verified(descriptor, limit),
        }
    }
}

/// Lower with the real ROM bytes and check what only bytes decide, such as a
/// BIOS that types its fixed disks from CMOS: an untyped geometry fails here.
fn lowered(plan: &Plan, blobs: &dyn BlobSource) -> Result<crate::provider::Prepared> {
    let overlay = Overlay {
        blobs,
        zero: &plan.zero,
    };
    let prepared = crate::provider::prepare(
        decode(config_table(&plan.config)?)?,
        &plan.descriptors,
        &overlay,
    )?;
    assembly::derive_cmos_config(&prepared.machine).map_err(|e| invalid(e.to_string()))?;
    Ok(prepared)
}

/// The generated configuration and metadata of the image: open content.
fn finish(mut plan: Plan, blobs: &dyn BlobSource, image: &NewImage) -> Result<(Vec<u8>, Manifest)> {
    let prepared = lowered(&plan, blobs)?;
    // Freeze lowering defaults into the image identity, as import does.
    crate::defaults::materialize(&mut plan.config.machine, &prepared.machine)?;
    let overlay = Overlay {
        blobs,
        zero: &plan.zero,
    };
    let references =
        PcProvider.validate(&config_table(&plan.config)?, &plan.descriptors, &overlay)?;
    let mut seen = BTreeSet::new();
    let mut layers = Vec::new();
    for digest in references {
        if seen.insert(digest.clone()) {
            layers.push(
                plan.descriptors
                    .iter()
                    .find(|d| d.names(&digest))
                    .ok_or_else(|| invalid("missing closure object"))?
                    .clone(),
            );
        }
    }
    let bytes = toml::to_string(&plan.config)
        .map_err(|e| invalid(e.to_string()))?
        .into_bytes();
    let configuration = generated(Descriptor::new(
        mt::CONFIG,
        Digest::of(&bytes),
        bytes.len() as u64,
    ));
    layers.push(generated(Descriptor::new(
        mt::METADATA,
        Digest::of(EMPTY_METADATA),
        EMPTY_METADATA.len() as u64,
    )));
    let mut manifest = Manifest::new_with_types(
        crate::PLATFORM,
        configuration,
        layers,
        PcProvider.payload_types(),
    )?;
    for (key, value) in [
        ("org.opencontainers.image.title", &image.title),
        ("org.opencontainers.image.description", &image.description),
    ] {
        if let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            manifest.annotations.insert(key.into(), value.into());
        }
    }
    manifest.annotations.insert(
        "unemu.firmware.sources".into(),
        format!(
            "[{}]",
            plan.sources
                .iter()
                .map(|d| format!("\"{d}\""))
                .collect::<Vec<_>>()
                .join(",")
        ),
    );
    let inherited = crate::compat::packages_floor(
        crate::compat::source_levels(blobs),
        image.firmware.iter().map(|rom| &rom.package),
    )?;
    crate::compat::stamp(
        &mut manifest,
        &bytes,
        inherited,
        None,
        crate::compat::source_levels(blobs),
    )?;
    Ok((bytes, manifest))
}

/// Check a new image against the real ROM bytes without writing anything.
pub fn check(image: &NewImage, blobs: &dyn BlobSource) -> Result<()> {
    lowered(&plan(image)?, blobs).map(|_| ())
}

/// The manifest Create would write, with every descriptor's own mark, so a
/// caller can apply the push guard's rule to the whole prospective closure.
/// Nothing is written; `blobs` must hold the chosen ROMs.
pub fn preview(image: &NewImage, blobs: &dyn BlobSource) -> Result<Manifest> {
    Ok(preview_with_config(image, blobs)?.1)
}

/// [`preview`] with the configuration bytes Create would store.
pub fn preview_with_config(
    image: &NewImage,
    blobs: &dyn BlobSource,
) -> Result<(Vec<u8>, Manifest)> {
    finish(plan(image)?, blobs, image)
}

/// Build and tag a new empty image: configuration, the chosen firmware blobs
/// (already in the store), zero bases and empty media catalogues. A new tag
/// needs `expected = None`; moving a tag needs its current digest, and a
/// version-shaped tag never moves. Returns the manifest descriptor.
pub fn create(
    store: &Store,
    image: &NewImage,
    tag: &str,
    expected: Option<&Digest>,
) -> Result<Descriptor> {
    create_inner(store, image, tag, expected, None)
}

/// [`create()`] that tags only the image whose manifest digest is `judged`,
/// the one a caller checked from [`preview`], for example against a push guard.
pub fn create_judged(
    store: &Store,
    image: &NewImage,
    tag: &str,
    expected: Option<&Digest>,
    judged: &Digest,
) -> Result<Descriptor> {
    create_inner(store, image, tag, expected, Some(judged))
}

fn create_inner(
    store: &Store,
    image: &NewImage,
    tag: &str,
    expected: Option<&Digest>,
    judged: Option<&Digest>,
) -> Result<Descriptor> {
    store.check_tag_target(tag, expected, None)?;
    let rules = PcFirmwareRules(crate::firmware::catalogue());
    for rom in &image.firmware {
        store.resolve_descriptor(&rom.package.descriptor, &Validator(&rules))?;
    }
    let plan = plan(image)?;
    for bytes in plan.zero.values() {
        store.import(bytes.as_slice(), None)?;
    }
    let (config, manifest) = finish(plan, store, image)?;
    store.import(config.as_slice(), Some(&manifest.config.digest))?;
    store.import(EMPTY_METADATA, None)?;
    let descriptor = store.import_manifest_with_types(&manifest, PcProvider.payload_types())?;
    if let Some(judged) = judged.filter(|judged| **judged != descriptor.digest) {
        return Err(invalid(format!(
            "the image is {}, not the checked {judged}",
            descriptor.digest
        )));
    }
    store.update_tag(tag, &descriptor, expected, &PcProvider)?;
    Ok(descriptor)
}
