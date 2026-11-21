// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host path aliases and reserved journal namespaces; persistent-media.md § Integration.
use assembly::{ConfigError, ImageSource, MachineConfig, WriteMode};
use journal_files::JournalMedia;
use std::path::{Path, PathBuf};
pub fn validate_paths(config: &MachineConfig) -> Result<(), ConfigError> {
    validate_media_paths(config).map_err(|error| match error {
        ConfigError::Journal {
            drive_index,
            message,
        } if drive_index >= config.drives.len() => ConfigError::Nvram { message },
        other => other,
    })
}

fn validate_media_paths(config: &MachineConfig) -> Result<(), ConfigError> {
    let nvram: Vec<_> = config
        .devices
        .iter()
        .enumerate()
        .filter_map(|(index, d)| match &d.kind {
            assembly::DeviceKind::Rtc(assembly::CmosCfg {
                nvram:
                    assembly::NvramSource::Journal {
                        image,
                        journal,
                        keep_generations,
                        ..
                    },
                ..
            }) => Some((
                config.drives.len() + index,
                image.as_path(),
                journal.as_path(),
                *keep_generations,
            )),
            _ => None,
        })
        .collect();
    for (_, image, _, keep) in &nvram {
        if *keep == 0 {
            return Err(ConfigError::Nvram {
                message: "keep_generations must be positive".into(),
            });
        }
        if JournalMedia::is_retention_temp_name(image)
            || normalized(image)
                .ok()
                .is_some_and(|p| JournalMedia::is_retention_temp_name(&p))
        {
            return Err(ConfigError::Nvram {
                message: "image uses a reserved retention-temporary name".into(),
            });
        }
    }
    let mut inputs: Vec<_> = config
        .drives
        .iter()
        .map(|d| &d.image)
        .chain(config.images.iter().map(|i| &i.source))
        .flat_map(|source| match source {
            ImageSource::File(path) => vec![path.as_path()],
            ImageSource::Interleaved(parts) => parts.iter().map(|p| p.path.as_path()).collect(),
            _ => Vec::new(),
        })
        .collect();
    inputs.extend(nvram.iter().map(|(_, image, _, _)| *image));
    let reserved = |path: &Path| {
        JournalMedia::is_retention_temp_name(path)
            || normalized(path)
                .ok()
                .is_some_and(|p| JournalMedia::is_retention_temp_name(&p))
    };
    for (drive_index, drive) in config.drives.iter().enumerate() {
        if let ImageSource::File(path) = &drive.image {
            if reserved(path) {
                return Err(ConfigError::DriveInput {
                    drive_index,
                    path: path.clone(),
                    message: "uses a reserved retention-temporary name".into(),
                });
            }
        }
    }
    for (image_index, image) in config.images.iter().enumerate() {
        let paths = match &image.source {
            ImageSource::File(path) => vec![path],
            ImageSource::Interleaved(parts) => parts.iter().map(|p| &p.path).collect(),
            _ => Vec::new(),
        };
        for path in paths {
            if reserved(path) {
                return Err(ConfigError::ImageInput {
                    image_index,
                    role: if image.kind == assembly::ImageKind::Rom {
                        "firmware"
                    } else {
                        "RAM-load"
                    },
                    path: path.clone(),
                    message: "uses a reserved retention-temporary name".into(),
                });
            }
        }
    }
    let normalize = |drive_index, path: &Path, role| {
        normalized(path).map_err(|error| {
            path_error(
                config,
                drive_index,
                format!("{role} path {}: {error}", path.display()),
            )
        })
    };
    let mut journals = Vec::new();
    for (drive_index, path) in config
        .drives
        .iter()
        .enumerate()
        .filter_map(|(i, drive)| {
            if let WriteMode::Journal { path, .. } = &drive.write {
                Some((i, path.as_path()))
            } else {
                None
            }
        })
        .chain(nvram.iter().map(|(i, _, path, _)| (*i, *path)))
    {
        journal_files::check_journal_path(path)
            .map_err(|error| path_error(config, drive_index, error.message))?;
        if JournalMedia::is_creation_temp_name(path) {
            return Err(ConfigError::Journal {
                drive_index,
                message: format!(
                    "journal path {} uses a reserved creation-temporary name",
                    path.display()
                ),
            });
        }
        let journal = normalize(drive_index, path, "journal")?;
        if JournalMedia::is_creation_temp_name(&journal) {
            return Err(ConfigError::Journal {
                drive_index,
                message: format!(
                    "journal path {} resolves to reserved creation-temporary path {}",
                    path.display(),
                    journal.display()
                ),
            });
        }
        let mut lock = journal.as_os_str().to_os_string();
        lock.push(".lock");
        let lock = normalize(drive_index, Path::new(&lock), "journal lock")?;
        journals.push((drive_index, path, journal, lock));
    }
    let mut outputs: Vec<PathBuf> = Vec::new();
    for (drive_index, path, journal, lock) in &journals {
        let error = |message| ConfigError::Journal {
            drive_index: *drive_index,
            message,
        };
        for input in &inputs {
            // Only successfully resolved inputs can alias an output. Their own
            // image read must report every path error without journal attribution.
            let unresolved = canonical_parent_name(input).ok();
            let resolved = normalized(input).ok();
            if [unresolved.as_ref(), resolved.as_ref()]
                .into_iter()
                .flatten()
                .any(|candidate| {
                    JournalMedia::is_creation_temp(journal, candidate)
                        || candidate.file_name().is_some_and(|n| {
                            n.as_encoded_bytes().windows(8).any(|s| s == b".retain-")
                        })
                })
            {
                return Err(error(format!(
                    "journal {} reserves creation-temporary input path {}",
                    path.display(),
                    input.display()
                )));
            }
            if let Some(input_path) = resolved {
                for output in [journal, lock] {
                    if aliases(output, &input_path) {
                        return Err(error(format!(
                            "output {} aliases mounted input {}",
                            output.display(),
                            input.display()
                        )));
                    }
                }
            }
        }
        for output in [journal, lock] {
            for other in &outputs {
                if aliases(output, other) {
                    return Err(error(format!(
                        "output {} aliases another output {}",
                        output.display(),
                        other.display()
                    )));
                }
            }
            outputs.push(output.clone());
        }
    }
    Ok(())
}

fn canonical_parent_name(path: &Path) -> std::io::Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "input path must have a filename",
        )
    })?;
    Ok(std::fs::canonicalize(parent)?.join(name))
}

fn normalized(path: &Path) -> std::io::Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Refuse dangling links instead of treating them as unused names.
            if std::fs::symlink_metadata(path).is_ok() {
                return Err(error);
            }
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            Ok(std::fs::canonicalize(parent)?.join(path.file_name().ok_or(error)?))
        }
        Err(error) => Err(error),
    }
}

fn aliases(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    #[cfg(unix)]
    if let (Ok(a), Ok(b)) = (std::fs::metadata(left), std::fs::metadata(right)) {
        use std::os::unix::fs::MetadataExt;
        return a.dev() == b.dev() && a.ino() == b.ino();
    }
    false
}

fn path_error(config: &MachineConfig, index: usize, message: String) -> ConfigError {
    if index >= config.drives.len() {
        ConfigError::Nvram { message }
    } else {
        ConfigError::Journal {
            drive_index: index,
            message,
        }
    }
}
