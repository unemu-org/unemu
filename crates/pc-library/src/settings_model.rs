// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless property-sheet state, inherited metadata and local child publication.
//!
//! This adapter maps file syntax to assembly's descriptors; it owns no hardware
//! domains or compatibility rules. Descriptions never assemble a guest or trust
//! unverified images. See `backlog/docs/design/machine-images.md` §6–7.

use assembly::{
    capabilities::{self as caps, Draft, Value},
    FirmwareEntry,
};
use machine_file::{schema::v1 as file, Describe, MachineFile, Origins};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

pub struct SettingsModel {
    pub name: String,
    pub parent: PathBuf,
    pub target: PathBuf,
    pub draft: Draft,
    pub read_only: bool,
    pub tab: caps::Group,
    pub error: Option<String>,
    pub overwrite: bool,
    pub preview: String,
    pub current_content: Option<String>,
    layer: toml::Table,
    base_fields: Vec<caps::Field>,
    original: Vec<caps::Field>,
    document: MachineFile,
    origins: Origins,
    saved_preview: Option<String>,
    choices: BTreeMap<String, Vec<caps::Choice>>,
    /// The library's machines directory it was opened with, for opening the
    /// staged and the saved child again.
    library: Option<PathBuf>,
}

impl SettingsModel {
    /// Opens the settings of the machine file at `path`. `library` is the
    /// library's machines directory, which holds the custom layer of a file
    /// outside any `machines` directory; the desktop passes the directory it
    /// lists.
    pub fn open(path: &Path, read_only: bool, library: Option<PathBuf>) -> Result<Self, String> {
        let parent = fs::canonicalize(path).map_err(|e| e.to_string())?;
        let (document, origins) = machine_file::resolve(&parent).map_err(|e| e.to_string())?;
        let mut draft = document
            .describe(&origins)
            .map_err(|error| error.to_string())?;
        draft.preserve_card_roles = true;
        let name = document.name.clone().unwrap_or_else(|| {
            parent
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        // A file outside any `machines` directory keeps its custom layer in the
        // directory the library lists, which need not exist until the first save.
        // Without one (no data directory), nothing may land in the working directory.
        let root = parent
            .ancestors()
            .find(|path| path.file_name().is_some_and(|name| name == "machines"))
            .map(Path::to_path_buf)
            .or_else(|| library.clone())
            .ok_or(
                "Settings cannot save a custom machine here: the host has no data directory. \
                 Set UNEMUPC_MACHINES_DIR to a folder for machine files.",
            )?;
        let root = match fs::canonicalize(&root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => std::path::absolute(&root),
            result => result,
        }
        .map_err(|e| e.to_string())?;
        let raw = fs::read_to_string(&parent).map_err(|error| error.to_string())?;
        let existing_layer = if parent
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            raw.parse::<toml::Table>().ok()
        } else {
            None
        };
        let editing = parent.parent() == Some(root.join("local").as_path())
            && existing_layer
                .as_ref()
                .is_some_and(|layer| layer.get("extends").and_then(toml::Value::as_str).is_some());
        let target = if editing {
            parent.clone()
        } else {
            root.join("local").join(format!(
                "{}-custom.toml",
                parent.file_stem().unwrap_or_default().to_string_lossy()
            ))
        };
        let mut layer = if editing {
            existing_layer.unwrap_or_default()
        } else {
            toml::Table::new()
        };
        let base_fields = if editing {
            layer
                .get("extends")
                .and_then(toml::Value::as_str)
                .filter(|reference| !reference.starts_with("preset:"))
                .map(|reference| {
                    let (file, origins) = machine_file::resolve(
                        parent.parent().unwrap_or(Path::new(".")).join(reference),
                    )
                    .map_err(|error| error.to_string())?;
                    file.describe(&origins)
                        .map(|draft| caps::fields(&draft))
                        .map_err(|error| error.to_string())
                })
                .transpose()?
                .unwrap_or_default()
        } else {
            caps::fields(&draft)
        };
        if !editing {
            layer.insert("schema".into(), 1.into());
            layer.insert(
                "extends".into(),
                relative_path(target.parent().ok_or("Missing output directory")?, &parent)?.into(),
            );
            layer.insert(
                "name".into(),
                if name.ends_with(" (custom)") {
                    name.clone()
                } else {
                    format!("{name} (custom)")
                }
                .into(),
            );
        }
        let current_content = fs::read_to_string(&target).ok();
        let original = caps::fields(&draft);
        let mut model = Self {
            name,
            parent,
            target,
            draft,
            read_only,
            tab: caps::Group::System,
            error: None,
            overwrite: false,
            preview: String::new(),
            current_content,
            layer,
            base_fields,
            original,
            document,
            origins,
            saved_preview: None,
            choices: BTreeMap::new(),
            library,
        };
        model.refresh_preview()?;
        Ok(model)
    }

    pub fn set(&mut self, path: &str, value: Value) {
        if self.read_only {
            return;
        }
        self.draft.set(path, value);
        self.draft.revision = self.draft.revision.wrapping_add(1);
        self.overwrite = false;
        self.choices.clear();
        self.error = self.refresh_preview().err();
    }

    pub fn choices(&mut self, path: &str) -> &[caps::Choice] {
        self.choices
            .entry(path.into())
            .or_insert_with(|| caps::choices(&self.draft, path))
    }

    pub fn diff_preview(&self) -> String {
        let Some(current) = &self.current_content else {
            return self.preview.clone();
        };
        let old: Vec<_> = current.lines().collect();
        let new: Vec<_> = self.preview.lines().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let mut diff = String::new();
        for line in &old[prefix..old.len() - suffix] {
            diff.push_str(&format!("- {line}\n"));
        }
        for line in &new[prefix..new.len() - suffix] {
            diff.push_str(&format!("+ {line}\n"));
        }
        diff
    }

    pub fn validation(&self) -> caps::Validation {
        caps::validate_draft(&self.draft)
    }

    pub fn dirty(&self) -> bool {
        self.saved_preview
            .as_ref()
            .map_or_else(|| self.changed(), |saved| saved != &self.preview)
    }

    pub fn can_apply(&self) -> bool {
        !self.read_only && self.error.is_none() && !self.validation().has_errors() && self.dirty()
    }

    /// [`Self::open`] under the name the desktop's test of a machine file
    /// outside any `machines` directory opens it by.
    #[cfg(any(test, feature = "test-support"))]
    pub fn open_with_library(
        path: &Path,
        read_only: bool,
        library: Option<PathBuf>,
    ) -> Result<Self, String> {
        Self::open(path, read_only, library)
    }

    fn changed(&self) -> bool {
        !self.changed_fields().is_empty()
    }

    fn changed_fields(&self) -> Vec<caps::Field> {
        caps::fields(&self.draft)
            .into_iter()
            .filter(|field| {
                !self
                    .original
                    .iter()
                    .any(|old| old.path == field.path && old.value == field.value)
            })
            .collect()
    }

    fn refresh_preview(&mut self) -> Result<(), String> {
        let mut root = self.layer.clone();
        let mut cards: BTreeMap<String, toml::Table> = root
            .remove("card")
            .and_then(|cards| cards.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| {
                let table = value.as_table()?.clone();
                let id = table
                    .get("id")
                    .or_else(|| table.get("kind"))?
                    .as_str()?
                    .to_string();
                Some((id, table))
            })
            .collect();
        let mut firmware_changes = Vec::new();
        let mut changed = self.changed_fields();
        changed.sort_by_key(|field| field.path.ends_with(".enabled"));
        for field in &changed {
            if field.path.starts_with("firmware.") {
                firmware_changes.push(field);
                continue;
            }
            if field.path.starts_with("cpu.rate_") {
                continue;
            }
            if let Some(tail) = field.path.strip_prefix("card[") {
                let (id, suffix) = tail.split_once("].").ok_or("Invalid card field")?;
                let card = cards
                    .entry(id.into())
                    .or_insert_with(|| toml::Table::from_iter([("id".into(), id.into())]));
                if let Some(drive) = suffix.strip_prefix("drives[unit=") {
                    let (unit, _) = drive.split_once(']').ok_or("Invalid drive field")?;
                    let drive = toml::Table::from_iter([
                        (
                            "unit".into(),
                            unit.parse::<i64>().map_err(|e| e.to_string())?.into(),
                        ),
                        ("type".into(), toml_value(&field.value)),
                    ]);
                    let drives = card
                        .entry("drives")
                        .or_insert(toml::Value::Array(Vec::new()));
                    if let toml::Value::Array(drives) = drives {
                        if let Some(existing) = drives
                            .iter_mut()
                            .find(|existing| existing.get("unit") == drive.get("unit"))
                        {
                            if let Some(existing) = existing.as_table_mut() {
                                existing.extend(drive);
                            }
                        } else {
                            drives.push(drive.into());
                        }
                    }
                } else if suffix == "enabled" {
                    if field.value == Value::Name("none".into()) {
                        card.clear();
                        card.insert("id".into(), id.into());
                        card.insert("remove".into(), true.into());
                    } else {
                        card.remove("remove");
                    }
                } else if self
                    .base_fields
                    .iter()
                    .any(|base| base.path == field.path && base.value == field.value)
                {
                    card.remove(suffix);
                } else {
                    card.insert(suffix.into(), toml_value(&field.value));
                }
            } else if let Some((table, key)) = field.path.split_once('.') {
                if self
                    .base_fields
                    .iter()
                    .any(|base| base.path == field.path && base.value == field.value)
                {
                    table_mut(&mut root, table).remove(key);
                } else {
                    table_mut(&mut root, table).insert(key.into(), toml_value(&field.value));
                }
            }
        }
        if changed.iter().any(|f| f.path.starts_with("cpu.rate_")) {
            let config = self.validation().config;
            table_mut(&mut root, "cpu").insert(
                "rate".into(),
                format!(
                    "{}/{}",
                    config.clock.retired_per_pit_clock,
                    config.clock.retired_per_pit_clock_denominator
                )
                .into(),
            );
        }
        for field in changed.iter().filter(|field| field.path.ends_with(".kind")) {
            let Some(old) = self.original.iter().find(|old| old.path == field.path) else {
                continue;
            };
            if old.value == field.value {
                continue;
            }
            let Value::Name(kind) = &field.value else {
                continue;
            };
            let id = field
                .path
                .strip_prefix("card[")
                .and_then(|tail| tail.strip_suffix("].kind"))
                .ok_or("Invalid card identity")?;
            if cards
                .get(id)
                .is_some_and(|card| card.get("remove") == Some(&toml::Value::Boolean(true)))
            {
                continue;
            }
            let mut replacement = cards.remove(id).ok_or("Missing replacement card")?;
            let mut new_id = kind.clone();
            while self.draft.card_ids.iter().any(|(_, id)| id == &new_id)
                || cards.contains_key(&new_id)
            {
                new_id.push_str("-custom");
            }
            replacement.insert("id".into(), new_id.clone().into());
            cards.insert(new_id, replacement);
            cards.insert(
                id.into(),
                toml::Table::from_iter([("id".into(), id.into()), ("remove".into(), true.into())]),
            );
        }
        cards.retain(|_, card| card.len() > 1 || !card.contains_key("id"));
        root.retain(|_, value| !value.as_table().is_some_and(|table| table.is_empty()));
        if !cards.is_empty() {
            root.insert(
                "card".into(),
                toml::Value::Array(cards.into_values().map(toml::Value::Table).collect()),
            );
        }
        let catalogue = self.draft.config.board.firmware_catalogue();
        let replacement = |value: &Value| -> Result<Option<toml::Table>, String> {
            match value {
                Value::Disconnected => Ok(None),
                Value::Name(id) => {
                    let entry = catalogue
                        .iter()
                        .find(|entry| entry.id == id)
                        .ok_or("Unknown firmware identity")?;
                    Ok(Some(catalogue_rom(entry)))
                }
                _ => Err("Invalid firmware identity".into()),
            }
        };
        if let Some(field) = firmware_changes
            .iter()
            .find(|field| field.path == "firmware.bios")
        {
            if let Some(rom) = replacement(&field.value)? {
                table_mut(&mut root, "firmware").insert("bios".into(), rom.into());
            }
        }
        let option_changes: Vec<_> = firmware_changes
            .iter()
            .filter(|field| field.path != "firmware.bios")
            .collect();
        if !option_changes.is_empty() {
            let mut options: Vec<toml::Value> = Vec::new();
            let mut replaced = Vec::new();
            for (index, rom) in self
                .document
                .firmware
                .iter()
                .flat_map(|f| f.option_roms.iter().flatten())
                .enumerate()
            {
                let path = format!("firmware.option_roms[{index}]");
                if let Some(field) = option_changes.iter().find(|field| field.path == path) {
                    if let Some(rom) = replacement(&field.value)? {
                        options.push(rom.into());
                    }
                    replaced.push(field.path.as_str());
                } else {
                    options.push(
                        rom_table(
                            rom,
                            &format!("firmware.option_roms[{index}]"),
                            &self.origins,
                            self.target.parent().ok_or("Missing output directory")?,
                        )?
                        .into(),
                    );
                }
            }
            for field in option_changes
                .iter()
                .filter(|field| !replaced.contains(&field.path.as_str()))
            {
                if let Some(rom) = replacement(&field.value)? {
                    options.push(rom.into());
                }
            }
            table_mut(&mut root, "firmware").insert("option_roms".into(), options.into());
        }
        self.preview = toml::to_string_pretty(&root).map_err(|e| e.to_string())?;
        machine_file::parse_toml(&self.target, &self.preview).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Write exactly the displayed text. Exclusive creation makes overwrite consent explicit.
    pub fn save(&mut self) -> Result<PathBuf, String> {
        if !self.can_apply() {
            return Err(
                "Settings cannot be applied: stop the machine and correct the reported fields"
                    .into(),
            );
        }
        let current = fs::read_to_string(&self.target).ok();
        if current != self.current_content {
            self.current_content = current;
            self.overwrite = false;
            return Err(
                "The local file changed. Review its current content before replacing it.".into(),
            );
        }
        if self.current_content.is_some() && !self.overwrite {
            return Err("A local child already exists. Confirm replacement before saving.".into());
        }
        let expected = self.preview.clone();
        self.refresh_preview()?;
        if expected != self.preview {
            return Err("Settings changed; review the updated preview".into());
        }
        let directory = self.target.parent().ok_or("Missing output directory")?;
        fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        if fs::symlink_metadata(directory)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Local directory must not be a symbolic link".into());
        }
        if fs::symlink_metadata(&self.target)
            .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
        {
            return Err("Output must be a regular local machine file".into());
        }
        let (parent, _) = machine_file::resolve(&self.parent).map_err(|e| e.to_string())?;
        if parent != self.document {
            return Err("The parent machine changed. Reopen Settings before saving.".into());
        }
        cleanup_staging(&self.target)?;
        let (staging, mut file) = create_staging(&self.target)?;
        let result: Result<(), String> = (|| {
            file.write_all(self.preview.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|e| e.to_string())?;
            let staged = Self::open(&staging, false, self.library.clone())?;
            let fields = |draft: &Draft| {
                let mut option_index = 0;
                caps::fields(draft)
                    .into_iter()
                    .filter(|field| {
                        !(field.path.ends_with(".enabled")
                            && field.value == Value::Name("none".into())
                            || field.path.starts_with("firmware.")
                                && field.value == Value::Disconnected)
                    })
                    .map(|field| {
                        let path = if field.path.starts_with("firmware.")
                            && field.path != "firmware.bios"
                        {
                            let path = format!("firmware.option_roms[{option_index}]");
                            option_index += 1;
                            path
                        } else {
                            field.path
                        };
                        let path = path
                            .strip_prefix("card[")
                            .and_then(|tail| tail.split_once("]."))
                            .and_then(|(id, suffix)| {
                                draft
                                    .card_ids
                                    .iter()
                                    .find(|(_, name)| name == id)
                                    .map(|(role, _)| format!("card[{role:?}].{suffix}"))
                            })
                            .unwrap_or(path);
                        (path, field.value)
                    })
                    .collect::<BTreeMap<_, _>>()
            };
            if staged.validation().has_errors() || fields(&staged.draft) != fields(&self.draft) {
                return Err("The child does not reproduce the validated settings. Reopen Settings before saving.".into());
            }
            if self.overwrite {
                fs::rename(&staging, &self.target)
            } else {
                fs::hard_link(&staging, &self.target)
            }
            .map_err(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    "A local child already exists. Confirm replacement before saving.".into()
                } else {
                    e.to_string()
                }
            })?;
            Ok(())
        })();
        drop(file);
        let cleanup = fs::remove_file(&staging);
        result?;
        if let Err(error) = cleanup {
            if error.kind() != io::ErrorKind::NotFound {
                return Err(error.to_string());
            }
        }
        let target = self.target.clone();
        let tab = self.tab;
        *self = Self::open(&target, self.read_only, self.library.clone())?;
        self.tab = tab;
        self.saved_preview = Some(self.preview.clone());

        Ok(self.target.clone())
    }
}

fn toml_value(value: &Value) -> toml::Value {
    match value {
        Value::Integer(value) => i64::from(*value).into(),
        Value::Name(name) | Value::FileName(name) => name.clone().into(),
        Value::Disconnected => "none".into(),
    }
}

fn table_mut<'a>(root: &'a mut toml::Table, name: &str) -> &'a mut toml::Table {
    let value = root
        .entry(name)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    match value {
        toml::Value::Table(table) => table,
        _ => unreachable!("settings sections are tables"),
    }
}

fn lexical_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

fn relative_path(from: &Path, to: &Path) -> Result<String, String> {
    let from = lexical_path(from);
    let to = lexical_path(to);
    let left: Vec<_> = from.components().collect();
    let right: Vec<_> = to.components().collect();
    let common = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    let mut path = PathBuf::new();
    for _ in common..left.len() {
        path.push("..");
    }
    for component in &right[common..] {
        if let Component::Normal(name) = component {
            path.push(name);
        }
    }
    path.to_str()
        .map(str::to_owned)
        .ok_or("Machine path is not Unicode".into())
}

fn catalogue_rom(entry: &FirmwareEntry) -> toml::Table {
    let binding = caps::firmware_binding(entry);
    let mut table = toml::Table::from_iter([
        ("image".into(), binding.filename.into()),
        ("sha256".into(), binding.sha256.into()),
        ("base".into(), (binding.base as i64).into()),
        (
            "provides".into(),
            binding
                .provides
                .iter()
                .map(|service| toml::Value::String(service.to_string()))
                .collect::<Vec<_>>()
                .into(),
        ),
    ]);
    if binding.offset != 0 {
        table.insert("offset".into(), (binding.offset as i64).into());
    }
    table
}

fn rom_table(
    rom: &file::Rom,
    path: &str,
    origins: &Origins,
    directory: &Path,
) -> Result<toml::Table, String> {
    let mut table = toml::Table::new();
    if let Some(image) = &rom.image {
        table.insert(
            "image".into(),
            relative_path(
                directory,
                &origins.image_path(&format!("{path}.image"), image),
            )?
            .into(),
        );
    }
    if let Some(pin) = &rom.sha256 {
        table.insert("sha256".into(), pin.as_str().into());
    }
    if let Some(base) = rom.base {
        table.insert("base".into(), i64::from(base).into());
    }
    if let Some(offset) = rom.offset {
        table.insert("offset".into(), (offset as i64).into());
    }
    if let Some(services) = &rom.provides {
        table.insert(
            "provides".into(),
            services
                .iter()
                .map(|s| {
                    toml::Value::String(
                        match s {
                            file::FirmwareService::Floppy => "floppy",
                            file::FirmwareService::FixedDisk => "fixed-disk",
                        }
                        .into(),
                    )
                })
                .collect::<Vec<_>>()
                .into(),
        );
    }
    Ok(table)
}

fn create_staging(target: &Path) -> Result<(PathBuf, fs::File), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let path = target.with_file_name(format!(
            ".{}.{}.{}.pending.toml",
            target.file_stem().unwrap_or_default().to_string_lossy(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                file.try_lock().map_err(|error| error.to_string())?;
                return Ok((path, file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
}
fn cleanup_staging(target: &Path) -> Result<(), String> {
    let directory = target.parent().ok_or("Missing output directory")?;
    let prefix = format!(
        ".{}.",
        target.file_stem().unwrap_or_default().to_string_lossy()
    );
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix)
            && (name.ends_with(".pending.toml") || name == format!("{prefix}pending.toml"))
            && entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            if let Ok(file) = fs::OpenOptions::new().write(true).open(entry.path()) {
                if file.try_lock().is_ok() {
                    fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly::*;
    fn card_kind_id(kind: file::CardKind) -> DeviceId {
        match kind {
            file::CardKind::Mda => DeviceId::Mda,
            file::CardKind::VgaText | file::CardKind::Vga => DeviceId::Vga,
            file::CardKind::Uart16550 => DeviceId::Uart,
            file::CardKind::Fdc765 => DeviceId::Fdc,
            file::CardKind::Ide => DeviceId::Ide,
            file::CardKind::Sb16 => DeviceId::Sb16,
            file::CardKind::LotechEms => DeviceId::LotechEms,
        }
    }

    fn fixture() -> (machine_file::test_support::TempDir, SettingsModel) {
        let temp = machine_file::test_support::TempDir::new("settings-sheet");
        fs::create_dir(temp.path().join("machines")).unwrap();
        fs::write(temp.path().join("machines/bios.bin"), vec![0xF4; 32768]).unwrap();
        let parent = temp.path().join("machines/example.toml");
        fs::write(&parent, "schema=1\nname='Example'\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n[[card]]\nkind='sb16'\n[[card]]\nkind='lotech-ems'\nframe=0xD000\n").unwrap();
        let model = SettingsModel::open(&parent, false, None).unwrap();
        (temp, model)
    }

    #[test]
    fn shipped_documents_save_memory_children_without_modifying_the_sources() {
        check_shipped_documents(&pc_harness::test_support::root().join("machines"));
    }

    fn copy_shipped_documents(source: &Path, destination: &Path) {
        fs::create_dir_all(destination.join("presets")).unwrap();
        for name in [
            "at-freedos-from-preset.toml",
            "at-freedos.toml",
            "at-freedos.yaml",
            "at-games-open.toml",
            "at-games.toml",
            "at-ide.toml",
            "presets/ibm-5170.toml",
        ] {
            fs::copy(source.join(name), destination.join(name)).unwrap();
        }
    }

    #[test]
    fn shipped_document_saves_ignore_local_fixture_state() {
        let source = machine_file::test_support::TempDir::new("settings-fixture-root");
        let machines = source.path().join("machines");
        copy_shipped_documents(
            &pc_harness::test_support::root().join("machines"),
            &machines,
        );
        fs::create_dir(machines.join("local")).unwrap();
        let local_files = [
            (
                "local/at-games-custom.toml",
                b"schema=1\nextends='../at-games.toml'\n".as_slice(),
            ),
            ("local/games.img", b"local disk".as_slice()),
            ("local/games.upfj", b"local journal".as_slice()),
            (
                "at-freedos.toml.local/diagnostics/report.txt",
                b"local report".as_slice(),
            ),
            ("untracked.toml", b"not a shipped machine".as_slice()),
        ];
        for (name, bytes) in local_files {
            fs::create_dir_all(machines.join(name).parent().unwrap()).unwrap();
            fs::write(machines.join(name), bytes).unwrap();
        }
        check_shipped_documents(&machines);
        for (name, bytes) in local_files {
            assert_eq!(fs::read(machines.join(name)).unwrap(), bytes);
        }
    }

    fn check_shipped_documents(source: &Path) {
        let output = machine_file::test_support::TempDir::new("shipped-settings-children");
        // Opening settings also reads the adjacent local child, before the test changes its target.
        let root = output.path().join("shipped/machines");
        copy_shipped_documents(source, &root);
        fs::create_dir(output.path().join("machines")).unwrap();
        let mut count = 0;
        for entry in fs::read_dir(&root).unwrap() {
            let path = entry.unwrap().path();
            if !matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("toml" | "yaml" | "yml")
            ) {
                continue;
            }
            let before = fs::read(&path).unwrap();
            let mut model = SettingsModel::open(&path, false, None)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let (document, _) = machine_file::resolve(&path).unwrap();
            let fields = caps::fields(&model.draft);
            for (path, expected) in [
                (
                    "memory.conventional_kib",
                    document
                        .memory
                        .as_ref()
                        .and_then(|m| m.conventional_kib)
                        .unwrap_or(assembly::descriptors::CONVENTIONAL_KIB.default),
                ),
                (
                    "memory.extended_kib",
                    document
                        .memory
                        .as_ref()
                        .and_then(|m| m.extended_kib)
                        .unwrap_or(assembly::descriptors::EXTENDED_KIB.default),
                ),
            ] {
                assert_eq!(
                    fields.iter().find(|f| f.path == path).unwrap().value,
                    Value::Integer(expected)
                );
            }
            for card in document.card.iter().flatten() {
                let id = card_kind_id(card.kind.unwrap());
                let name = &model
                    .draft
                    .card_ids
                    .iter()
                    .find(|(role, _)| *role == id)
                    .unwrap()
                    .1;
                for (key, expected) in [
                    ("port", card.port.map(u32::from)),
                    ("frame", card.frame.map(u32::from)),
                    ("base", card.base.map(u32::from)),
                    ("size_kib", card.size_kib),
                    ("dma8", card.dma8.map(u32::from)),
                    ("dma16", card.dma16.map(u32::from)),
                ] {
                    if let Some(expected) = expected {
                        assert_eq!(
                            fields
                                .iter()
                                .find(|f| f.path == format!("card[{name}].{key}"))
                                .unwrap()
                                .value,
                            Value::Integer(expected)
                        );
                    }
                }
            }
            assert!(
                !model.validation().has_errors(),
                "{}: {:?}",
                path.display(),
                model.validation().diagnostics
            );
            assert!(!model.can_apply());
            model.target = output
                .path()
                .canonicalize()
                .unwrap()
                .join(format!("machines/local/profile-{count}-custom.toml"));
            model.layer.insert(
                "extends".into(),
                relative_path(model.target.parent().unwrap(), &model.parent)
                    .unwrap()
                    .into(),
            );
            model.set("memory.conventional_kib", Value::Integer(512));
            let expected = model.validation().config;
            let preview = model.preview.clone();
            let child = model
                .save()
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(fs::read_to_string(&child).unwrap(), preview);
            assert_eq!(
                SettingsModel::open(&child, false, None)
                    .unwrap()
                    .validation()
                    .config,
                expected
            );
            assert_eq!(fs::read(&path).unwrap(), before);
            assert_eq!(
                fs::read(source.join(path.file_name().unwrap())).unwrap(),
                before
            );
            count += 1;
        }
        assert!(count >= 6, "covered {count} shipped machines");
    }

    #[test]
    fn preview_is_exact_child_bytes_and_round_trips_to_the_same_effective_configuration() {
        let (_temp, mut model) = fixture();
        let original_parent = model.parent.clone();
        let parent_before = fs::read(&original_parent).unwrap();
        model.set("memory.extended_kib", Value::Integer(1024));
        model.set("card[sb16].base", Value::Integer(0x240));
        assert!(model.can_apply(), "{:?}", model.validation().diagnostics);
        let preview = model.preview.clone();
        let expected = model.validation().config;
        let path = model.save().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), preview);
        assert!(!model.can_apply());
        let reopened = SettingsModel::open(&path, false, None).unwrap();
        assert_eq!(reopened.validation().config, expected);
        let loaded = machine_file::load_with_options(
            &path,
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        loaded.validate().unwrap();
        assert_eq!(loaded.memory, expected.memory);
        assert_eq!(loaded.devices, expected.devices);
        let child = machine_file::parse_toml(&path, &preview).unwrap();
        assert!(child.extends.is_some());
        assert!(child.firmware.is_none());
        assert!(child.cpu.is_none());
        assert_eq!(child.card.unwrap().len(), 1);
        assert_eq!(fs::read(&original_parent).unwrap(), parent_before);
    }

    #[test]
    fn invalid_draft_shows_the_api_reason_and_cannot_write() {
        let (_temp, mut model) = fixture();
        let choice = caps::choices(&model.draft, "card[sb16].base")
            .into_iter()
            .find(|c| c.value == Value::Integer(0x260))
            .unwrap();
        assert!(!choice.enabled);
        model.set("card[sb16].base", choice.value);
        assert!(!model.can_apply());
        assert!(model
            .validation()
            .diagnostics
            .iter()
            .any(|d| d.reason == *choice.reason.as_ref().unwrap()
                && d.field_paths.contains(&"card[sb16].base".into())));
        assert!(model.save().is_err());
        assert!(!model.target.exists());
    }

    #[test]
    fn running_and_paused_sessions_are_read_only_and_overwrites_require_consent() {
        let (_temp, mut model) = fixture();
        let initial = model.preview.clone();
        model.read_only = true;
        model.set("memory.extended_kib", Value::Integer(1024));
        assert_eq!(model.preview, initial);
        assert!(!model.can_apply());
        assert!(model.save().is_err());
        model.read_only = false;
        model.set("memory.extended_kib", Value::Integer(1024));
        model.save().unwrap();
        model.set("memory.extended_kib", Value::Integer(2048));
        let before = fs::read(&model.target).unwrap();
        assert!(model.save().unwrap_err().contains("Confirm replacement"));
        assert_eq!(fs::read(&model.target).unwrap(), before);
        model.overwrite = true;
        model.save().unwrap();
        assert_eq!(fs::read_to_string(&model.target).unwrap(), model.preview);
        model.set("memory.extended_kib", Value::Integer(3072));
        model.set("memory.extended_kib", Value::Integer(2048));
        assert!(!model.can_apply());
        model.set("memory.extended_kib", Value::Integer(3072));
        assert!(!model.overwrite);
        model.read_only = true;
        assert!(model.save().is_err());
    }

    #[test]
    fn cancel_and_reverted_edits_leave_the_parent_unchanged() {
        let (_temp, mut model) = fixture();
        let before = fs::read(&model.parent).unwrap();
        model.set("memory.extended_kib", Value::Integer(1024));
        model.set("memory.extended_kib", Value::Integer(0));
        assert!(!model.can_apply());
        assert!(!model.preview.contains("extended_kib"));
        assert!(!model.target.exists());
        drop(model);
        // Dropping the dialog is Cancel: there is no publication side effect.
        assert_eq!(
            fs::read(_temp.path().join("machines/example.toml")).unwrap(),
            before
        );
    }
    #[test]
    fn optional_card_removal_and_reinstallation_round_trip_without_stale_resources() {
        let (_temp, mut model) = fixture();
        model.set("card[lotech-ems].enabled", Value::Name("none".into()));
        let child = model.save().unwrap();
        let mut reopened = SettingsModel::open(&child, false, None).unwrap();
        assert!(!reopened
            .validation()
            .config
            .devices
            .iter()
            .any(|d| d.id == DeviceId::LotechEms));
        reopened.set("card[lotech-ems].enabled", Value::Name("installed".into()));
        assert!(
            reopened.can_apply(),
            "{:?}",
            reopened.validation().diagnostics
        );
        let expected = reopened.validation().config;
        assert!(reopened.save().is_err());
        reopened.overwrite = true;
        let child = reopened.save().unwrap();
        assert_eq!(
            SettingsModel::open(&child, false, None)
                .unwrap()
                .validation()
                .config,
            expected
        );
    }

    #[test]
    fn coprocessor_and_rate_changes_round_trip_through_a_local_child() {
        let (_temp, mut model) = fixture();
        model.set("cpu.fpu", Value::Name("80287".into()));
        model.set("cpu.rate_steps", Value::Integer(16));
        model.set("cpu.rate_pit_clocks", Value::Integer(13));
        let expected = model.validation().config;
        let child = model.save().unwrap();
        let reopened = SettingsModel::open(&child, false, None).unwrap();
        assert_eq!(reopened.validation().config, expected);
        let loaded = machine_file::load_with_options(
            &child,
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        assert_eq!(loaded.cpu.fpu, FpuChip::I80287);
        assert_eq!(loaded.clock, expected.clock);
    }

    #[test]
    fn changed_parent_and_exhausted_inheritance_depth_cannot_publish() {
        let (temp, mut model) = fixture();
        model.set("memory.extended_kib", Value::Integer(1024));
        let text = fs::read_to_string(&model.parent).unwrap();
        fs::write(
            &model.parent,
            text.replace("kind='sb16'", "kind='sb16'\nbase=0x240"),
        )
        .unwrap();
        assert!(model.save().unwrap_err().contains("parent machine changed"));
        assert!(!model.target.exists());
        let root = temp.path().join("machines");
        let mut previous = "example.toml".to_string();
        for index in 0..6 {
            let name = format!("layer-{index}.toml");
            fs::write(
                root.join(&name),
                format!("schema=1\nextends='{previous}'\n"),
            )
            .unwrap();
            previous = name;
        }
        let mut deep = SettingsModel::open(&root.join(previous), false, None).unwrap();
        deep.set("memory.extended_kib", Value::Integer(1024));
        assert!(deep.save().is_err());
        assert!(!deep.target.exists());
        assert!(fs::read_dir(root.join("local")).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("pending")));
    }

    #[test]
    fn firmware_option_removal_round_trips_and_preserves_the_system_pin() {
        let (_temp, mut model) = fixture();
        let mut text = fs::read_to_string(&model.parent).unwrap();
        let entry = FirmwareEntry::XUB_R638_XTP_ATA1F0;
        text.push_str(&format!(
            "\n[[firmware.option_roms]]\nimage='option.bin'\nsha256='{}'\nbase=0xC8000\n",
            entry.sha256
        ));
        fs::write(&model.parent, text).unwrap();
        model = SettingsModel::open(&model.parent, false, None).unwrap();
        model.set("firmware.option_roms", Value::Disconnected);
        assert!(model.can_apply(), "{:?}", model.validation().diagnostics);
        let expected = model.validation().config;
        let child = model.save().unwrap();
        let reopened = SettingsModel::open(&child, false, None).unwrap();
        assert_eq!(reopened.validation().config, expected);
        assert!(model.preview.contains("option_roms = []"));
        assert!(!model.preview.contains("bios"));
    }
    #[test]
    fn invalid_inherited_field_keeps_its_value_and_can_be_corrected_in_the_child() {
        let (_temp, mut model) = fixture();
        let text = fs::read_to_string(&model.parent)
            .unwrap()
            .replace("kind='sb16'", "kind='sb16'\nirq=6");
        fs::write(&model.parent, text).unwrap();
        model = SettingsModel::open(&model.parent, false, None).unwrap();
        assert_eq!(
            caps::fields(&model.draft)
                .iter()
                .find(|field| field.path == "card[sb16].irq")
                .unwrap()
                .value,
            Value::Integer(6)
        );
        assert!(!model.can_apply());
        model.set("card[sb16].irq", Value::Integer(5));
        assert!(model.can_apply());
        let child = model.save().unwrap();
        let loaded = machine_file::load_with_options(
            &child,
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        assert_eq!(
            loaded
                .devices
                .iter()
                .find(|device| device.id == DeviceId::Sb16)
                .unwrap()
                .irq,
            Some(5)
        );
    }

    #[test]
    fn display_card_change_and_removal_are_saved_as_one_valid_child() {
        let (_temp, mut model) = fixture();
        model.set("card[mda].enabled", Value::Name("none".into()));
        model.set("card[vga-text].kind", Value::Name("vga".into()));
        assert!(model.can_apply(), "{:?}", model.validation().diagnostics);
        let expected = model.validation().config;
        let child = model.save().unwrap();
        assert_eq!(
            SettingsModel::open(&child, false, None)
                .unwrap()
                .validation()
                .config,
            expected
        );
        let loaded = machine_file::load_with_options(
            &child,
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        assert!(loaded
            .devices
            .iter()
            .any(|device| matches!(device.kind, DeviceKind::VgaRegisters(_))));
        assert!(!loaded
            .devices
            .iter()
            .any(|device| device.id == DeviceId::Mda));
    }

    #[test]
    fn optional_card_with_a_custom_identity_can_be_removed() {
        let (_temp, mut model) = fixture();
        let text = fs::read_to_string(&model.parent)
            .unwrap()
            .replace("kind='sb16'", "id='audio'\nkind='sb16'");
        fs::write(&model.parent, text).unwrap();
        model = SettingsModel::open(&model.parent, false, None).unwrap();
        model.set("card[audio].enabled", Value::Name("none".into()));
        let child = model.save().unwrap();
        assert!(!SettingsModel::open(&child, false, None)
            .unwrap()
            .validation()
            .config
            .devices
            .iter()
            .any(|device| device.id == DeviceId::Sb16));
        assert!(model.preview.contains("id = \"audio\""));
    }

    #[test]
    fn inherited_board_wiring_and_calendar_match_normal_lowering() {
        let (_temp, mut model) = fixture();
        let mut text = fs::read_to_string(&model.parent).unwrap();
        text.push_str("\n[board]\npost_port=768\nmouse=true\n[board.rtc]\nstart='2001-02-03T04:05:06'\nnvram='blank'\n");
        fs::write(&model.parent, text).unwrap();
        model = SettingsModel::open(&model.parent, false, None).unwrap();
        let loaded = machine_file::load_with_options(
            &model.parent,
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        assert_eq!(model.validation().config.devices, loaded.devices);
        model.set("memory.extended_kib", Value::Integer(1024));
        let child = model.save().unwrap();
        assert_eq!(
            SettingsModel::open(&child, false, None)
                .unwrap()
                .validation()
                .config
                .devices,
            loaded.devices
        );
    }
    #[test]
    fn known_firmware_requires_a_fixed_disk_provider_without_rom_bytes() {
        let path = pc_harness::test_support::root().join("machines/at-games-open.toml");
        let mut model = SettingsModel::open(&path, false, None).unwrap();
        let choice = caps::choices(&model.draft, "firmware.option_roms")
            .into_iter()
            .find(|choice| choice.value == Value::Disconnected)
            .unwrap();
        assert!(!choice.enabled);
        model.set("firmware.option_roms", Value::Disconnected);
        assert!(!model.can_apply());
        assert!(model
            .validation()
            .diagnostics
            .iter()
            .any(|d| d.code == "firmware-service" && d.severity == caps::Severity::Error));
    }

    #[test]
    fn known_firmware_keeps_invalid_service_declarations_visible() {
        let (_temp, mut model) = fixture();
        let text = fs::read_to_string(&model.parent).unwrap().replace(
            "provides=['floppy']",
            &format!(
                "sha256='{}'\nprovides=['fixed-disk']",
                KISELEV_XI8088.sha256
            ),
        );
        fs::write(&model.parent, text).unwrap();
        model = SettingsModel::open(&model.parent, false, None).unwrap();
        model.set("memory.extended_kib", Value::Integer(1024));
        assert!(!model.can_apply());
        assert!(model
            .validation()
            .diagnostics
            .iter()
            .any(|d| d.code == "firmware-provides" && d.field_paths == ["firmware.bios"]));
    }
    fn fields(draft: &Draft) -> BTreeMap<String, Value> {
        caps::fields(draft)
            .into_iter()
            .map(|field| (field.path, field.value))
            .collect()
    }

    fn pin(bytes: &[u8]) -> file::Sha256 {
        use sha2::{Digest, Sha256};
        machine_file::parse_toml(
            "pin.toml",
            &format!(
                "schema=1\n[firmware.bios]\nsha256='{:x}'\n",
                Sha256::digest(bytes)
            ),
        )
        .unwrap()
        .firmware
        .unwrap()
        .bios
        .unwrap()
        .sha256
        .unwrap()
    }

    fn artifact(directory: &Path, name: &str, bytes: &[u8]) -> (String, file::Sha256) {
        let path = directory.join(name);
        fs::write(&path, bytes).unwrap();
        (path.to_string_lossy().into_owned(), pin(bytes))
    }

    fn option_bytes(size: usize, offset: usize) -> Vec<u8> {
        let mut bytes = vec![0; size];
        bytes[offset] = 0x55;
        bytes[offset + 1] = 0xAA;
        bytes[offset + 2] = ((size - offset) / 512) as u8;
        let sum = bytes[offset..]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        bytes[size - 1] = bytes[size - 1].wrapping_sub(sum);
        bytes
    }

    fn open_bios() -> Option<Vec<u8>> {
        open_firmware("unemu-at-bios/bios-unemu-at.bin")
    }

    fn open_firmware(name: &str) -> Option<Vec<u8>> {
        let path = pc_harness::test_support::root().join("vendor").join(name);
        match fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && std::env::var_os("UNEMUPC_REQUIRE_FIXTURES").is_none() =>
            {
                eprintln!("skipping open-firmware check: fetch {name}");
                None
            }
            Err(error) => panic!("cannot read the open firmware fixture {name}: {error}"),
        }
    }

    fn fixture_variant(
        mut document: MachineFile,
        origins: &Origins,
        directory: &Path,
        bios_bytes: &[u8],
    ) -> MachineFile {
        let original = document.describe(origins).unwrap();
        let catalogue = BoardModel::Ibm5170.firmware_catalogue();
        let firmware = document.firmware.as_mut().unwrap();
        let bios = firmware.bios.as_mut().unwrap();
        bios.base = None;
        bios.sha256 = Some(pin(bios_bytes));
        bios.provides = Some(vec![file::FirmwareService::Floppy]);
        if let Some(parts) = &mut bios.parts {
            assert_eq!(parts.len(), 2);
            for (index, part) in parts.iter_mut().enumerate() {
                let lane = usize::from(part.lane == file::Lane::Odd);
                let bytes: Vec<_> = bios_bytes.iter().skip(lane).step_by(2).copied().collect();
                let (path, digest) = artifact(directory, &format!("bios-part-{index}.bin"), &bytes);
                part.image = path;
                part.sha256 = Some(digest);
                part.offset = 0;
            }
        } else {
            bios.image = Some(artifact(directory, "bios.bin", bios_bytes).0);
        }
        let mut fixed_disk_provider = false;
        for (index, rom) in firmware.option_roms.iter_mut().flatten().enumerate() {
            let entry = rom
                .sha256
                .as_ref()
                .and_then(|pin| catalogue.iter().find(|entry| entry.sha256 == pin.as_str()));
            let size = entry.map_or(8192, |entry| entry.size as usize);
            let offset = rom
                .offset
                .unwrap_or_else(|| entry.map_or(0, |entry| entry.rom_offset));
            rom.provides = Some(entry.map_or_else(
                || rom.provides.clone().unwrap_or_default(),
                |entry| {
                    entry
                        .provides
                        .iter()
                        .map(|service| match service {
                            FirmwareService::Floppy => file::FirmwareService::Floppy,
                            FirmwareService::FixedDisk => file::FirmwareService::FixedDisk,
                        })
                        .collect()
                },
            ));
            fixed_disk_provider |= rom
                .provides
                .as_ref()
                .is_some_and(|services| services.contains(&file::FirmwareService::FixedDisk));
            let (path, digest) = artifact(
                directory,
                &format!("option-{index}.bin"),
                &option_bytes(size, offset),
            );
            rom.image = Some(path);
            rom.sha256 = Some(digest);
            rom.offset = Some(offset);
        }
        if original
            .config
            .drives
            .iter()
            .any(|drive| matches!(drive.kind, DriveKind::HardDisk { .. }))
            && !fixed_disk_provider
        {
            let (path, digest) = artifact(directory, "fixed-disk-rom.bin", &option_bytes(8192, 0));
            firmware
                .option_roms
                .get_or_insert_default()
                .push(file::Rom {
                    image: Some(path),
                    sha256: Some(digest),
                    base: Some(0xC8000),
                    offset: None,
                    provides: Some(vec![file::FirmwareService::FixedDisk]),
                    parts: None,
                });
        }
        let mut drive_index = 0;
        for card in document.card.iter_mut().flatten() {
            for drive in card.drives.iter_mut().flatten() {
                let size = original.config.drives[drive_index].kind.image_bytes() as usize;
                if let Some(media) = &mut drive.media {
                    for (index, medium) in media.iter_mut().enumerate() {
                        let (path, digest) = artifact(
                            directory,
                            &format!("drive-{drive_index}-medium-{index}.img"),
                            &vec![0; size],
                        );
                        medium.image = path;
                        medium.sha256 = Some(digest);
                        if medium.write == Some(file::WriteMode::Journal) {
                            medium.journal = Some(
                                directory
                                    .join(format!("medium-{drive_index}-{index}.upfj"))
                                    .to_string_lossy()
                                    .into_owned(),
                            );
                        }
                    }
                } else if drive.empty != Some(true) {
                    // An empty drive without a catalogue takes no image.
                    let (path, digest) = artifact(
                        directory,
                        &format!("drive-{drive_index}.img"),
                        &vec![0; size],
                    );
                    drive.image = Some(path);
                    drive.sha256 = Some(digest);
                    if drive.write == Some(file::WriteMode::Journal) {
                        drive.journal = Some(
                            directory
                                .join(format!("drive-{drive_index}.upfj"))
                                .to_string_lossy()
                                .into_owned(),
                        );
                    }
                }
                drive_index += 1;
            }
        }
        document
    }

    #[test]
    fn fix_round_every_shipped_sheet_field_matches_normal_lowering_with_open_and_synthetic_bindings(
    ) {
        let Some(bios) = open_bios() else {
            return;
        };
        let root = pc_harness::test_support::root().join("machines");
        let mut count = 0;
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if !matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("toml" | "yaml" | "yml")
            ) {
                continue;
            }
            let temp = machine_file::test_support::TempDir::new("normal-lowering-parity");
            let (document, origins) = machine_file::resolve(&path).unwrap();
            let variant = fixture_variant(document, &origins, temp.path(), &bios);
            let metadata = variant.describe(&origins).unwrap();
            let normal = machine_file::lower_with_options(
                &variant,
                &origins,
                &mut machine_file::FileImageReader,
                AssemblyOptions {
                    allow_unlisted_firmware: true,
                },
            )
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let mut expected = metadata.clone();
            expected.config = normal;
            let mut fixture_sheet = SettingsModel::open(&path, false, None).unwrap();
            fixture_sheet.document = variant;
            fixture_sheet.draft = metadata;
            assert_eq!(
                fields(&fixture_sheet.draft),
                fields(&expected),
                "{}",
                path.display()
            );
            assert_eq!(
                caps::storage_summary(&fixture_sheet.draft),
                caps::storage_summary(&expected)
            );
            assert_eq!(
                fixture_sheet
                    .draft
                    .config
                    .images
                    .iter()
                    .map(|image| image.base)
                    .collect::<Vec<_>>(),
                expected
                    .config
                    .images
                    .iter()
                    .map(|image| image.base)
                    .collect::<Vec<_>>()
            );
            let sheet = SettingsModel::open(&path, false, None).unwrap();
            for field in caps::fields(&sheet.draft)
                .iter()
                .filter(|field| field.group != caps::Group::Firmware)
            {
                assert_eq!(
                    fields(&expected).get(&field.path),
                    Some(&field.value),
                    "{}: {}",
                    path.display(),
                    field.path
                );
            }
            count += 1;
        }
        assert_eq!(count, 6);
    }

    fn parts_and_options_fixture() -> (machine_file::test_support::TempDir, SettingsModel) {
        let temp = machine_file::test_support::TempDir::new("parts-and-video-settings");
        fs::create_dir_all(temp.path().join("machines")).unwrap();
        fs::create_dir_all(temp.path().join("vendor/user-roms")).unwrap();
        fs::create_dir_all(temp.path().join("external")).unwrap();
        let bytes = vec![0xF4; 65536];
        let (even, even_pin) = artifact(&temp.path().join("vendor"), "even.bin", &bytes[..32768]);
        let (odd, odd_pin) = artifact(&temp.path().join("vendor"), "odd.bin", &bytes[..32768]);
        let (_, video_pin) = artifact(
            &temp.path().join("external"),
            "video.bin",
            &option_bytes(32768, 8192),
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            "../../external/video.bin",
            temp.path().join("vendor/user-roms/video-link.bin"),
        )
        .unwrap();
        #[cfg(not(unix))]
        fs::copy(
            temp.path().join("external/video.bin"),
            temp.path().join("vendor/user-roms/video-link.bin"),
        )
        .unwrap();
        let (first, first_pin) = artifact(
            &temp.path().join("vendor"),
            "first.bin",
            &option_bytes(8192, 0),
        );
        let (second, second_pin) = artifact(
            &temp.path().join("vendor"),
            "second.bin",
            &option_bytes(8192, 0),
        );
        let parent = temp.path().join("machines/parts.toml");
        let text = format!("schema=1\nname='Parts machine'\nextends='preset:ibm-5170'\n[firmware.bios]\nsha256='{}'\nprovides=['floppy','fixed-disk']\nparts=[{{image='{even}',lane='even',sha256='{}'}},{{image='{odd}',lane='odd',sha256='{}'}}]\n[[firmware.option_roms]]\nimage='../vendor/user-roms/video-link.bin'\nsha256='{}'\nbase=0xC0000\noffset=8192\n[[firmware.option_roms]]\nimage='{first}'\nsha256='{}'\nbase=0xC8000\nprovides=['fixed-disk']\n[[firmware.option_roms]]\nimage='{second}'\nsha256='{}'\nbase=0xD0000\n[[card]]\nid='mda'\nremove=true\n[[card]]\nid='vga-text'\nremove=true\n[[card]]\nkind='vga'\n[[card]]\nkind='ide'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='1.2M'\nmedia=[]\nempty=true\n", pin(&bytes).as_str(), even_pin.as_str(), odd_pin.as_str(), video_pin.as_str(), first_pin.as_str(), second_pin.as_str());
        fs::write(&parent, text).unwrap();
        let model = SettingsModel::open(&parent, false, None).unwrap();
        (temp, model)
    }

    #[test]
    fn fix_round_indexed_rewrite_preserves_other_roms_and_symlink_spelling() {
        let (_temp, mut model) = parts_and_options_fixture();
        let parent = model.parent.clone();
        let before = fs::read(&parent).unwrap();
        model.set(
            "firmware.option_roms[1]",
            Value::Name(FirmwareEntry::XUB_R638_XTP_ATA1F0.id.into()),
        );
        assert!(model.can_apply(), "{:?}", model.validation().diagnostics);
        let path = model.save().unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let layer = machine_file::parse_toml(&path, &text).unwrap();
        let options = layer.firmware.unwrap().option_roms.unwrap();
        assert_eq!(options.len(), 3);
        assert_eq!(
            options[0].image.as_deref(),
            Some("../../vendor/user-roms/video-link.bin")
        );
        assert!(options[2]
            .image
            .as_deref()
            .unwrap()
            .ends_with("vendor/second.bin"));
        assert_eq!(
            options[1].sha256.as_ref().unwrap().as_str(),
            FirmwareEntry::XUB_R638_XTP_ATA1F0.sha256
        );
        let (resolved, _) = machine_file::resolve(&path).unwrap();
        assert_eq!(
            resolved
                .firmware
                .unwrap()
                .bios
                .unwrap()
                .parts
                .unwrap()
                .len(),
            2
        );
        assert_eq!(fs::read(parent).unwrap(), before);
    }

    #[test]
    fn fix_round_firmware_selection_and_drive_type_round_trip_without_discovery() {
        let (_temp, mut model) = parts_and_options_fixture();
        model.set("firmware.bios", Value::Name(KISELEV_XI8088.id.into()));
        model.set(
            "card[fdc-765].drives[unit=0].type",
            Value::Name("1.44M".into()),
        );
        let expected = fields(&model.draft);
        assert!(model.can_apply(), "{:?}", model.validation().diagnostics);
        let target = model.save().unwrap();
        let reopened = SettingsModel::open(&target, false, None).unwrap();
        assert_eq!(fields(&reopened.draft), expected);
        let (file, _) = machine_file::resolve(&target).unwrap();
        let bios = file.firmware.unwrap().bios.unwrap();
        assert!(bios.parts.is_none());
        assert_eq!(bios.sha256.unwrap().as_str(), KISELEV_XI8088.sha256);
        assert_eq!(bios.image.as_deref(), Some("kiselev-xi8088-1.0.2.bin"));
        let text = fs::read_to_string(target).unwrap();
        assert_eq!(model.current_content.as_deref(), Some(text.as_str()));
    }

    #[test]
    fn fix_round_reediting_uses_the_same_child_and_requires_its_current_text_and_consent() {
        let (_temp, mut model) = fixture();
        model.set("memory.extended_kib", Value::Integer(1024));
        let target = model.save().unwrap();
        let current = fs::read_to_string(&target).unwrap();
        let mut again = SettingsModel::open(&target, false, None).unwrap();
        assert_eq!(again.target, target);
        assert_eq!(again.current_content.as_deref(), Some(current.as_str()));
        again.set("memory.extended_kib", Value::Integer(2048));
        assert!(again.diff_preview().contains("- extended_kib = 1024"));
        assert!(again.diff_preview().contains("+ extended_kib = 2048"));
        assert!(again.save().is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), current);
        again.overwrite = true;
        assert_eq!(again.save().unwrap(), target);
        assert_eq!(again.name.matches("(custom)").count(), 1);
        assert!(!again.target.to_string_lossy().contains("custom-custom"));
    }

    #[test]
    fn fix_round_stale_staging_is_cleaned_and_active_staging_is_not_reused() {
        let (_temp, mut model) = fixture();
        model.set("memory.extended_kib", Value::Integer(1024));
        fs::create_dir_all(model.target.parent().unwrap()).unwrap();
        let legacy = model.target.with_file_name(".example-custom.pending.toml");
        fs::write(&legacy, "stale").unwrap();
        let (active, file) = create_staging(&model.target).unwrap();
        let target = model.save().unwrap();
        assert!(target.exists());
        assert!(!legacy.exists());
        assert!(active.exists());
        drop(file);
        cleanup_staging(&target).unwrap();
        assert!(!active.exists());
    }

    #[test]
    fn fix_round_kind_change_then_removal_writes_only_the_inherited_removal() {
        let (_temp, mut model) = fixture();
        model.set("card[vga-text].kind", Value::Name("vga".into()));
        model.set("card[vga-text].enabled", Value::Name("none".into()));
        let target = model.save().unwrap();
        let layer =
            machine_file::parse_toml(&target, &fs::read_to_string(&target).unwrap()).unwrap();
        let cards = layer.card.unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id.as_deref(), Some("vga-text"));
        assert_eq!(cards[0].remove, Some(true));
        assert!(cards[0].kind.is_none());
    }

    #[test]
    fn fix_round_option_rewrite_without_a_bios_uses_the_document_index() {
        let (_temp, mut model) = parts_and_options_fixture();
        model.document.firmware.as_mut().unwrap().bios = None;
        model.draft = model.document.describe(&model.origins).unwrap();
        model.original = caps::fields(&model.draft);
        model.set(
            "firmware.option_roms[1]",
            Value::Name(FirmwareEntry::XUB_R638_XTP_ATA1F0.id.into()),
        );
        let layer = machine_file::parse_toml(&model.target, &model.preview).unwrap();
        let options = layer.firmware.unwrap().option_roms.unwrap();
        assert_eq!(
            options[0].image.as_deref(),
            Some("../../vendor/user-roms/video-link.bin")
        );
        assert_eq!(
            options[1].sha256.as_ref().unwrap().as_str(),
            FirmwareEntry::XUB_R638_XTP_ATA1F0.sha256
        );
        assert!(options[2]
            .image
            .as_deref()
            .unwrap()
            .ends_with("vendor/second.bin"));
        assert_eq!(model.draft.config.system_bios_image, None);
    }
    #[test]
    fn removing_one_indexed_option_keeps_the_other_option_and_its_identity() {
        let (_temp, mut model) = parts_and_options_fixture();
        model.set("firmware.option_roms[1]", Value::Disconnected);
        let target = model.save().unwrap();
        let (document, _) = machine_file::resolve(&target).unwrap();
        let options = document.firmware.unwrap().option_roms.unwrap();
        assert_eq!(options.len(), 2);
        assert!(options[0]
            .image
            .as_deref()
            .unwrap()
            .ends_with("video-link.bin"));
        assert!(options[1].image.as_deref().unwrap().ends_with("second.bin"));
    }

    #[test]
    fn saved_rom_and_drive_rewrites_match_normal_verified_lowering() {
        let Some(bios) = open_firmware("kiselev-bios/bios-xi8088.bin") else {
            return;
        };
        let Some(option) = open_firmware("xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin") else {
            return;
        };
        let (_temp, mut model) = parts_and_options_fixture();
        let parent = model.parent.clone();
        let before = fs::read(&parent).unwrap();
        model.set("firmware.bios", Value::Name(KISELEV_XI8088.id.into()));
        model.set(
            "firmware.option_roms[1]",
            Value::Name(FirmwareEntry::XUB_R638_XTP_ATA1F0.id.into()),
        );
        model.set(
            "card[fdc-765].drives[unit=0].type",
            Value::Name("1.44M".into()),
        );
        let preview = model.preview.clone();
        let target = model.save().unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), preview);
        let directory = target.parent().unwrap();
        fs::write(
            directory.join(caps::firmware_binding(&KISELEV_XI8088).filename),
            bios,
        )
        .unwrap();
        fs::write(
            directory.join(caps::firmware_binding(&FirmwareEntry::XUB_R638_XTP_ATA1F0).filename),
            option,
        )
        .unwrap();
        let normal = machine_file::load_with_options(
            &target,
            AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        let sheet = SettingsModel::open(&target, false, None).unwrap();
        let mut expected = sheet.draft.clone();
        expected.config = normal;
        assert_eq!(fields(&sheet.draft), fields(&expected));
        assert_eq!(
            caps::storage_summary(&sheet.draft),
            caps::storage_summary(&expected)
        );
        assert_eq!(fs::read(parent).unwrap(), before);
    }
}
