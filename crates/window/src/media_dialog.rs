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
    pub source: Option<image_store::media_set::CatalogueSource>,
    pub receipt: Option<std::sync::Arc<image_store::ResolvedImage>>,
    pub readonly_bytes: Option<std::sync::Arc<Vec<u8>>>,
    pub medium: FloppyMedium,
    pub geometry: Option<FloppyGeometry>,
    pub reason: Option<String>,
    /// Drive catalogue entry currently used for this image and write policy.
    pub index: Option<usize>,
    /// Declared by the machine file, whose write policy the dialog never changes.
    /// Images added during the session (Browse…, From library…) may switch.
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
                    source: None,
                    receipt: None,
                    readonly_bytes: None,
                    medium,
                    geometry: Some(geometry),
                    reason: None,
                    index: Some(index),
                    declared: true,
                    alternates: Vec::new(),
                    workspace_journal: None,
                    origin: None,
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
    pub library: Option<LibraryInsert>,
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
            library: None,
            unit,
            slot,
            selected: None,
            journal: false,
            journal_path: String::new(),
            error: None,
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
        let mut assembled = assembly::assemble(&config).unwrap();
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
        let mut assembled = assembly::assemble(&config).unwrap();
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
        let mut assembled = assembly::assemble(&config).unwrap();
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

/// Library drafts keep store identities separate from writable machine state.
type PreparedEntries = Result<(Vec<CatalogueEntry>, usize), String>;
enum LibraryMessage {
    Rows(Result<Vec<LibrarySet>, String>),
    Ready(PreparedEntries),
    Progress(String),
}
pub(super) struct LibraryInsert {
    pub store_path: String,
    pub workspace: PathBuf,
    pub rows: Vec<LibrarySet>,
    pub selected: Option<usize>,
    pub first: usize,
    pub load_all: bool,
    pub journal: bool,
    pending: Option<std::sync::mpsc::Receiver<LibraryMessage>>,
    pub ready: Option<PreparedEntries>,
    pub progress: String,
}
#[derive(Clone)]
pub(super) struct LibrarySet {
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
        }
    }
    fn snapshot(&self) -> Self {
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
                        Ok(rows) => self.rows = rows,
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
        self.poll();
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
    fn refresh(&mut self, drive: FloppyGeometry, unit: u8) -> Result<(), String> {
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
                if config.platform != manifest.platform()? {
                    return Err(store::Error::Invalid("config platform mismatch".into()));
                }
                if config.platform == "ibm-pc-at" {
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
    #[cfg(test)]
    fn entries(&self, slot: &FloppySlot, unit: u8) -> PreparedEntries {
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
                geometry: Some(slot.geometry),
                reason: None,
                index: None,
                declared: false,
                alternates: Vec::new(),
                workspace_journal: Some(journal),
                origin: Some(set_origin(&row.tag, index, config.media.len())),
            });
        }
        if self.journal {
            std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
        }
        Ok((entries, if self.load_all { self.first } else { 0 }))
    }
}

/// A library disk's source as the drive tooltip names it.
fn set_origin(tag: &str, disk: usize, disks: usize) -> String {
    format!(
        "Media set {tag} from the library · disk {} of {disks}",
        disk + 1
    )
}

pub(super) fn set_compatibility(
    config: &image_store::media_set::Config,
    drive: FloppyGeometry,
    unit: u8,
) -> Option<String> {
    if config.platform != "ibm-pc-at" {
        return Some(format!(
            "Platform {} is unavailable for this drive",
            config.platform
        ));
    }
    for medium in &config.media {
        match pc_image::media_set::geometry(&medium.format) {
            Ok(None) => return Some("CD-ROM emulation not available yet".into()),
            Ok(Some(geometry)) if geometry != drive => {
                return Some(format!(
                    "{} disks; drive {}: is {}",
                    geometry_name(geometry),
                    if unit == 0 { "A" } else { "B" },
                    geometry_name(drive)
                ))
            }
            Err(error) => return Some(error.to_string()),
            _ => {}
        }
    }
    None
}

impl InsertDialog {
    pub fn open_library(&mut self, workspace: &Path, store_root: Option<&Path>) {
        let mut library = LibraryInsert::new(workspace.to_path_buf());
        if let Some(root) = store_root {
            library.store_path = root.to_string_lossy().into_owned();
        }
        library.refresh_async(self.slot.geometry, self.unit);
        self.library = Some(library);
    }

    /// Returns true when From library… asks the caller to open the library,
    /// which knows the session workspace.
    pub fn library_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(library) = &mut self.library else {
            return ui.button("From library…").clicked();
        };
        if let Some(error) = library.poll() {
            self.error = Some(error);
        }
        let busy = library.is_busy() || library.ready.is_some();
        let mut back = false;
        ui.add_enabled_ui(!busy, |ui| {
        ui.label(format!(
            "Drive {}: — {} floppy",
            if self.unit == 0 { "A" } else { "B" },
            geometry_name(self.slot.geometry)
        ));
        ui.label("Choose an installation set. Unavailable formats stay visible with a reason.");
        ui.horizontal(|ui| {
            ui.label("Store");
            ui.text_edit_singleline(&mut library.store_path);
            if ui.button("Refresh").clicked() {
                self.error = None; library.refresh_async(self.slot.geometry, self.unit);
            }
        });
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                for (i, row) in library.rows.iter().enumerate() {
                    let response = ui.add_enabled(
                        row.reason.is_none(),
                        egui::RadioButton::new(
                            library.selected == Some(i),
                            format!("{} · {}", row.title, row.tag),
                        ),
                    );
                    if response.clicked() {
                        library.selected = Some(i);
                        library.first = 0;
                        self.error = None;
                    }
                    if let Some(reason) = &row.reason {
                        ui.label(reason);
                    }
                }
                if library.rows.is_empty() {
                    ui.label("No installation sets in this store.");
                }
            });
        if let Some(config) = library.selected.and_then(|i| library.rows.get(i)).and_then(|row| row.config.as_ref()) {
            egui::ComboBox::from_label("Insert disk")
                .selected_text(
                    config.media
                        .get(library.first)
                        .map_or("Choose a disk", |m| m.label.as_str()),
                )
                .show_ui(ui, |ui| {
                    for (i, m) in config.media.iter().enumerate() {
                        ui.selectable_value(&mut library.first, i, &m.label);
                    }
                });
        }
        ui.label("Workspace writes");
        ui.label("Already loaded disks retain their existing catalogue write policy.");
        ui.radio_value(&mut library.journal, true, "Per-workspace journal");
        ui.radio_value(&mut library.journal, false, "Write-protected");
        ui.checkbox(
            &mut library.load_all,
            "Load all disks of the set into the catalogue in order",
        );
        ui.label("The library set stays read-only. Journals belong only to this session's workspace.");
        back = ui.button("Back to catalogue").clicked();
        });
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(&library.progress);
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(30));
        }
        if back {
            self.library = None;
        }
        false
    }
}

/// Validate the complete draft before the existing append/insert boundary sees it.
pub(super) fn append_entries(
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
    candidate.validate().map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod media_set_tests {
    use super::*;
    use image_store::{
        self as store,
        media_set::{Config, Medium},
    };
    use pc_image::media_set as pc;
    fn config(format: &str) -> Config {
        Config {
            schema: 1,
            platform: "ibm-pc-at".into(),
            media: vec![Medium {
                id: "disk-1".into(),
                label: "Disk one".into(),
                format: pc::format(format).unwrap(),
                blob: store::Digest::of(b"disk"),
            }],
            extras: Vec::new(),
        }
    }
    #[test]
    fn media_set_incompatible_and_optical_rows_explain_disabled_state() {
        assert_eq!(
            set_compatibility(&config("1.2M"), FloppyGeometry::HD_1_44, 0).as_deref(),
            Some("1.2M disks; drive A: is 1.44M")
        );
        assert_eq!(
            set_compatibility(&config("iso-2048"), FloppyGeometry::HD_1_44, 0).as_deref(),
            Some("CD-ROM emulation not available yet")
        );
        assert_eq!(
            set_compatibility(&config("1.44M"), FloppyGeometry::HD_1_44, 0),
            None
        );
        let mut mixed = config("1.44M");
        mixed.media.push(config("720K").media.remove(0));
        assert!(set_compatibility(&mixed, FloppyGeometry::HD_1_44, 1)
            .unwrap()
            .contains("drive B:"));
    }
    #[test]
    fn media_set_load_all_appends_in_order_and_journals_preserve_store() {
        let temp = assembly::test_support::TempDir::new("library-insert");
        let root = temp.path().canonicalize().unwrap();
        let store_path = root.join("store");
        let source = root.join("first.img");
        let second = root.join("second.img");
        let args = vec![
            "floppy".into(),
            "--type".into(),
            "1.44M".into(),
            "--format".into(),
            source.clone().into_os_string(),
        ];
        cli::create::execute(cli::create::parse(&args).unwrap(), &mut io::sink()).unwrap();
        let bytes = std::fs::read(&source).unwrap();
        let mut other = bytes.clone();
        other[100] = 5;
        std::fs::write(&second, &other).unwrap();
        let args = vec![
            "media".into(),
            "import".into(),
            source.into_os_string(),
            second.into_os_string(),
            "--store".into(),
            store_path.clone().into_os_string(),
            "--title".into(),
            "Synthetic installation".into(),
            "--tag".into(),
            "test:set".into(),
            "--create".into(),
        ];
        let mut command = vec!["image".into()];
        command.extend(args);
        let mut errors = Vec::new();
        assert_eq!(
            cli::command::run(&command, &mut io::sink(), &mut errors),
            0,
            "{}",
            String::from_utf8_lossy(&errors)
        );
        let mut machine = assembly::floppy_boot(vec![0xFA, 0xEB, 0xFE], vec![0; bytes.len()]);
        let mut assembled = assembly::assemble(&machine).unwrap();
        let mut slots = FloppySlot::from_config(&machine);
        let mut library = LibraryInsert::new(
            image_store::location::machine_media_dir(&root.join("machine.toml")).unwrap(),
        );
        library.store_path = store_path.to_string_lossy().into_owned();
        library.refresh(FloppyGeometry::HD_1_44, 0).unwrap();
        assert_eq!(library.rows.len(), 1);
        assert_eq!(library.rows[0].reason, None);
        library.selected = Some(0);
        library.first = 1;
        let slot = slots[0].as_ref().unwrap();
        let (entries, chosen) = library.entries(slot, 0).unwrap();
        assert_eq!(chosen, 1);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].medium.name, "first.img");
        assert_eq!(entries[1].medium.name, "second.img");
        assert_eq!(
            entries[1].origin.as_deref(),
            Some("Media set test:set from the library · disk 2 of 2")
        );
        let indices =
            append_entries(&mut assembled, &mut machine, &mut slots, 0, &entries).unwrap();
        assert_eq!(indices, [1, 2]);
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 3);
        assembled
            .change_floppy(
                assembly::DeviceId::Fdc,
                0,
                assembly::FloppyAction::Insert(indices[chosen]),
            )
            .unwrap();
        let views = crate::drive_chips::drive_views(
            &machine,
            &slots,
            &[
                Some((
                    indices[chosen],
                    assembled.floppy_name(assembly::DeviceId::Fdc, 0),
                )),
                None,
            ],
            &crate::drive_chips::Origin::MachineFile("machine".into()),
        );
        assert_eq!(views[0].chip_text(), "A: second.img · 1.44M · journal");
        assert!(views[0].tooltip(false).contains(&(
            "Source",
            "Media set test:set from the library · disk 2 of 2".into()
        )));
        assert_eq!(
            assembled.floppy_name(assembly::DeviceId::Fdc, 0),
            "second.img"
        );
        assembled
            .change_floppy(assembly::DeviceId::Fdc, 0, assembly::FloppyAction::Eject)
            .unwrap();
        let ImageSource::File(base_path) = &entries[0].medium.image else {
            panic!("file base");
        };
        let WriteMode::Journal { path, .. } = &entries[0].medium.write else {
            panic!("journal");
        };
        assert!(!path.starts_with(&store_path));
        assembled
            .change_floppy(
                assembly::DeviceId::Fdc,
                0,
                assembly::FloppyAction::Insert(indices[0]),
            )
            .unwrap();
        let device_index = assembled.device_index[&assembly::DeviceId::Fdc];
        let fdc = assembled
            .machine
            .bus_mut()
            .device_mut(device_index)
            .unwrap();
        fdc.port_write(0x3f2, 1, 0x1c).unwrap();
        for byte in [5, 0, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            fdc.port_write(0x3f5, 1, byte).unwrap();
        }
        assert!(fdc.take_dma_request().is_some());
        fdc.dma_complete(device::DmaOutcome::FromMemory {
            bytes: vec![0xA5; 512],
            terminal_count: true,
        });
        for _ in 0..7 {
            fdc.port_read(0x3f5, 1).unwrap();
        }
        assembled
            .change_floppy(assembly::DeviceId::Fdc, 0, assembly::FloppyAction::Eject)
            .unwrap();
        let store = store::Store::open_existing(&store_path).unwrap();
        let source = entries[0].source.as_ref().unwrap();
        let image = entries[0].receipt.as_ref().unwrap();
        let stored = image
            .manifest
            .layers
            .iter()
            .find(|d| d.names(&source.blob))
            .unwrap();
        assert!(stored.is_compressed());
        store.verify(&stored.digest, stored.size).unwrap();
        assert_eq!(std::fs::read(base_path).unwrap(), bytes);
        let scan = media::scan(&std::fs::read(path).unwrap(), &bytes, bytes.len() as u64);
        assert_eq!(&scan.image.unwrap()[..512], &[0xA5; 512]);
        let mut receiving = assembly::floppy_boot(vec![0xFA, 0xEB, 0xFE], vec![0; bytes.len()]);
        receiving.drives[0].unit = 1;
        let mut receiving_slots = FloppySlot::from_config(&receiving);
        let mut receiving_machine = assembly::assemble(&receiving).unwrap();
        let (moved, _) = library
            .entries(receiving_slots[1].as_ref().unwrap(), 1)
            .unwrap();
        assert_eq!(moved[0].medium.write, entries[0].medium.write);
        let moved_indices = append_entries(
            &mut receiving_machine,
            &mut receiving,
            &mut receiving_slots,
            1,
            &moved,
        )
        .unwrap();
        receiving_machine
            .change_floppy(
                assembly::DeviceId::Fdc,
                1,
                assembly::FloppyAction::Insert(moved_indices[0]),
            )
            .unwrap();
        let fdc_index = receiving_machine.device_index[&assembly::DeviceId::Fdc];
        let fdc = receiving_machine
            .machine
            .bus_mut()
            .device_mut(fdc_index)
            .unwrap();
        fdc.port_write(0x3f2, 1, 0x2d).unwrap();
        for byte in [6, 1, 0, 0, 1, 2, 1, 0x1b, 0xff] {
            fdc.port_write(0x3f5, 1, byte).unwrap();
        }
        let device::DmaOp::ToMemory(read_back) = fdc.take_dma_request().unwrap().op else {
            panic!("drive B read");
        };
        assert_eq!(read_back, vec![0xA5; 512]);
        fdc.dma_complete(device::DmaOutcome::ToMemory {
            transferred: 512,
            terminal_count: true,
        });
        for _ in 0..7 {
            fdc.port_read(0x3f5, 1).unwrap();
        }
        receiving_machine
            .change_floppy(assembly::DeviceId::Fdc, 1, assembly::FloppyAction::Eject)
            .unwrap();
        store.verify(&stored.digest, stored.size).unwrap();

        let (reused, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        let reused_indices =
            append_entries(&mut assembled, &mut machine, &mut slots, 0, &reused).unwrap();
        assert_eq!(reused_indices, indices);
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 3);
        assert!(source.roots().contains(&library.rows[0].image.digest));
        #[cfg(unix)]
        {
            std::fs::remove_file(path).unwrap();
            std::os::unix::fs::symlink(base_path, path).unwrap();
            assert!(library
                .entries(slots[0].as_ref().unwrap(), 0)
                .unwrap_err()
                .contains("blobs"));
            std::fs::remove_file(path).unwrap();
        }
        library.load_all = false;
        library.journal = false;
        let (single, chosen) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(chosen, 0);
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].medium.name, "second.img");
        assert_eq!(single[0].index, Some(2));
        assert_eq!(single[0].medium.write, entries[1].medium.write);
        library.workspace = store_path.join("images/blobs/forbidden");
        assert!(library
            .entries(slots[0].as_ref().unwrap(), 0)
            .unwrap_err()
            .contains("blobs"));
    }
    #[test]
    fn media_set_batch_rejection_leaves_catalogue_unchanged() {
        let temp = assembly::test_support::TempDir::new("library-batch");
        let path = temp.path().join("disk.img");
        std::fs::write(&path, vec![0; 1_474_560]).unwrap();
        let mut config = assembly::floppy_boot(vec![0xFA, 0xEB, 0xFE], vec![0; 1_474_560]);
        let mut assembled = assembly::assemble(&config).unwrap();
        let mut slots = FloppySlot::from_config(&config);
        let first = browse(&path, FloppyGeometry::HD_1_44).unwrap();
        let mut bad = first.clone();
        bad.reason = Some("incompatible disk".into());
        assert!(append_entries(&mut assembled, &mut config, &mut slots, 0, &[first, bad]).is_err());
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 1);
        assert!(config.drives[0].media.is_none());
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    fn fixture() -> (
        assembly::test_support::TempDir,
        LibraryInsert,
        MachineConfig,
    ) {
        let temp = assembly::test_support::TempDir::new("review-media");
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("disk.img");
        std::fs::write(&source, vec![0x11; 1_474_560]).unwrap();
        let args: Vec<std::ffi::OsString> = vec![
            "image".into(),
            "media".into(),
            "import".into(),
            source.into_os_string(),
            "--title".into(),
            "Review set".into(),
            "--tag".into(),
            "review:set".into(),
            "--store".into(),
            root.join("store").into_os_string(),
            "--create".into(),
        ];
        let mut error = Vec::new();
        assert_eq!(
            cli::command::run(&args, &mut io::sink(), &mut error),
            0,
            "{}",
            String::from_utf8_lossy(&error)
        );
        let mut library = LibraryInsert::new(
            image_store::location::machine_media_dir(&root.join("machine.toml")).unwrap(),
        );
        library.store_path = root.join("store").to_string_lossy().into_owned();
        library.refresh(FloppyGeometry::HD_1_44, 0).unwrap();
        library.selected = Some(0);
        (
            temp,
            library,
            assembly::floppy_boot(vec![0xfa, 0xeb, 0xfe], vec![0; 1_474_560]),
        )
    }
    #[test]
    fn reused_library_disk_takes_the_source_of_the_set_it_came_from_again() {
        let (_temp, mut library, mut config) = fixture();
        let mut slots = FloppySlot::from_config(&config);
        let mut assembled = assembly::assemble(&config).unwrap();
        let (entries, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        let first = "Media set review:set from the library · disk 1 of 1";
        assert_eq!(entries[0].origin.as_deref(), Some(first));
        let indices = append_entries(&mut assembled, &mut config, &mut slots, 0, &entries).unwrap();
        // The same set under another tag reuses the loaded disk.
        library.rows[0].tag = "review:other".into();
        let (again, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        let other = "Media set review:other from the library · disk 1 of 1";
        assert_eq!(again[0].index, Some(indices[0]));
        assert_eq!(again[0].origin.as_deref(), Some(other));
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &again).unwrap(),
            indices
        );
        let slot = slots[0].as_ref().unwrap();
        assert_eq!(slot.entries.len(), 2);
        assert_eq!(slot.entries[1].origin.as_deref(), Some(other));
    }

    #[test]
    fn review_single_hash_reuse_per_medium_journal_and_gc_roots() {
        let (_temp, mut library, mut config) = fixture();
        let mut slots = FloppySlot::from_config(&config);
        let mut assembled = assembly::assemble(&config).unwrap();
        let medium = &library.rows[0].config.as_ref().unwrap().media[0];
        let decoded_digest = medium.blob.clone();
        let store = image_store::Store::open_existing(&library.store_path).unwrap();
        let image = store
            .resolve_descriptor(
                &library.rows[0].image,
                &image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats),
            )
            .unwrap();
        let digest = image
            .manifest
            .layers
            .iter()
            .find(|d| d.names(&medium.blob))
            .unwrap()
            .digest
            .clone();
        let stored = image_store::verification_passes(&digest);
        let hashes = media::hash_passes();
        let (entries, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(image_store::verification_passes(&digest) - stored, 1);
        let indices = append_entries(&mut assembled, &mut config, &mut slots, 0, &entries).unwrap();
        assert_eq!(media::hash_passes(), hashes);
        let (again, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &again).unwrap(),
            indices
        );
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 2);
        let (other_drive, _) = library
            .entries(
                &FloppySlot {
                    entries: vec![],
                    ..slots[0].as_ref().unwrap().clone()
                },
                1,
            )
            .unwrap();
        assert_eq!(entries[0].medium.write, other_drive[0].medium.write);
        let mut job = library.snapshot();
        assert!(job.request_entries(slots[0].as_ref().unwrap(), 0).is_none());
        let receiver = job.pending.take().unwrap();
        let mut progress = false;
        loop {
            match receiver
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap()
            {
                LibraryMessage::Progress(_) => progress = true,
                LibraryMessage::Ready(result) => {
                    assert_eq!(result.unwrap().0[0].index, Some(indices[0]));
                    break;
                }
                LibraryMessage::Rows(_) => panic!("unexpected refresh result"),
            }
        }
        assert!(progress);

        let roots = entries[0].source.as_ref().unwrap().roots();
        assert!(roots.contains(&decoded_digest));
        assert!(roots.contains(&library.rows[0].image.digest));
        let store = image_store::Store::open_existing(&library.store_path).unwrap();
        let owner = store.workspace("review").unwrap();
        library.workspace = image_store::location::workspace_media_dir(&owner);
        let (workspace_entries, _) = library
            .entries(
                &FloppySlot {
                    entries: vec![],
                    ..slots[0].as_ref().unwrap().clone()
                },
                0,
            )
            .unwrap();
        let WriteMode::Journal { path, .. } = &workspace_entries[0].medium.write else {
            panic!("journal")
        };
        assert!(path.starts_with(owner.path().join("media")));
    }
    #[test]
    fn library_disks_switch_between_write_protection_and_the_workspace_journal() {
        let (_temp, mut library, mut config) = fixture();
        let mut slots = FloppySlot::from_config(&config);
        let mut assembled = assembly::assemble(&config).unwrap();
        library.journal = false;
        let (entries, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(entries[0].medium.write, WriteMode::ReadOnly);
        let journal = entries[0].workspace_journal.clone().unwrap();
        assert!(!journal.starts_with(&library.store_path));
        assert!(!journal.parent().unwrap().exists());
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &entries).unwrap(),
            [1]
        );

        let mut draft = InsertDialog::new(0, slots[0].clone().unwrap());
        draft.select(Some(1));
        assert!(!draft.journal);
        draft.journal = true;
        draft.journal_path = "ignored-for-library-media.upfj".into();
        let entry = draft.selection().unwrap();
        assert_eq!(entry.index, None);
        assert_eq!(
            entry.medium.write,
            WriteMode::Journal {
                path: journal.clone(),
                keep_generations: draft.slot.journal_retention,
            }
        );
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &[entry]).unwrap(),
            [2]
        );
        assert!(journal.parent().unwrap().is_dir());
        for action in [
            assembly::FloppyAction::Insert(2),
            assembly::FloppyAction::Eject,
        ] {
            assembled
                .change_floppy(assembly::DeviceId::Fdc, 0, action)
                .unwrap();
        }
        let row = &slots[0].as_ref().unwrap().entries[1];
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 2);
        assert_eq!(row.alternates, [(WriteMode::ReadOnly, 1)]);

        // Loading the set again keeps the disk's current policy; the
        // catalogue switches it back without appending.
        let (again, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(again[0].index, Some(2));
        let mut draft = InsertDialog::new(0, slots[0].clone().unwrap());
        draft.select(Some(1));
        assert!(draft.journal);
        draft.journal = false;
        let entry = draft.selection().unwrap();
        assert_eq!(entry.index, Some(1));
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &[entry]).unwrap(),
            [1]
        );
        assert_eq!(
            assembled.floppy_names[&(assembly::DeviceId::Fdc, 0)].len(),
            3
        );
    }
    #[test]
    fn review_candidate_validation_and_mid_batch_failure_are_atomic() {
        let (temp, library, mut config) = fixture();
        let mut slots = FloppySlot::from_config(&config);
        let mut assembled = assembly::assemble(&config).unwrap();
        let (entries, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        let mut bad = entries[0].clone();
        let ImageSource::File(base) = &bad.medium.image else {
            panic!("base")
        };
        bad.medium.write = WriteMode::Journal {
            path: base.clone(),
            keep_generations: 10,
        };
        assert!(append_entries(&mut assembled, &mut config, &mut slots, 0, &[bad]).is_err());
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 1);
        let mut second = entries[0].clone();
        second.medium.name = "Second".into();
        second.medium.write = WriteMode::Journal {
            path: temp.path().join("second.upfj"),
            keep_generations: 10,
        };
        let original_handles = assembled.persistence.len();
        second.medium.expected_sha256 = Some([0; 32]);
        let error = append_entries(
            &mut assembled,
            &mut config,
            &mut slots,
            0,
            &[entries[0].clone(), second],
        )
        .unwrap_err();
        assert!(error.contains("SHA-256"));
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 1);
        assert!(config.drives[0].media.is_none());
        assert_eq!(assembled.persistence.len(), original_handles);
        assert_eq!(
            assembled.floppy_names[&(assembly::DeviceId::Fdc, 0)].len(),
            1
        );
        assert_eq!(
            append_entries(&mut assembled, &mut config, &mut slots, 0, &entries).unwrap(),
            [1]
        );
    }
    #[test]
    fn review_tampered_blob_and_receipt_refuse_insertion() {
        let (_temp, library, mut config) = fixture();
        let mut slots = FloppySlot::from_config(&config);
        let mut assembled = assembly::assemble(&config).unwrap();
        let (entries, _) = library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        let ImageSource::File(base) = &entries[0].medium.image else {
            panic!("base")
        };
        // The decoded file is a rebuildable cache: a damaged copy is rebuilt from
        // the verified stored blob, while receipts taken before stay refused.
        std::fs::write(base, vec![0x22; 1_474_560]).unwrap();
        library.entries(slots[0].as_ref().unwrap(), 0).unwrap();
        assert_eq!(std::fs::read(base).unwrap(), vec![0x11; 1_474_560]);
        assert!(append_entries(&mut assembled, &mut config, &mut slots, 0, &entries).is_err());
        assert_eq!(slots[0].as_ref().unwrap().entries.len(), 1);
        // Damaged stored bytes cannot be repaired and refuse the set.
        let receipt = entries[0].receipt.as_ref().unwrap();
        let blob = &entries[0].source.as_ref().unwrap().blob;
        let stored = receipt
            .manifest
            .layers
            .iter()
            .find(|d| d.names(blob))
            .unwrap();
        assert!(stored.is_compressed());
        let store = image_store::Store::open_existing(&library.store_path).unwrap();
        let path = store.blob_file_path(&stored.digest, stored.size).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        std::fs::write(&path, bytes).unwrap();
        assert!(library
            .entries(slots[0].as_ref().unwrap(), 0)
            .unwrap_err()
            .contains("digest mismatch"));
    }
    #[test]
    fn review_damaged_set_is_a_disabled_row_and_refresh_is_nonblocking() {
        let (_temp, mut library, _) = fixture();
        let store = image_store::Store::open_existing(&library.store_path).unwrap();
        let image = store
            .resolve_descriptor(
                &library.rows[0].image,
                &image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats),
            )
            .unwrap();
        let mut manifest = image.manifest.clone();
        let bytes = b"invalid config";
        let blob = store.import(bytes.as_slice(), None).unwrap();
        manifest.config = image_store::Descriptor::new(
            image_store::media_type::MEDIA_SET_CONFIG,
            blob.digest,
            blob.size,
        );
        let damaged = store.import_manifest(&manifest).unwrap();
        let mut bad_summary = image.manifest.clone();
        let blob = store
            .import(b"invalid metadata JSON".as_slice(), None)
            .unwrap();
        let metadata = bad_summary
            .layers
            .iter_mut()
            .find(|d| d.media_type == image_store::media_type::METADATA)
            .unwrap();
        *metadata =
            image_store::Descriptor::new(image_store::media_type::METADATA, blob.digest, blob.size);
        let summary_image = store.import_manifest(&bad_summary).unwrap();

        let index_path = store.root().join("images/index.json");
        // A damaged imported index can reference an invalid set without going through publication.
        let mut index = std::fs::read_to_string(&index_path).unwrap();
        let new= format!("{{\"mediaType\":\"{}\",\"digest\":\"{}\",\"size\":{},\"annotations\":{{\"org.opencontainers.image.ref.name\":\"review:damaged\"}}}},",image_store::media_type::MANIFEST,damaged.digest,damaged.size);
        let new = format!("{new}{{\"mediaType\":\"{}\",\"digest\":\"{}\",\"size\":{},\"annotations\":{{\"org.opencontainers.image.ref.name\":\"review:summary-damaged\"}}}},", image_store::media_type::MANIFEST, summary_image.digest, summary_image.size);
        index = index.replacen("\"manifests\":[", &format!("\"manifests\":[{new}"), 1);
        std::fs::write(index_path, index).unwrap();
        library.refresh(FloppyGeometry::HD_1_44, 0).unwrap();
        assert_eq!(library.rows.len(), 3);
        assert!(library
            .rows
            .iter()
            .any(|row| row.tag == "review:damaged" && row.reason.is_some()));
        assert!(library
            .rows
            .iter()
            .any(|row| row.tag == "review:set" && row.reason.is_none()));
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(store.root().join("locks/store.lock"))
            .unwrap();
        lock.lock().unwrap();
        library.refresh_async(FloppyGeometry::HD_1_44, 0);
        assert!(library.is_busy());
        let message = library
            .pending
            .take()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        let LibraryMessage::Rows(Err(reason)) = message else {
            panic!("busy result")
        };
        assert!(reason.contains("store busy"));
    }
}
