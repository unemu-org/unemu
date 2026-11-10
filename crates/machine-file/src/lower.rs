// SPDX-License-Identifier: AGPL-3.0-or-later
//! Lower schema 1 into the assembly-owned PC/AT configuration.
//!
//! See `backlog/docs/design/machine-files.md` § Schema and § Validation and
//! presets. Motherboard wiring comes from assembly; card defaults describe the
//! IBM Personal Computer AT Technical Reference, Section 1, I/O Address Map.
//! Hardware consistency is checked only by `MachineConfig::validate`.

use std::{collections::BTreeMap, io, path::Path};

use assembly::{
    BoardModel, ClockConfig, ConfigError, CpuChip, CpuConfig, DeviceConfig, DeviceId, DeviceKind,
    DmaSpec, DriveConfig, DriveKind, FloppyGeometry, ImageKind, ImageSource, LoadImage,
    MachineConfig, MdaTextCfg, MemoryConfig, MmioWindow, NvramSource, OutputConfig, PortWindow,
    PostPortCfg, RamRegion, ResetSpec, RtcStart, SegmentSeed, UartCfg, VgaTextCfg,
};
use sha2::{Digest, Sha256};

use crate::{schema::v1, MachineFile, Origin, Origins};

const MIB: u64 = 0x100000;

/// Image I/O seam. Lowering hashes every returned buffer before retaining it.
///
/// Tests may return synthetic bytes; production reads each referenced image
/// once. Paths have already been resolved against the defining field's file.
pub trait ImageReader {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>>;
}

/// Read images from the filesystem without retaining open handles.
pub struct FileImageReader;

impl ImageReader for FileImageReader {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }
}

/// A completeness, image-integrity, unsupported-feature or assembly diagnostic.
#[derive(Debug, thiserror::Error)]
#[error("{origin}: {field}: {message}")]
pub struct LowerError {
    pub origin: Origin,
    pub field: String,
    pub message: String,
    /// The original assembly error, when hardware validation failed.
    #[source]
    pub validation: Option<Box<ConfigError>>,
}

struct DriveOrigin {
    field: String,
    label: String,
}

struct Context<'a, R> {
    origins: &'a Origins,
    reader: &'a mut R,
    labels: Vec<Option<String>>,
    devices: Vec<(DeviceId, String)>,
    drives: Vec<DriveOrigin>,
    images: Vec<String>,
}

impl<R: ImageReader> Context<'_, R> {
    fn label(&self, field: &str) -> String {
        let field = self
            .drives
            .iter()
            .find_map(|drive| {
                field
                    .strip_prefix(&drive.field)
                    .filter(|tail| tail.is_empty() || tail.starts_with('.'))
                    .map(|tail| format!("{}{tail}", drive.label))
            })
            .unwrap_or_else(|| field.into());
        for (index, label) in self.labels.iter().enumerate() {
            let prefix = format!("card[{index}]");
            if let Some(tail) = field.strip_prefix(&prefix) {
                return match label {
                    Some(label) => format!("card[{label}]{tail}"),
                    None => format!("card #{index}{tail}"),
                };
            }
        }
        field
    }

    fn error(&self, field: &str, message: impl Into<String>) -> LowerError {
        LowerError {
            origin: self.origins.get(field).clone(),
            field: self.label(field),
            message: message.into(),
            validation: None,
        }
    }

    fn required<T>(&self, field: &str, value: Option<T>) -> Result<T, LowerError> {
        value.ok_or_else(|| self.error(field, "required field is missing"))
    }

    fn image(
        &mut self,
        field: &str,
        image: Option<&str>,
        pin: Option<&v1::Sha256>,
    ) -> Result<Vec<u8>, LowerError> {
        let image = self.required(field, image)?;
        let origin = self.origins.get(field);
        let display_path = origin.file.parent().unwrap_or(Path::new(".")).join(image);
        let path = self.origins.image_path(field, image);
        let description = if path == display_path {
            display_path.display().to_string()
        } else {
            format!(
                "{} (resolved to {})",
                display_path.display(),
                path.display()
            )
        };
        let bytes = self.reader.read(&path).map_err(|error| {
            self.error(field, format!("cannot read image {description}: {error}"))
        })?;
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if let Some(pin) = pin {
            if actual != pin.as_str() {
                return Err(self.error(
                    field,
                    format!(
                        "sha256 mismatch for {description}: expected {}, actual {actual}",
                        pin.as_str()
                    ),
                ));
            }
        }
        Ok(bytes)
    }

    fn write(
        &self,
        field: &str,
        mode: Option<v1::WriteMode>,
        journal: bool,
        retention: bool,
    ) -> Result<assembly::WriteMode, LowerError> {
        match mode.unwrap_or(v1::WriteMode::Volatile) {
            v1::WriteMode::Journal => Err(self.error(field,
                "write = \"journal\" is not implemented yet; journal attachment and persistent-media integration are later work")),
            _ if journal || retention => Err(self.error(field,
                "journal and keep_generations are only accepted with write = \"journal\"")),
            v1::WriteMode::ReadOnly => Ok(assembly::WriteMode::ReadOnly),
            v1::WriteMode::Volatile => Ok(assembly::WriteMode::Volatile),
        }
    }

    fn rom(&mut self, field: &str, rom: &v1::Rom, bios: bool) -> Result<LoadImage, LowerError> {
        if !bios {
            let base = self.required(&format!("{field}.base"), rom.base)?;
            if !(0xC0000..0xF0000).contains(&base) || !base.is_multiple_of(2048) {
                return Err(self.error(
                    &format!("{field}.base"),
                    "option ROM base must be 2 KiB aligned in 0xC0000..=0xEFFFF",
                ));
            }
        }
        let bytes = self.image(
            &format!("{field}.image"),
            rom.image.as_deref(),
            rom.sha256.as_ref(),
        )?;
        let base = match rom.base {
            Some(base) => u64::from(base),
            None if bios => MIB
                .checked_sub(bytes.len() as u64)
                .ok_or_else(|| self.error(field, "BIOS exceeds the 1 MiB reset address space"))?,
            None => return Err(self.error(&format!("{field}.base"), "required field is missing")),
        };
        self.images.push(field.into());
        Ok(LoadImage {
            base,
            source: ImageSource::Inline(bytes),
            kind: ImageKind::Rom,
            provides: rom
                .provides
                .iter()
                .flatten()
                .map(|service| match service {
                    v1::FirmwareService::Floppy => assembly::FirmwareService::Floppy,
                    v1::FirmwareService::FixedDisk => assembly::FirmwareService::FixedDisk,
                })
                .collect(),
        })
    }

    fn rtc(&mut self, rtc: &v1::Rtc) -> Result<assembly::CmosCfg, LowerError> {
        let start = match rtc.start.as_deref() {
            Some(text) => rtc_start(text).ok_or_else(|| {
                self.error("board.rtc.start", "expected quoted YYYY-MM-DDTHH:MM:SS")
            })?,
            None => RtcStart::default(),
        };
        let nvram = match rtc.nvram.as_ref().unwrap_or(&v1::Nvram::Derived) {
            v1::Nvram::Derived => NvramSource::Derived,
            v1::Nvram::Blank => NvramSource::Blank,
            v1::Nvram::Image(image) => {
                let write = self.write(
                    "board.rtc.nvram.write",
                    image.write,
                    image.journal.is_some(),
                    image.keep_generations.is_some(),
                )?;
                if write == assembly::WriteMode::ReadOnly {
                    return Err(self.error("board.rtc.nvram.write", "readonly NVRAM is not implemented yet; assembly currently supports volatile NVRAM only"));
                }
                let bytes = self.image(
                    "board.rtc.nvram.image",
                    image.image.as_deref(),
                    image.sha256.as_ref(),
                )?;
                let actual = bytes.len();
                let bytes = bytes.try_into().map_err(|_| {
                    self.error(
                        "board.rtc.nvram.image",
                        format!("NVRAM image must contain 64 bytes, got {actual}"),
                    )
                })?;
                NvramSource::Image(bytes)
            }
        };
        Ok(assembly::CmosCfg {
            start,
            nvram,
            timing: None,
        })
    }

    fn card(
        &mut self,
        index: usize,
        card: &v1::Card,
        mmio: &mut Vec<MmioWindow>,
        drives: &mut Vec<DriveConfig>,
    ) -> Result<DeviceConfig, LowerError> {
        let field = format!("card[{index}]");
        if card.remove.is_some() {
            return Err(self.error(
                &format!("{field}.remove"),
                "remove is only accepted in an extends layer",
            ));
        }
        let kind = self.required(&format!("{field}.kind"), card.kind)?;
        if !matches!(kind, v1::CardKind::Uart16550 | v1::CardKind::Ide) && card.port.is_some() {
            return Err(self.error(
                &format!("{field}.port"),
                "port override is not supported by this fixed-port card kind",
            ));
        }
        let mut device = match kind {
            v1::CardKind::Mda => {
                mmio.push(MmioWindow::new(0xB0000, 0x8000, DeviceId::Mda));
                DeviceConfig::new(
                    DeviceId::Mda,
                    DeviceKind::Mda(MdaTextCfg {}),
                    vec![PortWindow::new(0x3B0, 0x3BB), PortWindow::single(0x3BF)],
                )
            }
            v1::CardKind::VgaText => {
                mmio.push(MmioWindow::new(0xB8000, 0x8000, DeviceId::Vga));
                DeviceConfig::new(
                    DeviceId::Vga,
                    DeviceKind::Vga(VgaTextCfg {}),
                    vec![PortWindow::new(0x3D4, 0x3D5), PortWindow::new(0x3D8, 0x3DA)],
                )
            }
            v1::CardKind::Uart16550 => {
                let base = card.port.unwrap_or(0x3F8);
                let default_irq = match base {
                    0x3F8 | 0x3E8 => Some(4),
                    0x2F8 | 0x2E8 => Some(3),
                    _ => None,
                };
                let irq = match card.irq {
                    Some(irq) => line(irq),
                    None => Some(self.required(&format!("{field}.irq"), default_irq)?),
                };
                let mut device = DeviceConfig::new(
                    DeviceId::Uart,
                    DeviceKind::Uart(UartCfg { base }),
                    vec![PortWindow::new(base, base.saturating_add(7))],
                );
                device.irq = irq;
                device
            }
            v1::CardKind::Fdc765 => DeviceConfig::new(
                DeviceId::Fdc,
                DeviceKind::Fdc(assembly::FdcCfg { drive_slots: 2 }),
                assembly::FdcCfg::port_windows(false),
            )
            .with_irq(6)
            .with_dma(DmaSpec {
                controller: DeviceId::Dma,
                channel: 2,
            }),
            v1::CardKind::Ide => {
                let base = card.port.unwrap_or(0x1F0);
                let cfg = assembly::IdeCfg {
                    base,
                    control: base.saturating_add(0x206),
                };
                DeviceConfig::new(DeviceId::Ide, DeviceKind::Ide(cfg), cfg.port_windows())
                    .with_irq(cfg.irq())
            }
        };
        if matches!(kind, v1::CardKind::Mda | v1::CardKind::VgaText) && card.irq.is_some() {
            return Err(self.error(&format!("{field}.irq"), "this card has no interrupt source"));
        }
        if kind != v1::CardKind::Fdc765 && card.dma.is_some() {
            return Err(self.error(&format!("{field}.dma"), "this card cannot master DMA"));
        }
        if let Some(irq) = card.irq {
            device.irq = line(irq);
        }
        if let Some(dma) = card.dma {
            device.dma = line(dma).map(|channel| DmaSpec {
                controller: DeviceId::Dma,
                channel,
            });
        }
        if !matches!(kind, v1::CardKind::Fdc765 | v1::CardKind::Ide) && card.drives.is_some() {
            return Err(self.error(
                &format!("{field}.drives"),
                "drives are only accepted on a disk controller",
            ));
        }
        let mut ordered: Vec<_> = card.drives.iter().flatten().enumerate().collect();
        ordered.sort_by_key(|(_, drive)| {
            if kind == v1::CardKind::Ide {
                drive
                    .channel
                    .map(|channel| u8::from(channel == v1::Channel::Slave))
            } else {
                drive.unit
            }
        });
        for (drive_index, drive) in ordered {
            let drive_field = format!("{field}.drives[{drive_index}]");
            let (unit, drive_kind, label) = if kind == v1::CardKind::Ide {
                if drive.remove.is_some() || drive.unit.is_some() || drive.drive_type.is_some() {
                    return Err(self.error(&drive_field, "IDE drives accept channel and geometry; unit/type are floppy-only, remove is extends-only"));
                }
                let channel = self.required(&format!("{drive_field}.channel"), drive.channel)?;
                let unit = u8::from(channel == v1::Channel::Slave);
                let v1::Geometry::Chs(geometry) =
                    self.required(&format!("{drive_field}.geometry"), drive.geometry.as_ref())?;
                (
                    unit,
                    DriveKind::HardDisk {
                        geometry: assembly::HardDiskGeometry {
                            cylinders: self.required(
                                &format!("{drive_field}.geometry.cylinders"),
                                geometry.cylinders,
                            )?,
                            heads: self.required(
                                &format!("{drive_field}.geometry.heads"),
                                geometry.heads,
                            )?,
                            sectors_per_track: self.required(
                                &format!("{drive_field}.geometry.sectors"),
                                geometry.sectors,
                            )?,
                            bytes_per_sector: 512,
                        },
                    },
                    format!(
                        "{field}.drives[channel={}]",
                        if unit == 0 { "master" } else { "slave" }
                    ),
                )
            } else {
                let unit = self.required(&format!("{drive_field}.unit"), drive.unit)?;
                if drive.remove.is_some() || drive.channel.is_some() || drive.geometry.is_some() {
                    return Err(self.error(&drive_field, "floppy drives accept unit and type; channel/geometry are IDE-only, remove is extends-only"));
                }
                let geometry =
                    match self.required(&format!("{drive_field}.type"), drive.drive_type)? {
                        v1::DriveType::Hd144 => FloppyGeometry::HD_1_44,
                        v1::DriveType::Hd12 => FloppyGeometry::HD_1_2,
                        v1::DriveType::Dd720 => FloppyGeometry::DD_720K,
                        v1::DriveType::Dd360 => FloppyGeometry::DD_360K,
                    };
                (
                    unit,
                    DriveKind::Floppy { geometry },
                    format!("{field}.drives[unit={unit}]"),
                )
            };
            self.drives.push(DriveOrigin {
                field: drive_field.clone(),
                label,
            });
            let write = self.write(
                &format!("{drive_field}.write"),
                drive.write,
                drive.journal.is_some(),
                drive.keep_generations.is_some(),
            )?;
            let bytes = self.image(
                &format!("{drive_field}.image"),
                drive.image.as_deref(),
                drive.sha256.as_ref(),
            )?;
            drives.push(DriveConfig {
                controller: device.id,
                unit,
                kind: drive_kind,
                image: ImageSource::Inline(bytes),
                write,
            });
        }
        self.devices.push((device.id, field));
        Ok(device)
    }

    fn validation_error(&self, error: ConfigError) -> LowerError {
        use ConfigError::*;
        let device_fields = |ids: &[DeviceId]| -> Vec<String> {
            let mut fields: Vec<_> = self
                .devices
                .iter()
                .filter(|(id, _)| ids.contains(id))
                .map(|(_, field)| field.clone())
                .collect();
            fields.sort_by_key(|field| field.starts_with("board"));
            fields
        };
        let mut message = error.to_string();
        let fields = match &error {
            MissingSystemBios { .. }
            | FirmwareNotListed { .. }
            | FirmwareBoardMismatch { .. }
            | FirmwareNeedsCpu { .. }
            | FirmwareRequiresDevice { .. }
            | BiosPlacement { .. } => vec!["firmware.bios.image".into()],
            FirmwareProvidesMismatch { image_index, .. } => {
                vec![format!("{}.provides", self.image_label(*image_index))]
            }
            MultipleIdeChannels => device_fields(&[DeviceId::Ide]),
            FirmwareReset { .. } => vec!["cpu".into()],
            SystemBiosOverlapsRam { .. } => vec!["memory".into(), "firmware.bios.image".into()],
            DuplicateDeviceId(id) | MmioDeviceMissing(id) => device_fields(&[*id]),
            InvalidIdeResources { device }
            | InvalidIdePortWindows { device }
            | NoProvider { device, .. }
            | InvalidRtcStart { device, .. }
            | SelfAccessCoupling { device }
            | AccessDependentMissing { device, .. }
            | UartPortWindowOutOfSpace { device, .. }
            | UnsupportedWordPort { device, .. }
            | UnalignedWordPortWindow { device, .. }
            | UnsupportedObservationOutput { device, .. }
            | DuplicateObservationRoute { device, .. }
            | EmptyMmioWindow { device, .. }
            | InvalidIrqLine { device, .. }
            | MmioOverlapsRam { device, .. }
            | BoardDeviceMissing { device, .. }
            | BoardDeviceOrder { device, .. }
            | BoardDeviceWiring { device, .. }
            | BoardCardIrq { device, .. }
            | BoardCardDma { device, .. }
            | BoardCardDmaController { device, .. } => device_fields(&[*device]),
            DuplicatePicFace { a, b, .. }
            | PortOverlap { a, b, .. }
            | PortOverlapDecodeOrder { a, b, .. }
            | MmioOverlap { a, b, .. }
            | IrqConflict { a, b, .. } => device_fields(&[*a, *b]),
            DmaControllerMissing { requester, .. } => device_fields(&[*requester]),
            // Journal mode is not lowered yet, but every drive-indexed error
            // keeps the same origin and label contract as assembly validation.
            Journal { drive_index, .. }
            | InvalidAtaGeometry { drive_index }
            | AtaMissingMaster { drive_index, .. }
            | UnsupportedFloppySectorSize { drive_index, .. }
            | DriveControllerMissing { drive_index, .. }
            | DriveControllerKind { drive_index, .. }
            | DriveUnitUnavailable { drive_index, .. }
            | ImageGeometryMismatch { drive_index, .. } => {
                message = replace_index_prefix(
                    &message,
                    &format!("drive {drive_index} "),
                    &format!("{} ", self.drive_label(*drive_index, false)),
                );
                self.drives
                    .get(*drive_index)
                    .map(|drive| drive.field.clone())
                    .into_iter()
                    .collect()
            }
            DuplicateDriveUnit { a, b, .. } => {
                message = replace_index_prefix(
                    &message,
                    &format!("drives {a} and {b} "),
                    &format!(
                        "{} and {} ",
                        self.drive_label(*a, true),
                        self.drive_label(*b, true)
                    ),
                );
                [*a, *b]
                    .iter()
                    .filter_map(|i| self.drives.get(*i).map(|drive| drive.field.clone()))
                    .collect()
            }
            ServicesOnRamLoad { image } => {
                message = replace_index_prefix(
                    &message,
                    &format!("RAM load image {image} "),
                    &format!("{} ", self.image_label(*image)),
                );
                self.images.get(*image).cloned().into_iter().collect()
            }
            ImageOverlapsMmio {
                image_index,
                device,
                ..
            } => {
                message = replace_index_prefix(
                    &message,
                    &format!("image {image_index} "),
                    &format!("{} ", self.image_label(*image_index)),
                );
                let mut fields: Vec<_> =
                    self.images.get(*image_index).cloned().into_iter().collect();
                fields.extend(device_fields(&[*device]));
                fields
            }
            ImageOutsideRam { image_index, .. } => {
                message = replace_index_prefix(
                    &message,
                    &format!("ram-load image {image_index} "),
                    &format!("{} ", self.image_label(*image_index)),
                );
                self.images.get(*image_index).cloned().into_iter().collect()
            }
            ImageOverlap { a, b, .. } => {
                message = replace_index_prefix(
                    &message,
                    &format!("images {a} and {b} "),
                    &format!("{} and {} ", self.image_label(*a), self.image_label(*b)),
                );
                [*a, *b]
                    .iter()
                    .filter_map(|i| self.images.get(*i).cloned())
                    .collect()
            }
            InvalidCpuRate { .. } | StepBoundedCpuRequired(_) => vec!["cpu".into()],
            RegionOutOfSpace { .. } => self.images.clone(),
            ImageRead { .. } => vec!["firmware".into()],
        };
        let field = fields.first().map_or("memory", String::as_str);
        for related in fields.iter().skip(1) {
            message.push_str(&format!(
                "; {}: {}",
                self.origins.get(related),
                self.label(related)
            ));
        }
        let mut diagnostic = self.error(field, message);
        diagnostic.validation = Some(Box::new(error));
        diagnostic
    }

    fn drive_label(&self, index: usize, with_origin: bool) -> String {
        self.drives.get(index).map_or_else(
            || format!("drive #{index}"),
            |drive| {
                let label = self.label(&drive.field);
                if with_origin {
                    format!("{label} ({})", self.origins.get(&drive.field))
                } else {
                    label
                }
            },
        )
    }

    fn image_label(&self, index: usize) -> String {
        self.images
            .get(index)
            .cloned()
            .unwrap_or_else(|| format!("image #{index}"))
    }
}

// Assembly owns the wording; only its configuration indices need file labels.
fn replace_index_prefix(message: &str, prefix: &str, label: &str) -> String {
    message
        .strip_prefix(prefix)
        .map_or_else(|| message.into(), |tail| format!("{label}{tail}"))
}

fn line<const MAX: u16>(value: v1::NumberOrNone<MAX>) -> Option<u8> {
    match value {
        v1::NumberOrNone::Number(value) => Some(value as u8),
        v1::NumberOrNone::None => None,
    }
}

fn kind_name(kind: v1::CardKind) -> &'static str {
    match kind {
        v1::CardKind::Mda => "mda",
        v1::CardKind::VgaText => "vga-text",
        v1::CardKind::Uart16550 => "uart-16550",
        v1::CardKind::Fdc765 => "fdc-765",
        v1::CardKind::Ide => "ide",
    }
}

fn rtc_start(text: &str) -> Option<RtcStart> {
    let bytes = text.as_bytes();
    if bytes.len() != 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes
            .iter()
            .enumerate()
            .any(|(i, byte)| ![4, 7, 10, 13, 16].contains(&i) && !byte.is_ascii_digit())
    {
        return None;
    }
    Some(RtcStart {
        year: text.get(0..4)?.parse().ok()?,
        month: text.get(5..7)?.parse().ok()?,
        day: text.get(8..10)?.parse().ok()?,
        hour: text.get(11..13)?.parse().ok()?,
        minute: text.get(14..16)?.parse().ok()?,
        second: text.get(17..19)?.parse().ok()?,
    })
}

fn ram(
    conventional: u64,
    extended: u64,
    mmio: &[MmioWindow],
    images: &[LoadImage],
) -> Vec<RamRegion> {
    let mut regions = vec![(0xA0000, MIB)];
    for (base, end) in mmio
        .iter()
        .map(|w| (w.base, w.end()))
        .chain(images.iter().map(|image| {
            (
                image.base,
                image.base + image.source.known_len().unwrap_or(0),
            )
        }))
    {
        regions = regions
            .into_iter()
            .flat_map(|(start, stop)| {
                if base >= stop || end <= start {
                    return vec![(start, stop)];
                }
                vec![(start, base.max(start)), (end.min(stop), stop)]
            })
            .filter(|(start, stop)| start < stop)
            .collect();
    }
    regions.extend([(0, conventional), (MIB, MIB + extended)]);
    regions.sort_unstable();
    let mut joined: Vec<RamRegion> = Vec::new();
    for (base, end) in regions.into_iter().filter(|(base, end)| base < end) {
        if let Some(previous) = joined.last_mut().filter(|region| region.end() == base) {
            previous.size = end - previous.base;
        } else {
            joined.push(RamRegion::new(base, end - base));
        }
    }
    joined
}

/// Expand a complete typed document, read/hash its images and validate the result.
///
/// Origins define per-field path bases and diagnostic positions. No environment
/// or clock is consulted. Inheritance must be resolved before calling lowering;
/// unresolved `extends` and `remove` are rejected rather than ignored.
pub fn lower<R: ImageReader>(
    document: &MachineFile,
    origins: &Origins,
    reader: &mut R,
) -> Result<MachineConfig, LowerError> {
    let mut context = Context {
        origins,
        reader,
        labels: document
            .card
            .iter()
            .flatten()
            .map(|card| {
                card.id
                    .clone()
                    .or_else(|| card.kind.map(kind_name).map(String::from))
            })
            .collect(),
        devices: Vec::new(),
        drives: Vec::new(),
        images: Vec::new(),
    };
    if document.extends.is_some() {
        return Err(context.error(
            "extends",
            "extends resolution is not implemented yet; lower a complete document",
        ));
    }
    let mut card_ids = BTreeMap::new();
    for (index, card) in document.card.iter().flatten().enumerate() {
        let Some(id) = card.id.as_deref().or_else(|| card.kind.map(kind_name)) else {
            continue;
        };
        if let Some(first) = card_ids.insert(id, index) {
            let first_field = format!("card[{first}]");
            return Err(context.error(
                &format!("card[{index}]"),
                format!(
                    "duplicate card id `{id}`; first defined at {}: {}",
                    origins.get(&first_field),
                    context.label(&first_field)
                ),
            ));
        }
    }
    let board = context.required(
        "board.model",
        document.board.as_ref().and_then(|board| board.model),
    )?;
    let board = match board {
        v1::BoardModel::Ibm5170 => BoardModel::Ibm5170,
    };
    let mut devices = board.motherboard_devices();
    if let Some(settings) = &document.board {
        if let Some(port) = settings.post_port {
            let port = match port {
                v1::NumberOrNone::Number(port) => Some(port),
                v1::NumberOrNone::None => None,
            };
            if let Some(post) = devices.iter_mut().find(|d| d.id == DeviceId::Post) {
                if let Some(port) = port {
                    post.kind = DeviceKind::Post(PostPortCfg { port });
                    post.ports = vec![PortWindow::single(port)];
                }
            }
            if port.is_none() {
                devices.retain(|d| d.id != DeviceId::Post);
            }
            if port != Some(0x80) {
                if let Some(dma) = devices.iter_mut().find(|d| d.id == DeviceId::Dma) {
                    dma.ports
                        .iter_mut()
                        .filter(|window| window.start == 0x81)
                        .for_each(|window| window.start = 0x80);
                }
            }
        }
        if let Some(rtc) = &settings.rtc {
            let cfg = context.rtc(rtc)?;
            if let Some(device) = devices.iter_mut().find(|d| d.id == DeviceId::Rtc) {
                device.kind = DeviceKind::Rtc(cfg);
            }
        }
    }
    context.devices.extend(devices.iter().map(|d| {
        (
            d.id,
            if d.id == DeviceId::Rtc {
                "board.rtc"
            } else {
                "board"
            }
            .into(),
        )
    }));
    let firmware = context.required("firmware.bios", document.firmware.as_ref())?;
    let bios = context.required("firmware.bios", firmware.bios.as_ref())?;
    let mut images = vec![context.rom("firmware.bios", bios, true)?];
    for (index, rom) in firmware.option_roms.iter().flatten().enumerate() {
        images.push(context.rom(&format!("firmware.option_roms[{index}]"), rom, false)?);
    }
    let mut mmio = Vec::new();
    let mut drives = Vec::new();
    for (index, card) in document.card.iter().flatten().enumerate() {
        devices.push(context.card(index, card, &mut mmio, &mut drives)?);
    }
    let primary_ata = devices
        .iter()
        .any(|device| matches!(device.kind, DeviceKind::Ide(cfg) if cfg.base == 0x1F0));
    for device in &mut devices {
        if matches!(device.kind, DeviceKind::Fdc(_)) {
            device.ports = assembly::FdcCfg::port_windows(primary_ata);
        }
    }
    mmio.sort_by_key(|window| window.base);
    let conventional = document
        .memory
        .as_ref()
        .and_then(|memory| memory.conventional_kib)
        .unwrap_or(640);
    let extended = document
        .memory
        .as_ref()
        .and_then(|memory| memory.extended_kib)
        .unwrap_or(0);
    if conventional > 640 || !conventional.is_multiple_of(16) {
        return Err(context.error(
            "memory.conventional_kib",
            "must be a multiple of 16 KiB, at most 640 KiB",
        ));
    }
    if extended > 15360 || !extended.is_multiple_of(64) {
        return Err(context.error(
            "memory.extended_kib",
            "must be a multiple of 64 KiB, at most 15360 KiB on an 80286",
        ));
    }
    let mut clock = ClockConfig::default();
    if let Some(rate) = document.cpu.as_ref().and_then(|cpu| cpu.rate) {
        clock.retired_per_pit_clock = rate.steps.get();
        clock.retired_per_pit_clock_denominator = rate.pit_clocks.get();
    }
    let chip = match document
        .cpu
        .as_ref()
        .and_then(|cpu| cpu.model)
        .unwrap_or(v1::CpuModel::I80286)
    {
        v1::CpuModel::I80286 => CpuChip::I80286,
    };
    let config = MachineConfig {
        board,
        cpu: CpuConfig {
            chip,
            reset: ResetSpec::reset_vector(),
            segments: SegmentSeed::flat(0xF000),
        },
        clock,
        memory: MemoryConfig {
            address_space_bytes: MIB + u64::from(extended) * 1024,
            ram: ram(
                u64::from(conventional) * 1024,
                u64::from(extended) * 1024,
                &mmio,
                &images,
            ),
            mmio,
        },
        devices,
        drives,
        images,
        output: OutputConfig::default(),
    };
    config
        .validate()
        .map_err(|error| context.validation_error(error))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context<'a>(
        origins: &'a Origins,
        reader: &'a mut FileImageReader,
    ) -> Context<'a, FileImageReader> {
        Context {
            origins,
            reader,
            labels: vec![Some("serial".into()), Some("diskette".into())],
            devices: vec![
                (DeviceId::MasterPic, "board".into()),
                (DeviceId::Uart, "card[0]".into()),
            ],
            drives: (0..3)
                .map(|index| DriveOrigin {
                    field: format!("card[1].drives[{index}]"),
                    label: format!("card[1].drives[unit={}]", u8::from(index != 0)),
                })
                .collect(),
            images: Vec::new(),
        }
    }

    #[test]
    fn firmware_errors_retain_the_bios_image_origin_and_assembly_wording() {
        let mut origins = Origins::new("machine.toml");
        origins.insert(
            "firmware.bios",
            Origin {
                file: "machine.toml".into(),
                line: Some(4),
            },
        );
        let image_origin = Origin {
            file: "firmware.toml".into(),
            line: Some(9),
        };
        origins.insert("firmware.bios.image", image_origin.clone());
        let mut reader = FileImageReader;
        let context = context(&origins, &mut reader);
        for error in [
            ConfigError::MissingSystemBios { board: "ibm-5170" },
            ConfigError::FirmwareNotListed {
                sha256: "0".repeat(64),
                board: "ibm-5170",
                known: "kiselev-xi8088-1.0.2 (open, vendor/kiselev-bios/fetch.sh)".into(),
            },
            ConfigError::FirmwareBoardMismatch {
                firmware: "glabios-0.4.3-5160",
                boards: "ibm-5160".into(),
                board: "ibm-5170",
            },
            ConfigError::FirmwareNeedsCpu {
                firmware: "seabios-1.17.0-isapc",
                minimum: assembly::FirmwareCpu::I80386,
                cpu: assembly::FirmwareCpu::I80286,
            },
            ConfigError::FirmwareRequiresDevice {
                firmware: "bochs-rombios-legacy",
                part: assembly::FirmwarePart::VideoRom,
            },
            ConfigError::BiosPlacement {
                base: 0xF8000,
                size: 24 * 1024,
            },
        ] {
            let diagnostic = context.validation_error(error.clone());
            assert_eq!(diagnostic.origin, image_origin);
            assert_eq!(diagnostic.field, "firmware.bios.image");
            assert_eq!(diagnostic.message, error.to_string());
            assert_eq!(diagnostic.validation.as_deref(), Some(&error));
            assert_eq!(
                diagnostic.to_string(),
                format!("firmware.toml:9: firmware.bios.image: {error}")
            );
        }
    }

    #[test]
    fn firmware_layout_errors_name_cpu_or_memory_and_the_related_bios() {
        let input = "schema=1\n[cpu]\nmodel='80286'\n[memory]\nconventional_kib=640\n[firmware.bios]\nimage='bios.bin'\n";
        let origins = Origins::from_toml("machine.toml", input).unwrap();
        let mut reader = FileImageReader;
        let context = context(&origins, &mut reader);
        let reset = ConfigError::FirmwareReset {
            board: "ibm-5170",
            reset: ResetSpec::Entry { cs: 0, ip: 0x500 },
        };
        let diagnostic = context.validation_error(reset.clone());
        assert_eq!(diagnostic.field, "cpu");
        assert_eq!(&diagnostic.origin, origins.get("cpu"));
        assert_eq!(diagnostic.message, reset.to_string());
        let ram = ConfigError::SystemBiosOverlapsRam {
            board: "ibm-5170",
            addr: 0xF8000,
        };
        let diagnostic = context.validation_error(ram.clone());
        assert_eq!(diagnostic.field, "memory");
        assert_eq!(&diagnostic.origin, origins.get("memory"));
        assert_eq!(
            diagnostic.message,
            format!(
                "{ram}; {}: firmware.bios.image",
                origins.get("firmware.bios.image")
            )
        );
    }

    #[test]
    fn word_port_errors_retain_their_device_origins() {
        let mut origins = Origins::new("machine.toml");
        origins.insert(
            "card[0]",
            Origin {
                file: "serial.toml".into(),
                line: Some(12),
            },
        );
        let mut reader = FileImageReader;
        let context = context(&origins, &mut reader);
        for error in [
            ConfigError::InvalidIdeResources {
                device: DeviceId::Uart,
            },
            ConfigError::InvalidIdePortWindows {
                device: DeviceId::Uart,
            },
            ConfigError::UnsupportedWordPort {
                device: DeviceId::Uart,
                port: 0x3F8,
            },
            ConfigError::UnalignedWordPortWindow {
                device: DeviceId::Uart,
                start: 0x3F9,
            },
            ConfigError::PortOverlapDecodeOrder {
                a: DeviceId::MasterPic,
                b: DeviceId::Uart,
                a_window: PortWindow::single(0x20),
                b_window: PortWindow::new(0x20, 0x27),
                port: 0x20,
            },
        ] {
            let diagnostic = context.validation_error(error.clone());
            assert_eq!(diagnostic.field, "card[serial]");
            assert_eq!(&diagnostic.origin, origins.get("card[0]"));
            assert_eq!(diagnostic.validation.as_deref(), Some(&error));
            if matches!(error, ConfigError::PortOverlapDecodeOrder { .. }) {
                assert!(diagnostic.message.contains("; machine.toml: board"));
            }
        }
    }

    #[test]
    fn drive_error_prefixes_match_assembly_display() {
        let mut origins = Origins::new("machine.toml");
        origins.insert(
            "card[1].drives[1]",
            Origin {
                file: "machine.toml".into(),
                line: Some(10),
            },
        );
        origins.insert(
            "card[1].drives[2]",
            Origin {
                file: "machine.toml".into(),
                line: Some(18),
            },
        );
        let mut reader = FileImageReader;
        let context = context(&origins, &mut reader);
        for (error, prefix, label) in [
            (ConfigError::InvalidAtaGeometry { drive_index: 2 }, "drive 2 ", "card[diskette].drives[unit=1] "),
            (ConfigError::AtaMissingMaster { drive_index: 2, controller: DeviceId::Ide }, "drive 2 ", "card[diskette].drives[unit=1] "),
            (ConfigError::Journal { drive_index: 2, message: "invalid journal path".into() }, "drive 2 ", "card[diskette].drives[unit=1] "),
            (ConfigError::UnsupportedFloppySectorSize { drive_index: 2, bytes_per_sector: 256 }, "drive 2 ", "card[diskette].drives[unit=1] "),
            (ConfigError::DriveControllerMissing { drive_index: 2, controller: DeviceId::Fdc }, "drive 2 ", "card[diskette].drives[unit=1] "),
            (ConfigError::DriveControllerKind { drive_index: 2, controller: DeviceId::Uart }, "drive 2 ", "card[diskette].drives[unit=1] "),
            (ConfigError::DriveUnitUnavailable { drive_index: 2, controller: DeviceId::Fdc, unit: 1, available: 1 }, "drive 2 ", "card[diskette].drives[unit=1] "),
            (ConfigError::ImageGeometryMismatch { drive_index: 2, expected: 512, actual: 7 }, "drive 2 ", "card[diskette].drives[unit=1] "),
            (ConfigError::DuplicateDriveUnit { a: 1, b: 2, controller: DeviceId::Fdc, unit: 1 }, "drives 1 and 2 ", "card[diskette].drives[unit=1] (machine.toml:10) and card[diskette].drives[unit=1] (machine.toml:18) "),
        ] {
            let original = error.to_string();
            assert!(original.starts_with(prefix), "assembly prefix changed: {original}");
            let suffix = original.strip_prefix(prefix).unwrap();
            let diagnostic = context.validation_error(error);
            assert!(diagnostic.message.starts_with(&format!("{label}{suffix}")), "{diagnostic}");
        }
    }

    #[test]
    fn image_error_prefixes_match_assembly_display() {
        let origins = Origins::new("machine.toml");
        let mut reader = FileImageReader;
        let mut context = context(&origins, &mut reader);
        context.images = vec![
            "firmware.bios".into(),
            "firmware.option_roms[0]".into(),
            "firmware.option_roms[1]".into(),
        ];
        for (error, prefix, label) in [
            (
                ConfigError::ServicesOnRamLoad { image: 2 },
                "RAM load image 2 ",
                "firmware.option_roms[1] ",
            ),
            (
                ConfigError::ImageOverlapsMmio {
                    image_index: 2,
                    device: DeviceId::Uart,
                    addr: 0xC8800,
                },
                "image 2 ",
                "firmware.option_roms[1] ",
            ),
            (
                ConfigError::ImageOutsideRam {
                    image_index: 2,
                    addr: 0xC8800,
                },
                "ram-load image 2 ",
                "firmware.option_roms[1] ",
            ),
            (
                ConfigError::ImageOverlap {
                    a: 1,
                    b: 2,
                    addr: 0xC8800,
                },
                "images 1 and 2 ",
                "firmware.option_roms[0] and firmware.option_roms[1] ",
            ),
        ] {
            let original = error.to_string();
            assert!(
                original.starts_with(prefix),
                "assembly prefix changed: {original}"
            );
            let suffix = original.strip_prefix(prefix).unwrap();
            let diagnostic = context.validation_error(error);
            assert!(
                diagnostic.message.starts_with(&format!("{label}{suffix}")),
                "{diagnostic}"
            );
        }
    }
    #[test]
    fn firmware_service_mismatch_keeps_the_matched_image_origin() {
        let mut origins = Origins::new("machine.toml");
        origins.insert(
            "firmware.option_roms[0]",
            Origin {
                file: "roms.toml".into(),
                line: Some(12),
            },
        );
        let mut reader = FileImageReader;
        let mut context = context(&origins, &mut reader);
        context.images = vec!["firmware.bios".into(), "firmware.option_roms[0]".into()];
        let error = ConfigError::FirmwareProvidesMismatch {
            image_index: 1,
            firmware: "test-rom",
            declared: "[\"floppy\"]".into(),
            expected: "[\"fixed-disk\"]".into(),
        };
        let diagnostic = context.validation_error(error.clone());
        assert_eq!(diagnostic.field, "firmware.option_roms[0].provides");
        assert_eq!(diagnostic.origin.file, Path::new("roms.toml"));
        assert_eq!(diagnostic.origin.line, Some(12));
        assert_eq!(diagnostic.message, error.to_string());
    }
}
