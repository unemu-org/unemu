// SPDX-License-Identifier: AGPL-3.0-or-later
//! Floppy catalogue drafts and blank-image forms for the desktop shell.
//!
//! The assembly boundary owns geometry and persistence. See
//! `backlog/docs/design/machine-images.md` §7 and persistent-media.md § Media changes.

use assembly::{DriveKind, FloppyGeometry, FloppyMedium, ImageSource, MachineConfig, WriteMode};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub(super) struct CatalogueEntry {
    pub medium: FloppyMedium,
    pub geometry: Option<FloppyGeometry>,
    pub reason: Option<String>,
    pub index: Option<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct FloppySlot {
    pub geometry: FloppyGeometry,
    pub entries: Vec<CatalogueEntry>,
    pub journal_retention: u32,
}

impl FloppySlot {
    pub fn from_config(config: &MachineConfig) -> [Option<Self>; 2] {
        std::array::from_fn(|unit| {
            let drive = config.drives.iter().find(|drive| {
                drive.controller == assembly::DeviceId::Fdc && usize::from(drive.unit) == unit
            })?;
            let DriveKind::Floppy { geometry } = drive.kind else {
                return None;
            };
            let media = drive
                .media
                .as_ref()
                .map(|list| list.entries.clone())
                .unwrap_or_else(|| {
                    vec![FloppyMedium {
                        image: drive.image.clone(),
                        expected_sha256: drive.expected_sha256,
                        write: drive.write.clone(),
                        name: match &drive.image {
                            ImageSource::File(path) => filename(path),
                            _ => "image".into(),
                        },
                    }]
                });
            let journal_retention = media
                .iter()
                .find_map(|medium| match medium.write {
                    WriteMode::Journal {
                        keep_generations, ..
                    } => Some(keep_generations),
                    _ => None,
                })
                .unwrap_or(WriteMode::DEFAULT_KEEP_GENERATIONS);
            let entries = media
                .into_iter()
                .enumerate()
                .map(|(index, medium)| CatalogueEntry {
                    medium,
                    geometry: Some(geometry),
                    reason: None,
                    index: Some(index),
                })
                .collect();
            Some(Self {
                geometry,
                entries,
                journal_retention,
            })
        })
    }
}

pub(super) fn filename(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

pub(super) fn geometry_name(geometry: FloppyGeometry) -> String {
    assembly::descriptors::FLOPPY_TYPES
        .iter()
        .find(|kind| kind.geometry == geometry)
        .map_or_else(
            || {
                format!(
                    "{}/{}/{}",
                    geometry.cylinders, geometry.heads, geometry.sectors_per_track
                )
            },
            |kind| kind.name.into(),
        )
}

pub(super) fn compatibility(
    geometry: Option<FloppyGeometry>,
    drive: FloppyGeometry,
) -> Option<String> {
    match geometry {
        None => Some("Unrecognised image size; expected 1.2M, 1.44M, 720K or 360K".into()),
        Some(geometry) if geometry != drive => Some(format!(
            "{} image is incompatible with a {} drive; mixed-density mechanics are unavailable",
            geometry_name(geometry),
            geometry_name(drive)
        )),
        Some(_) => None,
    }
}

pub(super) fn browse(path: &Path, drive: FloppyGeometry) -> io::Result<CatalogueEntry> {
    let path = path.canonicalize()?;
    let file = std::fs::File::open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("image must be a regular file"));
    }
    let geometry = assembly::descriptors::FLOPPY_TYPES
        .iter()
        .find(|kind| kind.geometry.image_bytes() == metadata.len())
        .map(|kind| kind.geometry);
    let reason = compatibility(geometry, drive);
    let expected_sha256 = if geometry.is_some() {
        Some(media::VerifiedBase::from_file(file)?.identity().base_digest)
    } else {
        None
    };
    Ok(CatalogueEntry {
        medium: FloppyMedium {
            image: ImageSource::File(path.to_path_buf()),
            expected_sha256,
            write: WriteMode::ReadOnly,
            name: filename(&path),
        },
        geometry,
        reason,
        index: None,
    })
}

pub(super) fn write_name(mode: &WriteMode) -> &'static str {
    match mode {
        WriteMode::ReadOnly => "Read-only",
        WriteMode::Journal { .. } => "Journal",
        WriteMode::Volatile => "Volatile (machine file)",
    }
}

pub(super) struct InsertDialog {
    pub unit: u8,
    pub slot: FloppySlot,
    pub selected: Option<usize>,
    pub journal: bool,
    pub journal_path: String,
    pub error: Option<String>,
}

impl InsertDialog {
    pub fn new(unit: u8, slot: FloppySlot) -> Self {
        Self {
            unit,
            slot,
            selected: None,
            journal: false,
            journal_path: String::new(),
            error: None,
        }
    }

    pub fn select(&mut self, index: Option<usize>) {
        self.selected = index;
        self.journal = false;
        self.error = None;
        self.journal_path = index
            .and_then(|index| self.slot.entries.get(index))
            .and_then(|entry| {
                if let ImageSource::File(base) = &entry.medium.image {
                    let mut path = base.as_os_str().to_os_string();
                    path.push(".upfj");
                    Some(path.to_string_lossy().into_owned())
                } else {
                    None
                }
            })
            .unwrap_or_default();
    }

    pub fn selection(&self) -> Result<CatalogueEntry, String> {
        let mut entry = self
            .selected
            .and_then(|index| self.slot.entries.get(index))
            .cloned()
            .ok_or("Select an image")?;
        if let Some(reason) = &entry.reason {
            return Err(reason.clone());
        }
        if entry.index.is_none() && self.journal {
            let ImageSource::File(base) = &entry.medium.image else {
                return Err("Journal mode requires a file-backed base".into());
            };
            let keep_generations = self.slot.journal_retention;
            if self.journal_path.trim().is_empty() {
                return Err("Choose a separate journal path".into());
            }
            entry.medium.write = WriteMode::Journal {
                path: resolve_input(
                    base.parent().unwrap_or(Path::new(".")),
                    self.journal_path.trim(),
                ),
                keep_generations,
            };
        }
        Ok(entry)
    }
}

pub(super) fn resolve_input(directory: &Path, input: &str) -> PathBuf {
    directory.join(input)
}

pub(super) struct BlankDisk {
    pub directory: PathBuf,
    pub output: String,
    pub format: usize,
    pub chs: String,
    pub size: String,
    pub by_size: bool,
    pub error: Option<String>,
}

impl Default for BlankDisk {
    fn default() -> Self {
        Self {
            directory: crate::default_machine()
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf(),
            output: String::new(),
            format: 0,
            chs: "615/4/17".into(),
            size: "20MiB".into(),
            by_size: false,
            error: None,
        }
    }
}

impl BlankDisk {
    pub const FORMATS: [&'static str; 5] = ["Fixed disk", "1.2M", "1.44M", "720K", "360K"];

    pub fn for_machine(machine: &Path, warning: Option<&str>) -> Self {
        use machine_file::schema::v1::{DriveType, Geometry};
        let machine = machine
            .canonicalize()
            .or_else(|_| std::path::absolute(machine))
            .unwrap_or_else(|_| machine.to_path_buf());
        let form = Self {
            directory: machine.parent().unwrap_or(Path::new(".")).to_path_buf(),
            ..Default::default()
        };
        let Ok((document, origins)) = machine_file::resolve(&machine) else {
            return form;
        };
        let mut missing = None;
        for (card_index, card) in document.card.iter().flatten().enumerate() {
            for (drive_index, drive) in card.drives.iter().flatten().enumerate() {
                let (format, chs) = if let Some(kind) = drive.drive_type {
                    (
                        match kind {
                            DriveType::Hd12 => 1,
                            DriveType::Hd144 => 2,
                            DriveType::Dd720 => 3,
                            DriveType::Dd360 => 4,
                        },
                        form.chs.clone(),
                    )
                } else if let Some(Geometry::Chs(chs)) = &drive.geometry {
                    let (Some(c), Some(h), Some(s)) = (chs.cylinders, chs.heads, chs.sectors)
                    else {
                        continue;
                    };
                    (0, format!("{c}/{h}/{s}"))
                } else {
                    continue;
                };
                let field = format!("card[{card_index}].drives[{drive_index}]");
                let images = drive
                    .image
                    .iter()
                    .map(|image| (format!("{field}.image"), image.as_str()))
                    .chain(
                        drive
                            .media
                            .iter()
                            .flatten()
                            .enumerate()
                            .map(|(index, medium)| {
                                (
                                    format!("{field}.media[{index}].image"),
                                    medium.image.as_str(),
                                )
                            }),
                    );
                for (field, image) in images {
                    let source = &origins.get(&field).file;
                    let source = source.canonicalize().unwrap_or_else(|_| source.clone());
                    let path = resolve_input(source.parent().unwrap_or(&form.directory), image);
                    let candidate = Self {
                        directory: form.directory.clone(),
                        output: path.to_string_lossy().into_owned(),
                        format,
                        chs: chs.clone(),
                        ..Default::default()
                    };
                    if warning.is_some_and(|warning| {
                        warning.contains(image) || warning.contains(path.to_string_lossy().as_ref())
                    }) {
                        return candidate;
                    }
                    if missing.is_none() && !path.is_file() {
                        missing = Some(candidate);
                    }
                }
            }
        }
        missing.unwrap_or(form)
    }

    pub fn create(&self) -> Result<(), String> {
        let command = self.command().map_err(str::to_owned)?;
        cli::create::execute(command, &mut io::sink())
            .map(|_| ())
            .map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    "File exists; choose a new name".into()
                } else {
                    error.to_string()
                }
            })
    }

    pub fn command(&self) -> Result<cli::create::CreateCommand, &'static str> {
        use std::ffi::OsString;
        if self.output.trim().is_empty() {
            return Err("Choose an output path");
        }
        let output = resolve_input(&self.directory, self.output.trim()).into_os_string();
        let args: Vec<OsString> = if self.format == 0 {
            vec![
                "hd".into(),
                if self.by_size { "--size" } else { "--chs" }.into(),
                if self.by_size {
                    self.size.as_str()
                } else {
                    self.chs.as_str()
                }
                .into(),
                output,
            ]
        } else {
            let format = Self::FORMATS
                .get(self.format)
                .ok_or("Unknown floppy format")?;
            vec![
                "floppy".into(),
                "--type".into(),
                format.into(),
                "--format".into(),
                output,
            ]
        };
        cli::create::parse(&args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_reasons_name_both_image_and_drive_formats() {
        for image in assembly::descriptors::FLOPPY_TYPES {
            for drive in assembly::descriptors::FLOPPY_TYPES {
                let reason = compatibility(Some(image.geometry), drive.geometry);
                assert_eq!(reason.is_none(), image == drive);
                if let Some(reason) = reason {
                    assert!(reason.contains(image.name));
                    assert!(reason.contains(drive.name));
                }
            }
        }
        assert!(compatibility(None, FloppyGeometry::HD_1_2)
            .unwrap()
            .contains("size"));
    }

    #[test]
    fn browse_pins_every_byte_and_disables_incompatible_images() {
        use sha2::{Digest, Sha256};
        let temp = assembly::test_support::TempDir::new("browse-pins");
        let path = temp.path().join("image.img");
        let mut bytes = vec![0; FloppyGeometry::HD_1_44.image_bytes() as usize];
        let last = bytes.len() - 1;
        bytes[last] = 7;
        std::fs::write(&path, &bytes).unwrap();
        let entry = browse(&path, FloppyGeometry::HD_1_2).unwrap();
        assert_eq!(
            entry.medium.expected_sha256,
            Some(Sha256::digest(&bytes).into())
        );
        assert!(entry.reason.unwrap().contains("1.44M"));
        assert_eq!(entry.index, None);
        assert_eq!(entry.medium.write, WriteMode::ReadOnly);
        std::fs::write(&path, [0; 3]).unwrap();
        assert!(browse(&path, FloppyGeometry::HD_1_2)
            .unwrap()
            .reason
            .is_some());
    }

    #[test]
    fn relative_journal_paths_resolve_beside_the_selected_base() {
        let temp = assembly::test_support::TempDir::new("journal-directory");
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("disk.img");
        std::fs::write(&path, vec![0; 1_474_560]).unwrap();
        let mut slot =
            FloppySlot::from_config(&assembly::floppy_boot(vec![0xEB, 0xFE], vec![0; 1_474_560]))
                [0]
            .clone()
            .unwrap();
        slot.entries = vec![browse(&path, slot.geometry).unwrap()];
        let mut draft = InsertDialog::new(0, slot);
        draft.selected = Some(0);
        draft.journal = true;
        draft.journal_path = "changes.upfj".into();
        let WriteMode::Journal { path: actual, .. } = draft.selection().unwrap().medium.write
        else {
            panic!("journal mode");
        };
        assert_eq!(actual, root.join("changes.upfj"));
    }

    #[test]
    fn selected_browsed_image_defaults_to_an_adjacent_journal() {
        let temp = assembly::test_support::TempDir::new("journal-default");
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("disk.img");
        std::fs::write(&path, vec![0; 1_474_560]).unwrap();
        let mut slot =
            FloppySlot::from_config(&assembly::floppy_boot(vec![0xEB, 0xFE], vec![0; 1_474_560]))
                [0]
            .clone()
            .unwrap();
        slot.entries = vec![browse(&path, slot.geometry).unwrap()];
        let mut draft = InsertDialog::new(0, slot);
        draft.select(Some(0));
        assert_eq!(Path::new(&draft.journal_path), root.join("disk.img.upfj"));
        draft.journal = true;
        let WriteMode::Journal { path: actual, .. } = draft.selection().unwrap().medium.write
        else {
            panic!("journal mode");
        };
        assert_eq!(actual, root.join("disk.img.upfj"));
    }

    #[test]
    fn blank_disk_paths_and_prefills_follow_the_machine_and_defining_layer() {
        let temp = assembly::test_support::TempDir::new("blank-prefill");
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("child")).unwrap();
        let parent = root.join("base.toml");
        std::fs::write(&parent, "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='missing.bin'\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\nimage='local/games.img'\ngeometry={cylinders=321,heads=8,sectors=17}\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='720K'\n[[card.drives.media]]\nimage='local/transfer.img'\nwrite='readonly'\n").unwrap();
        let machine = root.join("child/example.toml");
        std::fs::write(&machine, "schema=1\nextends='../base.toml'\n").unwrap();
        machine_file::resolve(&machine).unwrap();
        let mut form = BlankDisk::for_machine(&machine, Some("cannot read image local/games.img"));
        assert_eq!(form.directory, root.join("child"));
        assert_eq!(Path::new(&form.output), root.join("local/games.img"));
        assert_eq!(form.format, 0);
        assert_eq!(form.chs, "321/8/17");
        assert_eq!(form.command().unwrap().output, root.join("local/games.img"));
        form.output = "new.img".into();
        assert_eq!(form.command().unwrap().output, root.join("child/new.img"));
        form.output = root.join("absolute.img").to_string_lossy().into_owned();
        assert_eq!(form.command().unwrap().output, root.join("absolute.img"));
        let floppy = BlankDisk::for_machine(&machine, Some("cannot read image local/transfer.img"));
        assert_eq!(floppy.format, 3);
        assert_eq!(
            floppy.command().unwrap().output,
            root.join("local/transfer.img")
        );
        assert!(matches!(
            floppy.command().unwrap().kind,
            cli::create::ImageKind::Floppy {
                format: cli::create::FloppyFormat::Dd720,
                formatted: true,
                ..
            }
        ));
    }

    #[test]
    fn existing_blank_disk_output_uses_a_gui_message_and_preserves_the_file() {
        let temp = assembly::test_support::TempDir::new("blank-existing");
        let path = temp.path().join("existing.img");
        std::fs::write(&path, b"existing base").unwrap();
        let form = BlankDisk {
            directory: temp.path().to_path_buf(),
            output: "existing.img".into(),
            format: 1,
            ..Default::default()
        };
        assert_eq!(form.create().unwrap_err(), "File exists; choose a new name");
        assert_eq!(std::fs::read(path).unwrap(), b"existing base");
    }

    #[test]
    fn journal_requires_a_file_backed_base_and_an_explicit_path() {
        let mut slot =
            FloppySlot::from_config(&assembly::floppy_boot(vec![0xEB, 0xFE], vec![0; 1_474_560]))
                [0]
            .clone()
            .unwrap();
        slot.entries[0].index = None;
        let mut draft = InsertDialog::new(0, slot);
        draft.selected = Some(0);
        draft.journal = true;
        draft.journal_path = "changes.upfj".into();
        assert!(draft.selection().unwrap_err().contains("file-backed"));
        draft.slot.journal_retention = 3;
        draft.slot.entries[0].medium.image = ImageSource::File("image.img".into());
        draft.journal_path.clear();
        assert!(draft.selection().unwrap_err().contains("path"));
        draft.journal_path = "changes.upfj".into();
        assert!(matches!(
            draft.selection().unwrap().medium.write,
            WriteMode::Journal {
                keep_generations: 3,
                ..
            }
        ));
    }

    #[test]
    fn blank_disk_form_uses_existing_chs_size_and_formatted_floppy_creation() {
        let temp = assembly::test_support::TempDir::new("blank-dialog");
        let mut form = BlankDisk {
            output: temp.path().join("fixed.img").to_string_lossy().into_owned(),
            ..Default::default()
        };
        assert!(matches!(
            form.command().unwrap().kind,
            cli::create::ImageKind::HardDisk(_)
        ));
        form.chs = "0/4/17".into();
        assert!(form.command().is_err());
        form.by_size = true;
        assert!(form.command().is_ok());
        for format in 1..5 {
            form.format = format;
            form.output = temp
                .path()
                .join(format!("{format}.img"))
                .to_string_lossy()
                .into_owned();
            cli::create::execute(form.command().unwrap(), &mut io::sink()).unwrap();
            let bytes = std::fs::read(&form.output).unwrap();
            assert_eq!(&bytes[510..512], &[0x55, 0xAA]);
            assert_eq!(&bytes[54..62], b"FAT12   ");
            assert!(cli::create::execute(form.command().unwrap(), &mut io::sink()).is_err());
        }
    }
}
