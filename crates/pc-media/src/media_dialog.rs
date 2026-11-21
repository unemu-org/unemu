// SPDX-License-Identifier: AGPL-3.0-or-later
//! Floppy catalogue drafts and blank-image forms for the desktop shell.
//!
//! The assembly boundary owns geometry and persistence. See
//! `backlog/docs/design/machine-images.md` §7 and persistent-media.md § Media changes.

use assembly::{DriveKind, FloppyGeometry, FloppyMedium, ImageSource, MachineConfig, WriteMode};
use machine_file::FileMachineExt;
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct CatalogueEntry {
    pub source: Option<image_store::media_set::CatalogueSource>,
    pub receipt: Option<std::sync::Arc<image_store::ResolvedImage>>,
    pub readonly_bytes: Option<std::sync::Arc<Vec<u8>>>,
    pub medium: FloppyMedium,
    pub geometry: Option<FloppyGeometry>,
    pub reason: Option<String>,
    /// Drive catalogue entry currently used for this image and write policy.
    pub index: Option<usize>,
    /// Declared by the machine file, whose write policy the dialog never changes.
    /// Images added during the session (a browsed file, a library set) may switch.
    pub declared: bool,
    /// Earlier catalogue entries of this image under another write policy. The
    /// drive catalogue only grows, so switching back reuses one of these.
    pub alternates: Vec<(WriteMode, usize)>,
    /// Journal location for library media: inside the machine workspace,
    /// never beside the store's immutable blob.
    pub workspace_journal: Option<PathBuf>,
    /// The media set and disk position of a library disk, as the status
    /// bar's drive tooltip names its source.
    pub origin: Option<String>,
    /// A disk Create from files… made, hashed in full off the window's
    /// thread, so it goes into a running drive without hashing it again.
    pub verified: Option<crate::created_media::Verified>,
    /// The tag a library disk came from, which a library machine keeps
    /// with its catalogue at Stop.
    pub reference: Option<String>,
}

impl CatalogueEntry {
    /// The same session image: one library medium, or one file with one SHA-256.
    fn same_image(&self, other: &Self) -> bool {
        match (&self.source, &other.source) {
            (Some(left), Some(right)) => left.same_medium(right),
            (None, None) => {
                self.medium.expected_sha256.is_some()
                    && self.medium.expected_sha256 == other.medium.expected_sha256
                    && same_file(&self.medium.image, &other.medium.image)
            }
            _ => false,
        }
    }
}

fn same_file(left: &ImageSource, right: &ImageSource) -> bool {
    let (ImageSource::File(left), ImageSource::File(right)) = (left, right) else {
        return false;
    };
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(left, right)| left == right)
}

#[derive(Clone, Debug)]
pub struct FloppySlot {
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
                    source: None,
                    receipt: None,
                    readonly_bytes: None,
                    geometry: Some(medium_format(geometry, &medium.image)),
                    medium,
                    reason: None,
                    index: Some(index),
                    declared: true,
                    alternates: Vec::new(),
                    workspace_journal: None,
                    origin: None,
                    verified: None,
                    reference: None,
                })
                .collect();
            Some(Self {
                geometry,
                entries,
                journal_retention,
            })
        })
    }

    /// Record the catalogue entry an insertion used. Each session image keeps
    /// one row; a policy switch moves the previous entry to its alternates.
    fn record(&mut self, entry: &CatalogueEntry, index: usize) {
        if entry.declared {
            return;
        }
        let Some(row) = self
            .entries
            .iter_mut()
            .find(|row| !row.declared && row.same_image(entry))
        else {
            self.entries.push(CatalogueEntry {
                index: Some(index),
                alternates: Vec::new(),
                ..entry.clone()
            });
            return;
        };
        // The same disk may arrive again through another set tag.
        if entry.origin.is_some() {
            row.origin.clone_from(&entry.origin);
            row.reference.clone_from(&entry.reference);
        }
        if row.index == Some(index) {
            return;
        }
        if let Some(previous) = row.index {
            row.alternates.push((row.medium.write.clone(), previous));
        }
        row.alternates.retain(|(_, alternate)| *alternate != index);
        row.index = Some(index);
        row.medium.write = entry.medium.write.clone();
    }
}

pub fn filename(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

pub fn geometry_name(geometry: FloppyGeometry) -> String {
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

/// The format of a catalogue medium in a drive of type `drive`: the accepted
/// medium its image size gives, else the drive's own format.
pub(super) fn medium_format(drive: FloppyGeometry, image: &ImageSource) -> FloppyGeometry {
    image
        .known_len()
        .or_else(|| match image {
            ImageSource::File(path) => std::fs::metadata(path).ok().map(|m| m.len()),
            _ => None,
        })
        .and_then(|len| drive.medium_for_len(len))
        .unwrap_or(drive)
}

/// Why an image of `geometry` cannot go into a drive of type `drive`, by the
/// assembly rule ([`FloppyGeometry::accepted_media`]).
pub(super) fn compatibility(
    geometry: Option<FloppyGeometry>,
    drive: FloppyGeometry,
) -> Option<String> {
    match geometry {
        None => Some("Unrecognised image size; expected 1.2M, 1.44M, 720K or 360K".into()),
        Some(geometry) if !drive.accepted_media().contains(&geometry) => Some(format!(
            "{} image does not fit: {}",
            geometry_name(geometry),
            assembly::descriptors::floppy_drive_media(drive)
        )),
        Some(_) => None,
    }
}

pub fn browse(path: &Path, drive: FloppyGeometry) -> io::Result<CatalogueEntry> {
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
        Some(
            journal_files::VerifiedBase::from_file(file)?
                .identity()
                .base_digest,
        )
    } else {
        None
    };
    Ok(CatalogueEntry {
        source: None,
        receipt: None,
        readonly_bytes: None,
        medium: FloppyMedium {
            image: ImageSource::File(path.to_path_buf()),
            expected_sha256,
            write: WriteMode::ReadOnly,
            name: filename(&path),
        },
        geometry,
        reason,
        index: None,
        declared: false,
        alternates: Vec::new(),
        workspace_journal: None,
        origin: None,
        verified: None,
        reference: None,
    })
}

pub fn write_name(mode: &WriteMode) -> &'static str {
    match mode {
        WriteMode::ReadOnly => "Read-only",
        WriteMode::Journal { .. } => "Journal",
        WriteMode::Volatile => "Volatile (machine file)",
    }
}

pub struct InsertDialog {
    pub library: Option<LibraryInsert>,
    pub unit: u8,
    pub slot: FloppySlot,
    pub selected: Option<usize>,
    pub journal: bool,
    pub journal_path: String,
    pub error: Option<String>,
    /// The media picker chose the set, disk and write setting: the shell
    /// inserts without showing this dialog once the set is verified.
    pub automatic: bool,
    /// Browse for an image file… of the media picker: the dialog shows only
    /// the chosen file's step (geometry, write mode, compatibility).
    pub browsed_only: bool,
}

impl InsertDialog {
    pub fn new(unit: u8, slot: FloppySlot) -> Self {
        Self {
            library: None,
            unit,
            slot,
            selected: None,
            journal: false,
            journal_path: String::new(),
            error: None,
            automatic: false,
            browsed_only: false,
        }
    }

    /// Selecting a session image shows its current write policy, so Insert
    /// without changes re-inserts the same catalogue entry. A journal used
    /// earlier for the image is the default journal path again.
    pub fn select(&mut self, index: Option<usize>) {
        self.selected = index;
        self.error = None;
        let entry = index
            .and_then(|index| self.slot.entries.get(index))
            .filter(|entry| !entry.declared);
        let journal_of = |write: &WriteMode| match write {
            WriteMode::Journal { path, .. } => Some(path.clone()),
            _ => None,
        };
        let current = entry.and_then(|entry| journal_of(&entry.medium.write));
        self.journal = current.is_some();
        self.journal_path = current
            .or_else(|| {
                entry.and_then(|entry| {
                    entry
                        .alternates
                        .iter()
                        .find_map(|(write, _)| journal_of(write))
                })
            })
            .or_else(|| {
                entry.and_then(|entry| match &entry.medium.image {
                    ImageSource::File(base) => {
                        let mut path = base.as_os_str().to_os_string();
                        path.push(".upfj");
                        Some(PathBuf::from(path))
                    }
                    _ => None,
                })
            })
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
    }

    /// Browse… reuses the row of an image it already added this session (same
    /// file and SHA-256). A machine-file row is never reused, so its image can
    /// still be added again with another write policy.
    pub fn add_browsed(&mut self, entry: CatalogueEntry) {
        let index = self
            .slot
            .entries
            .iter()
            .position(|row| !row.declared && row.same_image(&entry))
            .unwrap_or_else(|| {
                self.slot.entries.push(entry);
                self.slot.entries.len() - 1
            });
        let compatible = self.slot.entries[index].reason.is_none();
        self.select(compatible.then_some(index));
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
        if entry.declared {
            return Ok(entry);
        }
        let keep_generations = self.slot.journal_retention;
        let write = if !self.journal {
            WriteMode::ReadOnly
        } else if let Some(path) = &entry.workspace_journal {
            WriteMode::Journal {
                path: path.clone(),
                keep_generations,
            }
        } else {
            let ImageSource::File(base) = &entry.medium.image else {
                return Err("Journal mode requires a file-backed base".into());
            };
            if self.journal_path.trim().is_empty() {
                return Err("Choose a separate journal path".into());
            }
            WriteMode::Journal {
                path: resolve_input(
                    base.parent().unwrap_or(Path::new(".")),
                    self.journal_path.trim(),
                ),
                keep_generations,
            }
        };
        if write != entry.medium.write {
            // The drive catalogue only grows: another policy is either an
            // earlier entry of this image or a new one appended on Insert.
            entry.index = entry
                .alternates
                .iter()
                .find(|(mode, _)| *mode == write)
                .map(|(_, index)| *index);
            entry.medium.write = write;
        }
        Ok(entry)
    }
}

pub fn resolve_input(directory: &Path, input: &str) -> PathBuf {
    directory.join(input)
}

pub struct BlankDisk {
    pub directory: PathBuf,
    pub output: String,
    pub format: usize,
    pub chs: String,
    pub size: String,
    pub by_size: bool,
    pub error: Option<String>,
}

impl BlankDisk {
    /// An empty form whose typed paths resolve in `directory`, the machine
    /// files' folder, or in the working directory when there is none.
    pub fn new(directory: Option<PathBuf>) -> Self {
        Self {
            directory: directory.unwrap_or_else(|| PathBuf::from(".")),
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
            ..Self::new(None)
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
                        ..Self::new(None)
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
        pc_image::media_create::execute(command, &mut io::sink())
            .map(|_| ())
            .map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    "File exists; choose a new name".into()
                } else {
                    error.to_string()
                }
            })
    }

    pub fn command(&self) -> Result<pc_image::media_create::CreateCommand, &'static str> {
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
        pc_image::media_create::parse(&args)
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
                use assembly::descriptors::FloppyType;
                let mixed = (drive == &FloppyType::HD_1_44 && image == &FloppyType::DD_720K)
                    || (drive == &FloppyType::HD_1_2 && image == &FloppyType::DD_360K);
                assert_eq!(reason.is_none(), image == drive || mixed);
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
    fn browse_accepts_any_medium_the_drive_type_takes() {
        let temp = assembly::test_support::TempDir::new("browse-mixed-density");
        for (image, drive, fits) in [
            (FloppyGeometry::DD_720K, FloppyGeometry::HD_1_44, true),
            (FloppyGeometry::DD_360K, FloppyGeometry::HD_1_2, true),
            (FloppyGeometry::DD_720K, FloppyGeometry::DD_720K, true),
            (FloppyGeometry::HD_1_44, FloppyGeometry::DD_720K, false),
            (FloppyGeometry::DD_360K, FloppyGeometry::HD_1_44, false),
            (FloppyGeometry::DD_720K, FloppyGeometry::HD_1_2, false),
        ] {
            let path = temp.path().join("image.img");
            std::fs::write(&path, vec![0; image.image_bytes() as usize]).unwrap();
            let entry = browse(&path, drive).unwrap();
            assert_eq!(entry.geometry, Some(image));
            assert_eq!(entry.reason.is_none(), fits, "{image:?} in {drive:?}");
            if let Some(reason) = entry.reason {
                assert!(reason.contains(&geometry_name(drive)), "{reason}");
                assert!(reason.contains(&geometry_name(image)), "{reason}");
            }
        }
        // A declared catalogue entry shows its own format, not the drive's.
        let path = temp.path().join("game-720.img");
        std::fs::write(&path, vec![0; 737_280]).unwrap();
        assert_eq!(
            medium_format(FloppyGeometry::HD_1_44, &ImageSource::File(path)),
            FloppyGeometry::DD_720K
        );
        assert_eq!(
            medium_format(
                FloppyGeometry::HD_1_44,
                &ImageSource::Blank { size: 1_474_560 }
            ),
            FloppyGeometry::HD_1_44
        );
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
            pc_image::media_create::ImageKind::Floppy {
                format: pc_image::media_create::FloppyFormat::Dd720,
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
            ..BlankDisk::new(None)
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
        slot.entries[0].declared = false;
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

    fn insert(
        assembled: &mut assembly::AssembledMachine,
        config: &mut MachineConfig,
        slots: &mut [Option<FloppySlot>; 2],
        draft: &InsertDialog,
    ) -> usize {
        let entry = draft.selection().unwrap();
        append_entries(assembled, config, slots, 0, &[entry]).unwrap()[0]
    }

    fn catalogue_len(assembled: &assembly::AssembledMachine) -> usize {
        assembled.floppy_names[&(assembly::DeviceId::Fdc, 0)].len()
    }

    #[test]
    fn session_images_switch_write_policy_and_machine_file_entries_stay_fixed() {
        let temp = assembly::test_support::TempDir::new("session-write-policy");
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("extra.img");
        std::fs::write(&path, vec![0; 1_474_560]).unwrap();
        let mut config = assembly::floppy_boot(vec![0xEB, 0xFE], vec![0; 1_474_560]);
        let mut assembled = machine_file::assemble(&config).unwrap();
        let mut slots = FloppySlot::from_config(&config);
        let dialog =
            |slots: &[Option<FloppySlot>; 2]| InsertDialog::new(0, slots[0].clone().unwrap());

        let mut draft = dialog(&slots);
        draft.add_browsed(browse(&path, FloppyGeometry::HD_1_44).unwrap());
        assert_eq!(draft.selected, Some(1));
        assert!(!draft.journal);
        assert_eq!(insert(&mut assembled, &mut config, &mut slots, &draft), 1);
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 2);
        assert!(!slots[0].as_ref().unwrap().entries[1].declared);

        // Re-inserted, the browsed image may now keep its changes in a journal.
        let mut draft = dialog(&slots);
        draft.select(Some(1));
        assert!(!draft.journal);
        draft.journal = true;
        draft.journal_path = "extra.upfj".into();
        assert_eq!(insert(&mut assembled, &mut config, &mut slots, &draft), 2);
        let row = &slots[0].as_ref().unwrap().entries[1];
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 2);
        assert_eq!(row.index, Some(2));
        assert_eq!(
            row.medium.write,
            WriteMode::journal(root.join("extra.upfj"))
        );
        assert_eq!(row.alternates, [(WriteMode::ReadOnly, 1)]);

        // Back to read-only reuses the earlier catalogue entry, and the journal
        // policy is remembered for the next switch.
        let mut draft = dialog(&slots);
        draft.select(Some(1));
        assert!(draft.journal);
        assert_eq!(Path::new(&draft.journal_path), root.join("extra.upfj"));
        draft.journal = false;
        assert_eq!(insert(&mut assembled, &mut config, &mut slots, &draft), 1);
        let mut draft = dialog(&slots);
        draft.select(Some(1));
        assert!(!draft.journal);
        assert_eq!(Path::new(&draft.journal_path), root.join("extra.upfj"));
        draft.journal = true;
        assert_eq!(insert(&mut assembled, &mut config, &mut slots, &draft), 2);
        assert_eq!(catalogue_len(&assembled), 3);
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 2);

        // The machine file's own entry keeps its declared policy.
        let mut draft = dialog(&slots);
        draft.select(Some(0));
        assert!(slots[0].as_ref().unwrap().entries[0].declared);
        draft.journal = true;
        draft.journal_path = "declared.upfj".into();
        let entry = draft.selection().unwrap();
        assert_eq!(entry.index, Some(0));
        assert_eq!(entry.medium.write, config.drives[0].write);
        assert_eq!(insert(&mut assembled, &mut config, &mut slots, &draft), 0);
        assert_eq!(catalogue_len(&assembled), 3);
        assert!(!root.join("declared.upfj").exists());
    }

    #[test]
    fn browsing_the_same_image_again_reuses_its_catalogue_entry() {
        let temp = assembly::test_support::TempDir::new("browse-reuse");
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("disk.img");
        let copy = root.join("copy.img");
        std::fs::write(&path, vec![0; 1_474_560]).unwrap();
        std::fs::write(&copy, vec![0; 1_474_560]).unwrap();
        let mut config = assembly::floppy_boot(vec![0xEB, 0xFE], vec![0; 1_474_560]);
        let mut assembled = machine_file::assemble(&config).unwrap();
        let mut slots = FloppySlot::from_config(&config);
        let mut draft = InsertDialog::new(0, slots[0].clone().unwrap());
        for _ in 0..2 {
            draft.add_browsed(browse(&path, FloppyGeometry::HD_1_44).unwrap());
            assert_eq!(draft.slot.entries.len(), 2);
            assert_eq!(draft.selected, Some(1));
        }
        // The same path through another spelling is the same image.
        draft.add_browsed(
            browse(&root.join(".").join("disk.img"), FloppyGeometry::HD_1_44).unwrap(),
        );
        assert_eq!(draft.slot.entries.len(), 2);
        draft.journal = true;
        draft.journal_path = "disk.upfj".into();
        assert_eq!(insert(&mut assembled, &mut config, &mut slots, &draft), 1);

        let mut draft = InsertDialog::new(0, slots[0].clone().unwrap());
        draft.add_browsed(browse(&path, FloppyGeometry::HD_1_44).unwrap());
        assert_eq!(draft.slot.entries.len(), 2);
        assert_eq!(draft.selected, Some(1));
        assert!(draft.journal);
        assert_eq!(insert(&mut assembled, &mut config, &mut slots, &draft), 1);
        assert_eq!(catalogue_len(&assembled), 2);

        // Equal bytes under another path, or changed bytes under the same
        // path, are different images.
        draft.add_browsed(browse(&copy, FloppyGeometry::HD_1_44).unwrap());
        assert_eq!(draft.slot.entries.len(), 3);
        std::fs::write(&path, vec![1; 1_474_560]).unwrap();
        draft.add_browsed(browse(&path, FloppyGeometry::HD_1_44).unwrap());
        assert_eq!(draft.slot.entries.len(), 4);
        assert_eq!(draft.selected, Some(3));
    }

    #[test]
    fn browsing_a_machine_file_image_adds_a_session_row_that_may_journal() {
        use sha2::{Digest, Sha256};
        let temp = assembly::test_support::TempDir::new("browse-declared");
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("declared.img");
        std::fs::write(&path, vec![0; 1_474_560]).unwrap();
        let mut config = assembly::floppy_boot(vec![0xEB, 0xFE], vec![0; 1_474_560]);
        config.drives[0].image = ImageSource::File(path.clone());
        config.drives[0].expected_sha256 = Some(Sha256::digest(vec![0; 1_474_560]).into());
        config.drives[0].write = WriteMode::ReadOnly;
        let mut assembled = machine_file::assemble(&config).unwrap();
        let mut slots = FloppySlot::from_config(&config);
        let declared = &slots[0].as_ref().unwrap().entries[0];
        assert!(declared.declared);
        let browsed = browse(&path, FloppyGeometry::HD_1_44).unwrap();
        assert!(declared.same_image(&browsed));

        let mut draft = InsertDialog::new(0, slots[0].clone().unwrap());
        draft.add_browsed(browsed);
        assert_eq!(draft.slot.entries.len(), 2);
        assert_eq!(draft.selected, Some(1));
        draft.journal = true;
        assert_eq!(
            Path::new(&draft.journal_path),
            root.join("declared.img.upfj")
        );
        assert_eq!(insert(&mut assembled, &mut config, &mut slots, &draft), 1);
        let rows = &slots[0].as_ref().unwrap().entries;
        assert_eq!(rows[0].medium.write, WriteMode::ReadOnly);
        assert_eq!(
            rows[1].medium.write,
            WriteMode::journal(root.join("declared.img.upfj"))
        );
        // Browsing it again now finds the session row, not the declared one.
        let mut draft = InsertDialog::new(0, slots[0].clone().unwrap());
        draft.add_browsed(browse(&path, FloppyGeometry::HD_1_44).unwrap());
        assert_eq!(draft.slot.entries.len(), 2);
        assert_eq!(draft.selected, Some(1));
        assert!(draft.journal);
    }

    #[test]
    fn blank_disk_form_uses_existing_chs_size_and_formatted_floppy_creation() {
        let temp = assembly::test_support::TempDir::new("blank-dialog");
        let mut form = BlankDisk {
            output: temp.path().join("fixed.img").to_string_lossy().into_owned(),
            ..BlankDisk::new(None)
        };
        assert!(matches!(
            form.command().unwrap().kind,
            pc_image::media_create::ImageKind::HardDisk(_)
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
            pc_image::media_create::execute(form.command().unwrap(), &mut io::sink()).unwrap();
            let bytes = std::fs::read(&form.output).unwrap();
            assert_eq!(&bytes[510..512], &[0x55, 0xAA]);
            assert_eq!(&bytes[54..62], b"FAT12   ");
            assert!(
                pc_image::media_create::execute(form.command().unwrap(), &mut io::sink()).is_err()
            );
        }
    }
}

/// Library drafts keep store identities separate from writable machine state.
type PreparedEntries = Result<(Vec<CatalogueEntry>, usize), String>;
pub enum LibraryMessage {
    Rows(Result<Vec<LibrarySet>, String>),
    Ready(PreparedEntries),
    Progress(String),
}
pub struct LibraryInsert {
    pub store_path: String,
    pub workspace: PathBuf,
    pub rows: Vec<LibrarySet>,
    pub selected: Option<usize>,
    pub first: usize,
    pub load_all: bool,
    pub journal: bool,
    pub pending: Option<std::sync::mpsc::Receiver<LibraryMessage>>,
    pub ready: Option<PreparedEntries>,
    pub progress: String,
    /// The set chosen in the library window, selected once the rows arrive.
    pub preselect: Option<image_store::Digest>,
    /// Why journals are unavailable here; the disks then go in write-protected.
    pub journal_unavailable: Option<&'static str>,
}
#[derive(Clone)]
pub struct LibrarySet {
    pub tag: String,
    pub title: String,
    pub reason: Option<String>,
    pub image: image_store::Descriptor,
    pub config: Option<image_store::media_set::Config>,
}
impl LibraryInsert {
    /// `workspace` is the running session's media directory, which receives
    /// every journal of a library disk.
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            store_path: image_store::location::default_store_root()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            workspace,
            rows: Vec::new(),
            selected: None,
            first: 0,
            load_all: true,
            journal: true,
            pending: None,
            ready: None,
            progress: String::new(),
            preselect: None,
            journal_unavailable: None,
        }
    }
    /// Library disks go in write-protected only, for `reason`.
    pub fn write_protected_only(&mut self, reason: &'static str) {
        self.journal = false;
        self.journal_unavailable = Some(reason);
    }
    pub fn snapshot(&self) -> Self {
        Self {
            store_path: self.store_path.clone(),
            workspace: self.workspace.clone(),
            rows: self.rows.clone(),
            selected: self.selected,
            first: self.first,
            load_all: self.load_all,
            journal: self.journal,
            pending: None,
            ready: None,
            progress: String::new(),
            preselect: None,
            journal_unavailable: self.journal_unavailable,
        }
    }
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn refresh_async(&mut self, drive: FloppyGeometry, unit: u8) {
        let mut draft = self.snapshot();
        self.rows.clear();
        self.selected = None;
        self.first = 0;
        self.ready = None;
        let (sender, receiver) = std::sync::mpsc::channel();
        self.pending = Some(receiver);
        self.progress = "Reading installation sets…".into();
        std::thread::spawn(move || {
            let result = draft.refresh(drive, unit).map(|_| draft.rows);
            let _ = sender.send(LibraryMessage::Rows(result));
        });
    }
    pub fn poll(&mut self) -> Option<String> {
        let mut failure = None;
        loop {
            let message = match self.pending.as_ref().map(|receiver| receiver.try_recv()) {
                Some(Ok(message)) => Some(message),
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    self.pending = None;
                    self.progress.clear();
                    failure = Some("Media preparation worker stopped".into());
                    break;
                }
                _ => None,
            };
            match message {
                Some(LibraryMessage::Progress(progress)) => self.progress = progress,
                Some(LibraryMessage::Rows(result)) => {
                    match result {
                        Ok(rows) => {
                            self.rows = rows;
                            if let Some(wanted) = self.preselect.take() {
                                self.selected = self.rows.iter().position(|row| {
                                    row.image.digest == wanted && row.reason.is_none()
                                });
                            }
                        }
                        Err(error) => failure = Some(error),
                    }
                    self.pending = None;
                    self.progress.clear();
                    break;
                }
                Some(LibraryMessage::Ready(result)) => {
                    self.ready = Some(result);
                    self.pending = None;
                    self.progress.clear();
                    break;
                }
                None => break,
            }
        }
        failure
    }
    pub fn request_entries(&mut self, slot: &FloppySlot, unit: u8) -> Option<PreparedEntries> {
        // A set list that could not be read says why, not "Choose a set".
        if let Some(failure) = self.poll() {
            return Some(Err(failure));
        }
        if let Some(ready) = self.ready.take() {
            return Some(ready);
        }
        if self.is_busy() {
            return None;
        }
        let draft = self.snapshot();
        let slot = slot.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        self.pending = Some(receiver);
        self.progress = "Verifying installation media…".into();
        std::thread::spawn(move || {
            let result = draft.prepare_entries(&slot, unit, &|done, total| {
                let _ = sender.send(LibraryMessage::Progress(format!(
                    "Verifying blobs: {done}/{total}"
                )));
            });
            let _ = sender.send(LibraryMessage::Ready(result));
        });
        None
    }
    pub fn refresh(&mut self, drive: FloppyGeometry, unit: u8) -> Result<(), String> {
        use image_store::{self as store, BlobSource};
        self.rows.clear();
        self.selected = None;
        self.first = 0;
        let store = store::Store::open_existing(&self.store_path).map_err(|e| e.to_string())?;
        let _lease = store.try_read_lease().map_err(|e| e.to_string())?;
        let mut summaries =
            store::summary::SummaryCache::open(&store).map_err(|e| e.to_string())?;
        while !summaries.pending().is_empty() {
            summaries
                .rebuild(&store, 64, &[], &[])
                .map_err(|e| e.to_string())?;
        }
        for tag in summaries.tags() {
            let summary = summaries.summary(&tag.manifest.digest);
            if summary.is_some_and(|row| row.kind != store::summary::ArtifactKind::MediaSet) {
                continue;
            }
            let result = (|| {
                if let Some(failure) = summaries.failure(&tag.manifest.digest) {
                    return Err(store::Error::Invalid(failure.reason.clone()));
                }
                let manifest = store::Manifest::decode_for_summary(
                    &store.read_verified(&tag.manifest, store::MAX_SMALL_BLOB)?,
                )?;
                let config = store::media_set::Config::decode(
                    &store.read_verified(&manifest.config, store::MAX_SMALL_BLOB)?,
                )?;
                if !store::platform::same(&config.platform, manifest.platform()?) {
                    return Err(store::Error::Invalid("config platform mismatch".into()));
                }
                if pc_image::is_pc(&config.platform) {
                    use store::ConfigValidator;
                    let table = toml::Table::try_from(&config)
                        .map_err(|e| store::Error::Invalid(e.to_string()))?;
                    let _ = store::media_set::Validator(&pc_image::media_set::PcMediaFormats)
                        .validate(&table, &manifest.layers, &store)?;
                }
                for layer in &manifest.layers {
                    if matches!(
                        layer.decoded_media_type(),
                        store::media_type::FLOPPY
                            | store::media_type::OPTICAL
                            | store::media_type::EXTRA
                    ) {
                        store.blob_file_path(&layer.digest, layer.size)?;
                    }
                }
                Ok(config)
            })();
            let (config, reason) = match result {
                Ok(config) => {
                    let reason = set_compatibility(&config, drive, unit);
                    (Some(config), reason)
                }
                Err(error) => (None, Some(error.to_string())),
            };
            self.rows.push(LibrarySet {
                tag: tag.name.clone(),
                title: summary
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| tag.name.clone()),
                reason,
                image: tag.manifest.clone(),
                config,
            });
        }
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn entries(&self, slot: &FloppySlot, unit: u8) -> PreparedEntries {
        self.prepare_entries(slot, unit, &|_, _| {})
    }
    fn prepare_entries(
        &self,
        slot: &FloppySlot,
        unit: u8,
        progress: &dyn Fn(usize, usize),
    ) -> PreparedEntries {
        use image_store as store;
        let row = self
            .selected
            .and_then(|i| self.rows.get(i))
            .ok_or("Choose an installation set")?;
        if let Some(reason) = &row.reason {
            return Err(reason.clone());
        }
        let config = row.config.as_ref().ok_or("Set is damaged")?;
        if let Some(reason) = set_compatibility(config, slot.geometry, unit) {
            return Err(reason);
        }
        if self.first >= config.media.len() {
            return Err("Choose a disk".into());
        }
        let store = store::Store::open_existing(&self.store_path).map_err(|e| e.to_string())?;
        let resolved = std::sync::Arc::new(
            store
                .resolve_with_progress(
                    &row.image,
                    &store::media_set::Validator(&pc_image::media_set::PcMediaFormats),
                    progress,
                )
                .map_err(|e| e.to_string())?,
        );
        let config =
            store::media_set::Config::decode(&resolved.config).map_err(|e| e.to_string())?;
        let workspace = store
            .validate_media_output(&self.workspace)
            .map_err(|e| e.to_string())?;
        let mut entries = Vec::new();
        for (index, medium) in config.media.iter().enumerate() {
            if !self.load_all && index != self.first {
                continue;
            }
            let source = store::media_set::CatalogueSource {
                set: row.image.digest.clone(),
                medium_id: medium.id.clone(),
                blob: medium.blob.clone(),
            };
            if let Some(existing) = slot.entries.iter().find(|entry| {
                entry
                    .source
                    .as_ref()
                    .is_some_and(|s| s.same_medium(&source))
            }) {
                if let WriteMode::Journal { path, .. } = &existing.medium.write {
                    store
                        .validate_media_output(path)
                        .map_err(|e| e.to_string())?;
                }
                entries.push(CatalogueEntry {
                    origin: Some(set_origin(&row.tag, index, config.media.len())),
                    reference: Some(row.tag.clone()),
                    ..existing.clone()
                });
                continue;
            }
            let (_, metadata) = resolved
                .verified_file(&medium.blob)
                .map_err(|e| e.to_string())?;
            let path = store
                .resolved_file_path(&resolved, &medium.blob)
                .map_err(|e| e.to_string())?;
            let hash = pc_image::media_set::verified_base(&resolved, &medium.blob)
                .map_err(|e| e.to_string())?
                .identity()
                .base_digest;
            let journal = store
                .validate_media_output(&workspace.join(format!(
                    "{}-{}.upfj",
                    row.image.digest.hex(),
                    medium.id
                )))
                .map_err(|e| e.to_string())?;
            let write = if self.journal {
                WriteMode::Journal {
                    path: journal.clone(),
                    keep_generations: slot.journal_retention,
                }
            } else {
                WriteMode::ReadOnly
            };
            let readonly_bytes = if !self.journal {
                use std::io::{Read, Seek};
                let (mut file, _) = resolved
                    .verified_file(&medium.blob)
                    .map_err(|e| e.to_string())?;
                file.rewind().map_err(|e| e.to_string())?;
                let mut bytes = vec![0; metadata.len() as usize];
                file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                resolved
                    .verified_file(&medium.blob)
                    .map_err(|e| e.to_string())?;
                Some(std::sync::Arc::new(bytes))
            } else {
                None
            };
            entries.push(CatalogueEntry {
                readonly_bytes,
                source: Some(source),
                receipt: Some(resolved.clone()),
                medium: FloppyMedium {
                    image: ImageSource::File(path),
                    expected_sha256: Some(hash),
                    write,
                    name: medium.label.clone(),
                },
                geometry: slot.geometry.medium_for_len(metadata.len()),
                reason: None,
                index: None,
                declared: false,
                alternates: Vec::new(),
                workspace_journal: Some(journal),
                origin: Some(set_origin(&row.tag, index, config.media.len())),
                verified: None,
                reference: Some(row.tag.clone()),
            });
        }
        if self.journal {
            std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
        }
        Ok((entries, if self.load_all { self.first } else { 0 }))
    }
}

/// A library disk's source as the drive tooltip names it.
pub fn set_origin(tag: &str, disk: usize, disks: usize) -> String {
    format!(
        "Media set {tag} from the library · disk {} of {disks}",
        disk + 1
    )
}

pub fn set_compatibility(
    config: &image_store::media_set::Config,
    drive: FloppyGeometry,
    unit: u8,
) -> Option<String> {
    media_compatibility(&config.platform, &config.media, drive, unit)
}

/// The drive check of [`set_compatibility`] over a set's platform and media,
/// as the library's set summaries hold them.
pub fn media_compatibility(
    platform: &str,
    media: &[image_store::media_set::Medium],
    drive: FloppyGeometry,
    unit: u8,
) -> Option<String> {
    if !pc_image::is_pc(platform) {
        return Some(format!("Platform {platform} is unavailable for this drive"));
    }
    for medium in media {
        match pc_image::media_set::geometry(&medium.format) {
            Ok(None) => return Some("CD-ROM emulation not available yet".into()),
            Ok(Some(geometry)) if !drive.accepted_media().contains(&geometry) => {
                let accepted: Vec<String> = drive
                    .accepted_media()
                    .into_iter()
                    .map(geometry_name)
                    .collect();
                return Some(format!(
                    "{} disks; drive {}: is {}, which takes {}",
                    geometry_name(geometry),
                    if unit == 0 { "A" } else { "B" },
                    geometry_name(drive),
                    accepted.join(" or ")
                ));
            }
            Err(error) => return Some(error.to_string()),
            _ => {}
        }
    }
    None
}

impl InsertDialog {
    /// The library insert of a set, optionally chosen beforehand as the media
    /// picker does; that set is selected once the rows are read and checked.
    pub fn open_library_at(
        &mut self,
        workspace: &Path,
        store_root: Option<&Path>,
        set: Option<image_store::Digest>,
    ) {
        let mut library = LibraryInsert::new(workspace.to_path_buf());
        if let Some(root) = store_root {
            library.store_path = root.to_string_lossy().into_owned();
        }
        library.preselect = set;
        library.refresh_async(self.slot.geometry, self.unit);
        self.library = Some(library);
    }
}

/// Validate the complete draft before the existing append/insert boundary sees it.
pub fn append_entries(
    assembled: &mut assembly::AssembledMachine,
    config: &mut MachineConfig,
    slots: &mut [Option<FloppySlot>; 2],
    unit: u8,
    entries: &[CatalogueEntry],
) -> Result<Vec<usize>, String> {
    let original = config.clone();
    let original_slots = slots.clone();
    let key = (assembly::DeviceId::Fdc, unit);
    let count = assembled
        .floppy_names
        .get(&key)
        .ok_or("Drive unavailable")?
        .len();
    let persistence = assembled.persistence.len();
    let mut candidate = config.clone();
    let drive = candidate
        .drives
        .iter_mut()
        .find(|d| d.controller == key.0 && d.unit == unit)
        .ok_or("Drive unavailable")?;
    if drive.media.is_none() {
        drive.media = Some(assembly::FloppyMediaList {
            empty: false,
            entries: vec![FloppyMedium {
                image: drive.image.clone(),
                expected_sha256: drive.expected_sha256,
                write: drive.write.clone(),
                name: String::new(),
            }],
            inserted: 0,
        });
    }
    for entry in entries {
        if let Some(reason) = &entry.reason {
            return Err(reason.clone());
        }
        if entry.index.is_none() {
            if let (Some(_), WriteMode::Journal { path, .. }) = (&entry.source, &entry.medium.write)
            {
                // A library disk first loaded write-protected has no workspace
                // media directory yet, and validation requires the parent.
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
            }
            if let Some(list) = &mut drive.media {
                list.entries.push(entry.medium.clone());
            }
        }
    }
    machine_file::validate(&candidate).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut indices = Vec::new();
        for entry in entries {
            let index = if let Some(index) = entry.index {
                index
            } else {
                let index =
                    if let (Some(receipt), Some(source)) = (&entry.receipt, &entry.source) {
                        let base = pc_image::media_set::verified_base(receipt, &source.blob)
                            .map_err(|e| e.to_string())?;
                        assembled.append_floppy_verified(
                            key.0,
                            unit,
                            config,
                            &entry.medium,
                            base,
                            entry.readonly_bytes.as_deref().map(Vec::as_slice),
                        )
                    } else if let Some(verified) = &entry.verified {
                        let base = verified.base().map_err(|e| e.to_string())?;
                        assembled.append_floppy_verified(
                            key.0,
                            unit,
                            config,
                            &entry.medium,
                            base,
                            None,
                        )
                    } else {
                        assembled.append_floppy(key.0, unit, config, &entry.medium)
                    }
                    .map_err(|e| e.to_string())?;
                index
            };
            if let Some(slot) = &mut slots[usize::from(unit)] {
                slot.record(entry, index);
            }
            indices.push(index);
        }
        Ok(indices)
    })();
    if result.is_err() {
        let device_index = *assembled
            .device_index
            .get(&key.0)
            .ok_or("Drive unavailable")?;
        let fdc = assembled
            .machine
            .bus_mut()
            .device_mut(device_index)
            .and_then(|d| d.as_any_mut())
            .and_then(|d| d.downcast_mut::<devices::Upd765>())
            .ok_or("Drive unavailable")?;
        fdc.truncate_catalogue(usize::from(unit), count)
            .map_err(|e| e.to_string())?;
        assembled.persistence.truncate(persistence);
        if let Some(names) = assembled.floppy_names.get_mut(&key) {
            names.truncate(count);
        }
        *config = original;
        *slots = original_slots;
    }
    result
}

/// Whose floppies wait for Start: a machine file by path, or a machine image
/// by digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingOwner {
    MachineFile(PathBuf),
    Image(image_store::Digest),
}

/// Floppies put into the drives of the selected machine while it is stopped,
/// as into a powered-off PC. Start inserts them before the first instruction,
/// so POST boots from them. They live only in the shell: the machine file and
/// the stored image never change, and selecting another machine drops them.
pub struct PendingMedia {
    pub owner: PendingOwner,
    /// The selection's machine; a machine image lists no catalogue here, its
    /// own media join at Start.
    config: MachineConfig,
    pub slots: [Option<FloppySlot>; 2],
    /// Per drive, the entries one Insert chose and the one that goes in.
    pub inserts: [Option<(Vec<CatalogueEntry>, usize)>; 2],
}

impl PendingMedia {
    pub fn new(owner: PendingOwner, config: MachineConfig) -> Self {
        Self {
            owner,
            slots: FloppySlot::from_config(&config),
            config,
            inserts: [None, None],
        }
    }

    /// A later Insert into the same drive replaces the earlier floppy.
    pub fn insert(&mut self, unit: u8, entries: Vec<CatalogueEntry>, chosen: usize) {
        if let Some(insert) = self.inserts.get_mut(usize::from(unit)) {
            *insert = Some((entries, chosen));
        }
    }

    /// Eject before Start takes the pending floppy out again.
    pub fn eject(&mut self, unit: u8) {
        if let Some(insert) = self.inserts.get_mut(usize::from(unit)) {
            *insert = None;
        }
    }

    pub fn has_insert(&self, unit: u8) -> bool {
        self.inserts
            .get(usize::from(unit))
            .is_some_and(Option::is_some)
    }

    pub fn has_inserts(&self) -> bool {
        self.inserts.iter().any(Option::is_some)
    }

    /// The floppy that goes into drive `unit` at Start, when one waits.
    pub fn chosen(&self, unit: u8) -> Option<&CatalogueEntry> {
        let (entries, chosen) = self.waiting(unit)?;
        entries.get(chosen)
    }

    /// The floppies one Insert put into drive `unit` for Start, all of which
    /// join its catalogue then, and the place of the one that goes in.
    pub fn waiting(&self, unit: u8) -> Option<(&[CatalogueEntry], usize)> {
        let (entries, chosen) = self.inserts.get(usize::from(unit))?.as_ref()?;
        Some((entries, *chosen))
    }

    /// Another of the floppies waiting for drive `unit` goes in at Start.
    pub fn choose(&mut self, unit: u8, chosen: usize) {
        if let Some(Some((entries, at))) = self.inserts.get_mut(usize::from(unit)) {
            if chosen < entries.len() {
                *at = chosen;
            }
        }
    }

    /// The catalogue index of the floppy drive `unit` holds at power-on
    /// without a waiting one: the one its catalogue names, unless the machine
    /// starts with the door open.
    pub fn power_on(&self, unit: u8) -> Option<usize> {
        let drive = self
            .config
            .drives
            .iter()
            .find(|drive| drive.controller == assembly::DeviceId::Fdc && drive.unit == unit)?;
        match &drive.media {
            Some(list) if list.empty || list.entries.is_empty() => None,
            Some(list) => Some(list.inserted),
            None => Some(0),
        }
    }

    /// The drives that hold a pending floppy, as the status bar shows them:
    /// each floppy is listed after the drive's own catalogue, where Start
    /// appends it.
    pub fn drive_views(
        &self,
        origin: &crate::drive_chips::Origin,
    ) -> Vec<crate::drive_chips::DriveView> {
        let mut config = self.config.clone();
        let mut slots = self.slots.clone();
        let mut inserted = [None, None];
        for (unit, insert) in self.inserts.iter().enumerate() {
            let Some(entry) = insert
                .as_ref()
                .and_then(|(entries, chosen)| entries.get(*chosen))
            else {
                continue;
            };
            let Some(drive) = config.drives.iter_mut().find(|drive| {
                drive.controller == assembly::DeviceId::Fdc && usize::from(drive.unit) == unit
            }) else {
                continue;
            };
            let index = match entry.index {
                Some(index) => index,
                None => {
                    if drive.media.is_none() {
                        drive.media = Some(assembly::FloppyMediaList {
                            empty: false,
                            entries: vec![FloppyMedium {
                                image: drive.image.clone(),
                                expected_sha256: drive.expected_sha256,
                                write: drive.write.clone(),
                                name: String::new(),
                            }],
                            inserted: 0,
                        });
                    }
                    let Some(list) = &mut drive.media else {
                        continue;
                    };
                    list.entries.push(entry.medium.clone());
                    list.entries.len() - 1
                }
            };
            if let Some(slot) = &mut slots[unit] {
                slot.record(entry, index);
            }
            inserted[unit] = Some((index, entry.medium.name.clone()));
        }
        crate::drive_chips::drive_views(&config, &slots, &inserted, origin)
            .into_iter()
            .filter(|view| {
                view.floppy_unit()
                    .is_some_and(|unit| inserted[usize::from(unit)].is_some())
            })
            .map(|view| crate::drive_chips::DriveView {
                at_start: true,
                ..view
            })
            .collect()
    }
}

/// Catalogue entry `index` of floppy `unit` in `config`, as its image, pin
/// and write policy; a drive without a list holds its single image.
pub fn listed_medium(
    config: &MachineConfig,
    unit: u8,
    index: usize,
) -> Option<(&ImageSource, Option<[u8; 32]>, &WriteMode)> {
    let drive = config
        .drives
        .iter()
        .find(|drive| drive.controller == assembly::DeviceId::Fdc && drive.unit == unit)?;
    match &drive.media {
        Some(list) => list
            .entries
            .get(index)
            .map(|medium| (&medium.image, medium.expected_sha256, &medium.write)),
        None => (index == 0).then_some((&drive.image, drive.expected_sha256, &drive.write)),
    }
}

impl CatalogueEntry {
    /// Keep a library disk's workspace journal under the same name in
    /// `directory`, the media folder of the session that now runs.
    pub fn move_workspace_journal(&mut self, directory: &Path) {
        let Some(name) = self
            .workspace_journal
            .as_ref()
            .and_then(|journal| journal.file_name())
        else {
            return;
        };
        let moved = directory.join(name);
        if let WriteMode::Journal { path, .. } = &mut self.medium.write {
            if Some(&*path) == self.workspace_journal.as_ref() {
                path.clone_from(&moved);
            }
        }
        self.workspace_journal = Some(moved);
    }
}
