// SPDX-License-Identifier: AGPL-3.0-or-later
//! Offline raw-image creation and deterministic FAT12 formatting.
//!
//! Microsoft FAT Specification, FAT on-disk format, BPB and FAT12 entries;
//! IBM Personal Computer AT Technical Reference, diskette formats and INT 13h
//! CHS addressing. See machines/README.md § Preparing disks. Hard disks remain
//! unpartitioned; the bounded CHS policy targets the shipped XUB BIOS path.

use assembly::{FloppyGeometry, HardDiskGeometry};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub struct CreateCommand {
    pub output: PathBuf,
    pub force: bool,
    pub kind: ImageKind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ImageKind {
    Floppy {
        format: FloppyFormat,
        formatted: bool,
        label: Option<[u8; 11]>,
    },
    HardDisk(HardDiskGeometry),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloppyFormat {
    Hd144,
    Hd12,
    Dd720,
    Dd360,
}

impl FloppyFormat {
    fn geometry(self) -> FloppyGeometry {
        match self {
            Self::Hd144 => FloppyGeometry::HD_1_44,
            Self::Hd12 => FloppyGeometry::HD_1_2,
            Self::Dd720 => FloppyGeometry::DD_720K,
            Self::Dd360 => FloppyGeometry::DD_360K,
        }
    }
    fn layout(self) -> (u8, u16, u8, u16) {
        match self {
            Self::Hd144 => (1, 224, 0xF0, 9),
            Self::Hd12 => (1, 224, 0xF9, 7),
            Self::Dd720 => (2, 112, 0xF9, 3),
            Self::Dd360 => (2, 112, 0xFD, 2),
        }
    }
}

pub fn parse(args: &[OsString]) -> Result<CreateCommand, &'static str> {
    let kind = args
        .first()
        .and_then(|s| s.to_str())
        .ok_or("create requires floppy or hd")?;
    if !matches!(kind, "floppy" | "hd") {
        return Err("create requires floppy or hd");
    }
    let (mut output, mut floppy, mut chs, mut size, mut label) = (None, None, None, None, None);
    let (mut force, mut format) = (false, false);
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--force") if !force => force = true,
            Some("--format") if kind == "floppy" && !format => format = true,
            Some(option @ ("--type" | "--label" | "--chs" | "--size")) => {
                let value = args
                    .next()
                    .and_then(|s| s.to_str())
                    .filter(|s| !s.is_empty() && !s.starts_with("--"))
                    .ok_or("missing create option value")?;
                match option {
                    "--type" if kind == "floppy" && floppy.is_none() => {
                        floppy = Some(match value {
                            "1.44M" => FloppyFormat::Hd144,
                            "1.2M" => FloppyFormat::Hd12,
                            "720K" => FloppyFormat::Dd720,
                            "360K" => FloppyFormat::Dd360,
                            _ => return Err("floppy type must be 1.44M, 1.2M, 720K or 360K"),
                        })
                    }
                    "--label" if kind == "floppy" && label.is_none() => {
                        label = Some(volume_label(value)?)
                    }
                    "--chs" if kind == "hd" && chs.is_none() => chs = Some(parse_chs(value)?),
                    "--size" if kind == "hd" && size.is_none() => size = Some(parse_size(value)?),
                    _ => return Err("unknown, duplicate or incompatible create option"),
                }
            }
            _ if !arg.is_empty()
                && !arg.as_encoded_bytes().starts_with(b"--")
                && output.is_none() =>
            {
                output = Some(PathBuf::from(arg))
            }
            _ => return Err("unknown, duplicate or extra create argument"),
        }
    }
    let kind = if kind == "floppy" {
        if label.is_some() && !format {
            return Err("--label requires --format");
        }
        ImageKind::Floppy {
            format: floppy.ok_or("floppy requires --type")?,
            formatted: format,
            label,
        }
    } else {
        ImageKind::HardDisk(match (chs, size) {
            (Some(geometry), None) | (None, Some(geometry)) => geometry,
            _ => return Err("hd requires exactly one of --chs C/H/S or --size SIZE[MiB|GiB]"),
        })
    };
    Ok(CreateCommand {
        output: output.ok_or("create requires an output path")?,
        force,
        kind,
    })
}

fn unsigned(value: &str) -> Result<u64, &'static str> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err("expected an unsigned decimal integer");
    }
    value.parse().map_err(|_| "integer is out of range")
}

fn parse_chs(value: &str) -> Result<HardDiskGeometry, &'static str> {
    let mut parts = value.split('/');
    let mut next = || unsigned(parts.next().ok_or("expected --chs C/H/S")?);
    let (cylinders, heads, sectors) = (next()?, next()?, next()?);
    if parts.next().is_some() {
        return Err("expected --chs C/H/S");
    }
    if !(1..=1024).contains(&cylinders)
        || !(1..=16).contains(&heads)
        || !(1..=63).contains(&sectors)
    {
        return Err("BIOS CHS limit: cylinders 1..=1024, heads 1..=16, sectors 1..=63 (512 bytes each); larger/LBA disks are not supported by create");
    }
    Ok(HardDiskGeometry {
        cylinders: cylinders as u32,
        heads: heads as u32,
        sectors_per_track: sectors as u32,
        bytes_per_sector: 512,
    })
}

fn parse_size(value: &str) -> Result<HardDiskGeometry, &'static str> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix("GiB") {
        (number, 1 << 30)
    } else {
        (value.strip_suffix("MiB").unwrap_or(value), 1 << 20)
    };
    let bytes = unsigned(number)?
        .checked_mul(multiplier)
        .ok_or("size is out of range")?;
    if bytes == 0 || bytes > 1024 * 16 * 63 * 512 {
        return Err("BIOS CHS size limit: 1..=504 MiB (1024 cylinders x 16 heads x 63 sectors); larger/LBA disks are not supported by create");
    }
    let cylinders = bytes.div_ceil(16 * 63 * 512) as u32;
    Ok(HardDiskGeometry {
        cylinders,
        heads: 16,
        sectors_per_track: 63,
        bytes_per_sector: 512,
    })
}

fn volume_label(value: &str) -> Result<[u8; 11], &'static str> {
    if value.len() > 11
        || value.starts_with(' ')
        || value.trim().is_empty()
        || !value.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
        || value.bytes().any(|b| b"\"*+,./:;<=>?[\\]|".contains(&b))
    {
        return Err("label must contain 1..=11 ASCII characters, without leading spaces or FAT filename punctuation");
    }
    let mut label = [b' '; 11];
    for (out, byte) in label.iter_mut().zip(value.bytes()) {
        *out = byte.to_ascii_uppercase();
    }
    Ok(label)
}

/// Format metadata only; the atomic image writer supplies zero data sectors.
/// Fixed serial, FAT epoch and a silent CLI/HLT loop avoid host-time inputs and
/// accidental execution of filesystem metadata if this data floppy is booted.
fn fat12_prefix(format: FloppyFormat, label: Option<[u8; 11]>) -> Vec<u8> {
    let geometry = format.geometry();
    let (cluster_sectors, root_entries, media, fat_sectors) = format.layout();
    let root_start = (1 + 2 * usize::from(fat_sectors)) * 512;
    let mut disk = vec![0; root_start + usize::from(root_entries) * 32];
    disk[..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    disk[3..11].copy_from_slice(b"UNEMUPC ");
    for (offset, value) in [
        (11, 512),
        (14, 1),
        (17, root_entries),
        (19, (geometry.image_bytes() / 512) as u16),
        (22, fat_sectors),
        (24, geometry.sectors_per_track as u16),
        (26, geometry.heads as u16),
    ] {
        disk[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    disk[13] = cluster_sectors;
    disk[16] = 2;
    disk[21] = media;
    disk[38] = 0x29;
    disk[39..43].copy_from_slice(&0x28959855u32.to_le_bytes());
    disk[43..54].copy_from_slice(&label.unwrap_or(*b"NO NAME    "));
    disk[54..62].copy_from_slice(b"FAT12   ");
    disk[62..66].copy_from_slice(&[0xFA, 0xF4, 0xEB, 0xFD]);
    disk[510..512].copy_from_slice(&[0x55, 0xAA]);
    for start in [512, (1 + usize::from(fat_sectors)) * 512] {
        disk[start..start + 3].copy_from_slice(&[media, 0xFF, 0xFF]);
    }
    if let Some(label) = label {
        disk[root_start..root_start + 11].copy_from_slice(&label);
        disk[root_start + 11] = 8;
        // FAT dates cannot represent year zero. Use 1980-01-01 for all date fields.
        for offset in [16, 18, 24] {
            disk[root_start + offset..root_start + offset + 2]
                .copy_from_slice(&0x21u16.to_le_bytes());
        }
    }
    disk
}

pub fn execute(command: CreateCommand, output: &mut impl Write) -> io::Result<u8> {
    let (bytes, prefix) = match command.kind {
        ImageKind::Floppy {
            format,
            formatted,
            label,
        } => (
            format.geometry().image_bytes(),
            if formatted {
                fat12_prefix(format, label)
            } else {
                Vec::new()
            },
        ),
        ImageKind::HardDisk(geometry) => (geometry.image_bytes(), Vec::new()),
    };
    media::create::image(&command.output, bytes, &prefix, command.force).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("create {}: {error}", command.output.display()),
        )
    })?;
    writeln!(
        output,
        "created {} ({bytes} bytes)",
        command.output.display()
    )?;
    if let ImageKind::HardDisk(geometry) = command.kind {
        let image = command.output.canonicalize()?;
        if let Some(image) = image.to_str().filter(|s| !s.chars().any(char::is_control)) {
            let journal = format!("{image}.upfj")
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            let image = image.replace('\\', "\\\\").replace('"', "\\\"");
            writeln!(output, "\n# Merge into an at-ide.toml-style machine using the AT-reset BIOS and XUB.\n[[card]]\nid = \"ide\"\nkind = \"ide\"\nport = 0x1F0\nirq = 14\n\n[[card.drives]]\nchannel = \"master\"\nimage = \"{image}\"\ngeometry = {{ cylinders = {}, heads = {}, sectors = {} }}\nwrite = \"journal\"\njournal = \"{journal}\"\nkeep_generations = 10", geometry.cylinders, geometry.heads, geometry.sectors_per_track)?;
        } else {
            writeln!(
                output,
                "image path cannot be represented as a TOML string; set it manually"
            )?;
        }
        writeln!(output, "\nXUB supplies geometry through IDENTIFY; an IBM AT system BIOS alone requires a matching AT drive type (for example --chs 615/4/17).\nrun FDISK in DOS, reboot, then FORMAT C: /S (or FORMAT C: and SYS C:).\nguest writes persist in the adjacent journal and preserve the base. Stop the emulator before media commit (export a new image) or media rollback (with a backup). See machines/README.md, Preparing disks.")?;
    }
    Ok(0)
}
