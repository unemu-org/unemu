// SPDX-License-Identifier: AGPL-3.0-or-later
//! The library's machine files: the files of the machine-file directory, the
//! row each lists as, its readiness check on a host thread and the
//! diagnostics a row shows. They consume the existing machine-file validator;
//! see `backlog/docs/design/machine-images.md` §7 and `backlog/docs/architecture.md` § Observation and presentation.

use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct LibraryEntry {
    pub path: PathBuf,
    pub name: String,
    pub details: Vec<(String, String)>,
    pub warning: Option<String>,
    pub ems: String,
    pub rate: String,
    pub floppy: [bool; 2],
    pub checked: bool,
    pub discovery_warnings: Vec<String>,
    source: Option<String>,
}

impl LibraryEntry {
    pub fn pending(path: PathBuf) -> Self {
        let source = source_revision(&path);
        Self {
            name: machine_file::resolve(&path)
                .ok()
                .and_then(|(file, _)| file.name)
                .unwrap_or_else(|| {
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                }),
            path,
            details: Vec::new(),
            warning: Some("Checking firmware…".into()),
            ems: "EMS unknown".into(),
            rate: String::new(),
            floppy: [false; 2],
            checked: false,
            discovery_warnings: Vec::new(),
            source,
        }
    }

    #[cfg(test)]
    pub fn inspect(path: PathBuf, options: assembly::AssemblyOptions) -> Self {
        Self::inspect_with_rom_dirs(path, options, &[])
    }

    /// A machine's row read at once, with firmware found in `roots`, for the
    /// tests of this crate and of the desktop's shell.
    #[cfg(any(test, feature = "test-support"))]
    pub fn inspect_with_rom_dirs(
        path: PathBuf,
        options: assembly::AssemblyOptions,
        roots: &[PathBuf],
    ) -> Self {
        let roots = roots
            .iter()
            .map(|path| {
                machine_file::firmware_discovery::FirmwareRoot::new(
                    path,
                    machine_file::firmware_discovery::RootOrigin::CommandLine,
                )
            })
            .collect::<Vec<_>>();
        Self::prepare(
            path,
            options,
            &roots,
            None,
            None,
            &pc_image::compat::compatibility(image_store::compat::EXTRACTION_BASELINE),
        )
        .0
    }

    fn prepare(
        path: PathBuf,
        options: assembly::AssemblyOptions,
        roots: &[machine_file::firmware_discovery::FirmwareRoot],
        control: Option<&machine_file::firmware_discovery::DiscoveryControl>,
        store_root: Option<&Path>,
        compatibility: &image_store::compat::Compatibility,
    ) -> PreparedEntry {
        let name = machine_file::resolve(&path)
            .ok()
            .and_then(|(file, _)| file.name)
            .unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        let source = source_revision(&path);
        let mut entry = Self {
            path,
            name,
            details: Vec::new(),
            warning: None,
            ems: "EMS unknown".into(),
            rate: String::new(),
            floppy: [false; 2],
            checked: true,
            discovery_warnings: Vec::new(),
            source,
        };
        let mut prepared = None;
        match pc_image::firmware::prepare_for_assembly_with_compatibility(
            &entry.path,
            roots,
            &mut entry.discovery_warnings,
            store_root,
            compatibility,
        )
        .and_then(|document| {
            if let Some(control) = control {
                control.loading();
            }
            document.load(options).map(|loaded| (document, loaded))
        }) {
            Ok((document, loaded)) => {
                let config = &loaded.config;
                entry.ems = "EMS absent".into();
                entry.rate = format!(
                    "80286 · {}/{}",
                    config.clock.retired_per_pit_clock,
                    config.clock.retired_per_pit_clock_denominator
                );
                let memory: u64 = config.memory.ram.iter().map(|ram| ram.size).sum();
                let display = loaded
                    .cards
                    .iter()
                    .filter(|(_, id)| {
                        matches!(id, assembly::DeviceId::Mda | assembly::DeviceId::Vga)
                    })
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let sound = config
                    .devices
                    .iter()
                    .any(|device| matches!(device.kind, assembly::DeviceKind::Sb16(_)));
                for device in &config.devices {
                    if let assembly::DeviceKind::LotechEms(ems) = device.kind {
                        entry.ems = format!("EMS {} KiB", ems.size_kib);
                    }
                }
                for drive in &config.drives {
                    if drive.controller == assembly::DeviceId::Fdc && drive.unit < 2 {
                        entry.floppy[drive.unit as usize] = true;
                    }
                }
                entry.details = vec![
                    ("Board".into(), config.board.id().into()),
                    ("Memory".into(), format!("{} KiB RAM", memory / 1024)),
                    ("Display".into(), display),
                    (
                        "Sound".into(),
                        if sound { "SB16 / OPL3" } else { "PC speaker" }.into(),
                    ),
                    (
                        "Drives".into(),
                        config
                            .drives
                            .iter()
                            .map(|drive| {
                                format!("{:?} {}: {:?}", drive.controller, drive.unit, drive.kind)
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                ];
                if let Err(error) = loaded.check() {
                    entry.warning = Some(error.to_string());
                } else {
                    prepared = Some(document);
                }
            }
            Err(error) => entry.warning = Some(error.to_string()),
        }
        if let Some(warning) = &entry.warning {
            entry.warning = Some(relative_diagnostic(warning, &entry.path));
        }
        (entry, prepared)
    }
}

impl LibraryEntry {
    pub fn has_problem(&self) -> bool {
        self.checked && self.warning.is_some()
    }

    pub fn hint(&self) -> &str {
        if !self.checked {
            return "Checking machine configuration…";
        }
        match self.warning.as_deref() {
            Some(warning) if warning.contains("cancelled") =>
                "Use Tools › Locate firmware… › Check again for a normal cached firmware check.",
            Some(warning) if warning.contains("cannot read image") && warning.contains("drives[") =>
                "Media image is missing or unreadable. Use Tools › Create blank disk… for a new disk, or correct the image path in the machine file.",
            Some(warning) if (warning.contains("SHA-256") || warning.contains("sha256")) && warning.contains("drives[") =>
                "Media image has the wrong SHA-256. Restore the expected image or correct its pin in the machine file.",
            Some(warning) if warning.contains("firmware") || warning.contains("ROM") || warning.contains("bios") =>
                "Required firmware is missing, unreadable or incompatible. Use Tools › Locate firmware… and check the firmware declarations.",
            Some(_) if self.checked => "Correct the reported configuration field in the machine file.",
            _ => "Checking machine configuration…",
        }
    }
}

pub(crate) fn relative_diagnostic(text: &str, machine: &Path) -> String {
    let root = machine
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "machines"))
        .or_else(|| machine.parent())
        .unwrap_or(Path::new("."));
    let mut result = text.to_owned();
    for root in [
        Some(root.to_path_buf()),
        root.canonicalize().ok(),
        std::path::absolute(root).ok(),
    ]
    .into_iter()
    .flatten()
    {
        let prefix = format!("{}/", root.display());
        result = result.replace(&prefix, "");
    }
    result
}

fn source_revision(path: &Path) -> Option<String> {
    machine_file::resolve(path)
        .ok()
        .map(|(file, origins)| format!("{file:?}{origins:?}"))
}

fn failed_firmware_paths(
    document: &machine_file::PreparedMachineFile,
    field: &str,
) -> Vec<PathBuf> {
    let (document, origins) = document.resolved();
    let mut paths = Vec::new();
    if let Some(firmware) = &document.firmware {
        let roms = firmware
            .bios
            .iter()
            .map(|rom| ("firmware.bios".into(), rom))
            .chain(
                firmware
                    .option_roms
                    .iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, rom)| (format!("firmware.option_roms[{index}]"), rom)),
            );
        for (rom_field, rom) in roms {
            if !field.starts_with(&rom_field) {
                continue;
            }
            if let Some(image) = &rom.image {
                paths.push(origins.image_path(&format!("{rom_field}.image"), image));
            }
            for (index, part) in rom.parts.iter().flatten().enumerate() {
                let part_field = format!("{rom_field}.parts[{index}]");
                if !field.contains(".parts[") || field.starts_with(&part_field) {
                    paths.push(origins.image_path(&format!("{part_field}.image"), &part.image));
                }
            }
        }
    }
    paths
}

pub type PreparedEntry = (LibraryEntry, Option<machine_file::PreparedMachineFile>);

/// Only host loading/checking runs on this worker. Guest assembly, audio and
/// execution remain on the frontend's existing execution path.
pub struct LibraryCheck {
    pub path: PathBuf,
    pub options: assembly::AssemblyOptions,
    pub roots: Vec<machine_file::firmware_discovery::FirmwareRoot>,
    pub cache: Option<PathBuf>,
    pub store_root: Option<PathBuf>,
    pub rescan: bool,
    pub refresh_listing: bool,
    pub control: machine_file::firmware_discovery::DiscoveryControl,
    pub session: machine_file::firmware_discovery::DiscoverySession,
    pub prepared: Option<PreparedEntry>,
}

pub fn spawn_library_check(
    request: LibraryCheck,
    wake: impl FnOnce() + Send + 'static,
) -> std::sync::mpsc::Receiver<PreparedEntry> {
    spawn_library_check_with_compatibility(
        request,
        wake,
        pc_image::compat::compatibility(image_store::compat::EXTRACTION_BASELINE),
    )
}

pub fn spawn_library_check_with_compatibility(
    request: LibraryCheck,
    wake: impl FnOnce() + Send + 'static,
    compatibility: image_store::compat::Compatibility,
) -> std::sync::mpsc::Receiver<PreparedEntry> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let LibraryCheck {
            path,
            options,
            roots,
            cache,
            store_root,
            rescan,
            refresh_listing,
            control,
            session,
            prepared,
        } = request;
        let roots = roots
            .into_iter()
            .map(|root| {
                machine_file::firmware_discovery::FirmwareRoot::new(
                    std::path::absolute(&root.path).unwrap_or(root.path),
                    root.origin,
                )
            })
            .collect::<Vec<_>>();
        let prepared = session.run(cache.as_deref(), rescan, &control, || {
            if control.is_cancelled() {
                return (LibraryEntry::pending(path), None);
            }
            if refresh_listing {
                machine_file::firmware_discovery::refresh_firmware_roots(&roots);
            }
            if let Some((mut entry, Some(document))) = prepared {
                if entry.source.is_some() && entry.source == source_revision(&path) {
                    control.loading();
                    match document
                        .load(options)
                        .and_then(|loaded| loaded.check().map(|_| ()).map_err(Into::into))
                    {
                        Ok(()) => return (entry, Some(document)),
                        Err(error) => {
                            let failed_paths = match &error {
                                machine_file::LoadError::Lower(error)
                                    if error.field.starts_with("firmware.") =>
                                {
                                    Some(failed_firmware_paths(&document, &error.field))
                                }
                                _ => None,
                            };
                            let error = error.to_string();
                            if let Some(paths) = failed_paths {
                                machine_file::firmware_discovery::refresh_changed_firmware_roots(
                                    &roots, &paths,
                                );
                            } else {
                                entry.warning = Some(relative_diagnostic(&error, &path));
                                return (entry, None);
                            }
                        }
                    }
                }
            }
            LibraryEntry::prepare(
                path,
                options,
                &roots,
                Some(&control),
                store_root.as_deref(),
                &compatibility,
            )
        });
        let _ = tx.send(prepared);
        wake();
    });
    rx
}

pub fn machine_paths(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for dir in [root.to_path_buf(), root.join("local")] {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            let visible = path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                !name.starts_with('.') && !name.contains(".pending.")
            });
            if visible
                && path.is_file()
                && matches!(
                    path.extension().and_then(|value| value.to_str()),
                    Some("toml" | "yaml" | "yml")
                )
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

/// The library row selected at start. A machine named with `--machine`
/// (`given`) is added even when missing, so its error shows; the default
/// machine only when its file exists, so a fresh install selects nothing.
pub fn select_machine(paths: &mut Vec<PathBuf>, requested: &Path, given: bool) -> Option<usize> {
    if let Some(index) = paths.iter().position(|path| {
        path == requested
            || path
                .canonicalize()
                .ok()
                .zip(requested.canonicalize().ok())
                .is_some_and(|(left, right)| left == right)
    }) {
        return Some(index);
    }
    if !given && !requested.is_file() {
        return None;
    }
    paths.push(requested.to_path_buf());
    Some(paths.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fix_round_library_ignores_dotfiles_and_pending_files() {
        let temp = machine_file::test_support::TempDir::new("library-pending");
        std::fs::create_dir_all(temp.path().join("local")).unwrap();
        for file in [
            "visible.toml",
            ".hidden.toml",
            "stale.pending.toml",
            "local/.settings.pending.toml",
            "local/saved.toml",
        ] {
            std::fs::write(temp.path().join(file), "").unwrap();
        }
        let paths = machine_paths(temp.path()).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().all(|path| path
            .file_name()
            .is_some_and(|name| name == "visible.toml" || name == "saved.toml")));
    }

    #[test]
    fn row_warnings_exclude_pending_checks_and_skipped_roots() {
        let mut entry = LibraryEntry::pending("synthetic.toml".into());
        assert!(!entry.has_problem());
        assert!(!entry.hint().contains("Required firmware"));
        entry.checked = true;
        entry.warning = None;
        entry
            .discovery_warnings
            .push("skipped unreadable root".into());
        assert!(!entry.has_problem());
        entry.warning = Some("missing local disk".into());
        assert!(entry.has_problem());
    }

    #[test]
    fn cause_specific_hints_distinguish_media_firmware_and_integrity() {
        let mut entry = LibraryEntry::pending("machines/example.toml".into());
        entry.checked = true;
        for (warning, expected) in [
            (
                "card[ide].drives[master]: cannot read image local/games.img",
                "Create blank disk",
            ),
            (
                "card[fdc].drives[unit=0]: cannot read image local/transfer-1200.img",
                "Create blank disk",
            ),
            (
                "card[fdc].drives[unit=0]: SHA-256 mismatch",
                "Restore the expected image",
            ),
            ("firmware.bios: missing ROM", "Locate firmware"),
            ("memory: invalid size", "configuration field"),
        ] {
            entry.warning = Some(warning.into());
            assert!(entry.hint().contains(expected), "{}", entry.hint());
        }
        assert_eq!(
            relative_diagnostic(
                "machines/local/games.img",
                Path::new("machines/example.toml")
            ),
            "local/games.img"
        );
    }

    #[test]
    fn firmware_readiness_is_lazy_and_runs_on_a_host_worker() {
        let temp = machine_file::test_support::TempDir::new("worker-firmware-check");
        let path = temp.path().join("missing.toml");
        let entry = LibraryEntry::pending(path.clone());
        assert!(!entry.checked);
        assert_eq!(entry.warning.as_deref(), Some("Checking firmware…"));
        let caller = std::thread::current().id();
        let (tx, rx) = std::sync::mpsc::channel();
        let result = spawn_library_check(
            LibraryCheck {
                path,
                options: assembly::AssemblyOptions::default(),
                roots: vec![],
                cache: None,
                store_root: None,
                rescan: false,
                refresh_listing: false,
                control: Default::default(),
                session: Default::default(),
                prepared: None,
            },
            move || {
                tx.send(std::thread::current().id()).unwrap();
            },
        )
        .recv_timeout(crate::test_wait::HANG_GUARD)
        .unwrap();
        assert_ne!(
            rx.recv_timeout(crate::test_wait::HANG_GUARD).unwrap(),
            caller
        );
        assert!(result.0.checked);
        assert!(result.0.warning.is_some());
        assert!(result.1.is_none());
    }

    #[test]
    fn discovery_includes_local_and_yaml_but_not_nested_or_non_machine_files() {
        let temp = machine_file::test_support::TempDir::new("shell-discovery");
        std::fs::create_dir_all(temp.path().join("local/nested")).unwrap();
        for file in [
            "z.toml",
            "a.yaml",
            "b.yml",
            "local/c.toml",
            "README.md",
            "local/nested/hidden.toml",
        ] {
            std::fs::write(temp.path().join(file), "").unwrap();
        }
        let names: Vec<_> = machine_paths(temp.path())
            .unwrap()
            .iter()
            .map(|path| {
                path.strip_prefix(temp.path())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["a.yaml", "b.yml", "local/c.toml", "z.toml"]);
        assert!(machine_paths(&temp.path().join("absent"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn selection_keeps_a_missing_requested_machine_instead_of_starting_another() {
        let mut paths = vec![PathBuf::from("first.toml")];
        assert_eq!(
            select_machine(&mut paths, Path::new("first.toml"), true),
            Some(0)
        );
        assert_eq!(paths.len(), 1);
        let selected = select_machine(&mut paths, Path::new("missing.toml"), true).unwrap();
        assert_eq!(paths[selected], Path::new("missing.toml"));
    }

    /// A fresh install has no default machine file: the library starts with
    /// nothing selected instead of a row for a file that does not exist.
    #[test]
    fn a_missing_default_machine_is_neither_listed_nor_selected() {
        let temp = machine_file::test_support::TempDir::new("default-machine-selection");
        let mut paths = vec![PathBuf::from("first.toml")];
        let missing = temp.path().join("at-freedos.toml");
        assert_eq!(select_machine(&mut paths, &missing, false), None);
        assert_eq!(paths, [PathBuf::from("first.toml")]);
        assert_eq!(select_machine(&mut paths, Path::new(""), false), None);
        assert_eq!(paths.len(), 1);
        std::fs::write(&missing, "schema=1\n").unwrap();
        assert_eq!(select_machine(&mut paths, &missing, false), Some(1));
        assert_eq!(paths[1], missing);
        assert_eq!(select_machine(&mut paths, &missing, false), Some(1));
    }

    #[test]
    fn invalid_machine_is_visible_with_start_blocked() {
        let temp = machine_file::test_support::TempDir::new("shell-invalid");
        let path = temp.path().join("missing-firmware.toml");
        std::fs::write(&path, "schema = 1\nname = 'Missing firmware'\nextends = 'preset:ibm-5170'\n[firmware.bios]\nimage = 'absent.bin'\n").unwrap();
        let entry = LibraryEntry::inspect(path, assembly::AssemblyOptions::default());
        assert_eq!(entry.name, "Missing firmware");
        assert!(entry.warning.as_deref().unwrap().contains("absent.bin"));
        assert_eq!(entry.ems, "EMS unknown");
    }

    #[test]
    fn missing_local_media_names_the_media_cause_and_shortens_resolved_paths() {
        let temp = machine_file::test_support::TempDir::new("missing-media-hint");
        std::fs::create_dir(temp.path().join("machines")).unwrap();
        let path = temp.path().join("machines/example.toml");
        std::fs::write(temp.path().join("machines/bios.bin"), vec![0xF4; 32768]).unwrap();
        std::fs::write(&path, "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='1.2M'\nimage='local/transfer-1200.img'\nwrite='readonly'\n").unwrap();
        let entry = LibraryEntry::inspect(
            path,
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        );
        assert!(entry
            .warning
            .as_deref()
            .unwrap()
            .contains("transfer-1200.img"));
        assert!(!entry
            .warning
            .as_deref()
            .unwrap()
            .contains(&temp.path().to_string_lossy().to_string()));
        assert!(
            entry.hint().contains("Create blank disk"),
            "{}: {}",
            entry.warning.as_deref().unwrap(),
            entry.hint()
        );
    }
}
