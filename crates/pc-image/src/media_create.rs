// SPDX-License-Identifier: AGPL-3.0-or-later
//! Offline raw-image creation: blank or formatted diskettes, diskettes filled
//! from host files (one disk or a numbered set), and blank hard disks. The
//! command line's `media create` and the desktop's create dialog both parse
//! and execute their requests here, so the two write the same images.
//!
//! The FAT12 formatter and builder live in [`media::fat12`] (Microsoft FAT
//! Specification; IBM Personal Computer AT Technical Reference, diskette
//! formats and INT 13h CHS addressing). See the machine guide in unemupc § Preparing
//! disks. Hard disks remain unpartitioned; the bounded CHS policy targets the
//! shipped XUB BIOS path.

use assembly::HardDiskGeometry;
pub use media::fat12::FloppyFormat;
use media::fat12::{volume_label, BuildError, Request, ShortName};
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

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
        /// Host files to copy onto the new disk; implies `formatted`.
        files: Option<FloppyFiles>,
    },
    HardDisk(HardDiskGeometry),
}

/// `--add`, `--top` and `--split`: what goes on a diskette built from host files.
#[derive(Debug, PartialEq, Eq)]
pub struct FloppyFiles {
    pub sources: Vec<PathBuf>,
    pub top: Option<ShortName>,
    /// Spread the files over `OUT-1ofN`, `OUT-2ofN`… instead of refusing a misfit.
    pub split: bool,
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
    let (mut force, mut format, mut split) = (false, false, false);
    let (mut sources, mut top) = (Vec::new(), None);
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--force") if !force => force = true,
            Some("--format") if kind == "floppy" && !format => format = true,
            Some("--split") if kind == "floppy" && !split => split = true,
            // Host paths need not be UTF-8, so --add keeps the raw argument.
            Some("--add") if kind == "floppy" => sources.push(PathBuf::from(
                args.next()
                    .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
                    .ok_or("missing create option value")?,
            )),
            Some(option @ ("--type" | "--label" | "--top" | "--chs" | "--size")) => {
                let value = args
                    .next()
                    .and_then(|s| s.to_str())
                    .filter(|s| !s.is_empty() && !s.starts_with("--"))
                    .ok_or("missing create option value")?;
                match option {
                    "--type" if kind == "floppy" && floppy.is_none() => {
                        floppy = Some(
                            FloppyFormat::from_name(value)
                                .ok_or("floppy type must be 1.44M, 1.2M, 720K or 360K")?,
                        )
                    }
                    "--label" if kind == "floppy" && label.is_none() => {
                        label = Some(volume_label(value)?)
                    }
                    "--top" if kind == "floppy" && top.is_none() => {
                        top = Some(
                            ShortName::parse(value).ok_or("--top must be a DOS 8.3 folder name")?,
                        )
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
        if sources.is_empty() && (top.is_some() || split) {
            return Err("--top and --split require --add");
        }
        if label.is_some() && !format && sources.is_empty() {
            return Err("--label requires --format or --add");
        }
        ImageKind::Floppy {
            format: floppy.ok_or("floppy requires --type")?,
            formatted: format || !sources.is_empty(),
            label,
            files: (!sources.is_empty()).then_some(FloppyFiles {
                sources,
                top,
                split,
            }),
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

pub fn execute(command: CreateCommand, output: &mut impl Write) -> io::Result<u8> {
    let (bytes, prefix) = match command.kind {
        ImageKind::Floppy {
            format,
            label,
            files: Some(files),
            ..
        } => return execute_files(&command.output, command.force, format, label, files, output),
        ImageKind::Floppy {
            format,
            formatted,
            label,
            files: None,
        } => (
            format.image_bytes(),
            if formatted {
                media::fat12::format_metadata(format, label)
            } else {
                Vec::new()
            },
        ),
        ImageKind::HardDisk(geometry) => (geometry.image_bytes(), Vec::new()),
    };
    journal_files::create::image(&command.output, bytes, &prefix, command.force).map_err(
        |error| {
            io::Error::new(
                error.kind(),
                format!("create {}: {error}", command.output.display()),
            )
        },
    )?;
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
            writeln!(output, "\n# Merge into an at-ide.toml-style machine using the open AT BIOS and XUB.\n[[card]]\nid = \"ide\"\nkind = \"ide\"\nport = 0x1F0\nirq = 14\n\n[[card.drives]]\nchannel = \"master\"\nimage = \"{image}\"\ngeometry = {{ cylinders = {}, heads = {}, sectors = {} }}\nwrite = \"journal\"\njournal = \"{journal}\"\nkeep_generations = 10", geometry.cylinders, geometry.heads, geometry.sectors_per_track)?;
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

/// `OUT-kofN` beside `OUT`, keeping its extension: `game.img` → `game-2of3.img`.
fn set_member(output: &Path, disk: usize, disks: usize) -> io::Result<PathBuf> {
    let stem = output.file_stem().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("create {}: output needs a file name", output.display()),
        )
    })?;
    let mut name = stem.to_owned();
    name.push(format!("-{disk}of{disks}"));
    if let Some(extension) = output.extension() {
        name.push(".");
        name.push(extension);
    }
    Ok(output.with_file_name(name))
}

/// The outputs this command may replace that already exist: `OUT`, and with
/// `--split` any earlier set beside it (`<stem>-<k>of<n>.<ext>`). The scan
/// leaves them out should they sit inside a source tree.
fn existing_outputs(output: &Path, split: bool) -> Vec<PathBuf> {
    let mut paths = vec![output.to_owned()];
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if let (true, Some(stem), Ok(entries)) = (split, output.file_stem(), std::fs::read_dir(parent))
    {
        paths.extend(
            entries
                .flatten()
                .filter(|entry| is_set_member(&entry.file_name(), stem, output.extension()))
                .map(|entry| entry.path()),
        );
    }
    paths
}

/// `name` is `<stem>-<k>of<n>` plus `extension`, as [`set_member`] writes.
fn is_set_member(name: &OsStr, stem: &OsStr, extension: Option<&OsStr>) -> bool {
    let Some(rest) = name
        .as_encoded_bytes()
        .strip_prefix(stem.as_encoded_bytes())
        .and_then(|rest| rest.strip_prefix(b"-"))
    else {
        return false;
    };
    let rest = match extension {
        Some(extension) => rest
            .strip_suffix(extension.as_encoded_bytes())
            .and_then(|rest| rest.strip_suffix(b".")),
        None => Some(rest),
    };
    let number = |part: &[u8]| !part.is_empty() && part.iter().all(u8::is_ascii_digit);
    rest.and_then(|rest| {
        let at = rest.windows(2).position(|pair| pair == b"of")?;
        Some(number(&rest[..at]) && number(&rest[at + 2..]))
    })
    .unwrap_or(false)
}

/// What to suggest when the files do not fit one disk of `format`.
fn misfit_hint(format: FloppyFormat) -> &'static str {
    match format {
        FloppyFormat::Dd720 => {
            "; use --split for a set of disks, or --type 1.44M if the drive is a 1.44M one"
        }
        FloppyFormat::Dd360 => {
            "; use --split for a set of disks, or --type 1.2M if the drive is a 1.2M one"
        }
        FloppyFormat::Hd144 | FloppyFormat::Hd12 => "; use --split for a set of disks",
    }
}

/// Build every image in memory, check that no output exists (unless `force`),
/// then publish them in order. A misfit or an unreadable host file stops the
/// command before anything is written; a failure publishing a later disk of a
/// set names the disks already written.
fn execute_files(
    output_path: &Path,
    force: bool,
    format: FloppyFormat,
    label: Option<[u8; 11]>,
    files: FloppyFiles,
    output: &mut impl Write,
) -> io::Result<u8> {
    let request = Request {
        sources: files.sources,
        top: files.top,
        format,
        label,
        exclude: existing_outputs(output_path, files.split),
    };
    let failed = |error: BuildError| {
        let (kind, hint) = match &error {
            BuildError::Io { source, .. } => (source.kind(), ""),
            BuildError::DoesNotFit { .. } => (io::ErrorKind::InvalidInput, misfit_hint(format)),
            _ => (io::ErrorKind::InvalidInput, ""),
        };
        io::Error::new(
            kind,
            format!("create {}: {error}{hint}", output_path.display()),
        )
    };
    let plan = if files.split {
        media::fat12::plan_split(&request)
    } else {
        media::fat12::plan(&request)
    }
    .map_err(failed)?;
    let images = plan.images().map_err(failed)?;
    let paths = match plan.disks.len() {
        1 => vec![output_path.to_owned()],
        disks => (1..=disks)
            .map(|disk| set_member(output_path, disk, disks))
            .collect::<io::Result<_>>()?,
    };
    if !force {
        for path in &paths {
            match std::fs::symlink_metadata(path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "create {}: output exists; use --force to replace its directory entry",
                            path.display()
                        ),
                    ))
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("create {}: {error}", path.display()),
                    ))
                }
            }
        }
    }
    for (index, ((path, image), disk)) in paths.iter().zip(&images).zip(&plan.disks).enumerate() {
        journal_files::create::image(path, image.len() as u64, image, force).map_err(|error| {
            let written: Vec<_> = paths[..index]
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            let note = if written.is_empty() {
                String::new()
            } else {
                format!("; already written: {}", written.join(", "))
            };
            io::Error::new(
                error.kind(),
                format!("create {}: {error}{note}", path.display()),
            )
        })?;
        let label = disk
            .label
            .map(|label| {
                format!(
                    "label {}, ",
                    String::from_utf8_lossy(label.trim_ascii_end())
                )
            })
            .unwrap_or_default();
        let count = disk.files.len();
        writeln!(
            output,
            "created {} ({} bytes): {label}{count} file{}, {} bytes free",
            path.display(),
            image.len(),
            if count == 1 { "" } else { "s" },
            disk.free_bytes
        )?;
    }
    for rename in &plan.renames {
        writeln!(
            output,
            "renamed {} -> {}",
            rename.host.display(),
            rename.dos_path
        )?;
    }
    for skipped in &plan.skipped {
        writeln!(
            output,
            "skipped {}: {}",
            skipped.host.display(),
            skipped.reason
        )?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn requests_parse_into_the_image_they_describe() {
        let floppy = parse(&args(&["floppy", "--type", "720K", "--format", "a.img"])).unwrap();
        assert_eq!(floppy.output, PathBuf::from("a.img"));
        assert!(matches!(
            floppy.kind,
            ImageKind::Floppy {
                format: FloppyFormat::Dd720,
                formatted: true,
                label: None,
                files: None
            }
        ));
        let hd = parse(&args(&["hd", "--chs", "615/4/17", "c.img"])).unwrap();
        let ImageKind::HardDisk(geometry) = hd.kind else {
            panic!("expected a hard disk");
        };
        assert_eq!(
            (
                geometry.cylinders,
                geometry.heads,
                geometry.sectors_per_track
            ),
            (615, 4, 17)
        );
        assert_eq!(
            parse(&args(&["floppy", "a.img"])),
            Err("floppy requires --type")
        );
        assert_eq!(
            parse(&args(&["hd", "--chs", "1/1/1", "--size", "1MiB", "c.img"])),
            Err("hd requires exactly one of --chs C/H/S or --size SIZE[MiB|GiB]")
        );
    }

    #[test]
    fn execute_writes_an_image_of_the_requested_size_and_never_replaces_one() {
        let dir =
            std::env::temp_dir().join(format!("pc-image-media-create-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("blank.img");
        let request = || {
            let mut words = args(&["floppy", "--type", "360K"]);
            words.push(path.clone().into_os_string());
            parse(&words).unwrap()
        };
        let mut output = Vec::new();
        assert_eq!(execute(request(), &mut output).unwrap(), 0);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 368_640);
        assert!(String::from_utf8(output).unwrap().starts_with("created "));
        assert!(execute(request(), &mut io::sink()).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
