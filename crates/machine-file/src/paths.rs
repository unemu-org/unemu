// SPDX-License-Identifier: AGPL-3.0-or-later
//! Journal output checks for paths consumed by machine-file loading.
//!
//! Assembly sees file-backed drives; this boundary also retains inline image
//! sources and every inheritance layer. See machine-files.md § Drives and extends.

use std::path::{Path, PathBuf};

use assembly::{ConfigError, MachineConfig, WriteMode};
use media::JournalMedia;

use crate::Origins;

pub(crate) fn validate_journals(
    config: &MachineConfig,
    origins: &Origins,
    images: &[PathBuf],
) -> Result<(), ConfigError> {
    let drives = config.drives.iter().enumerate().flat_map(|(i, drive)| {
        let writes: Vec<_> = drive.media.as_ref().map_or_else(
            || vec![&drive.write],
            |list| list.entries.iter().map(|m| &m.write).collect(),
        );
        writes.into_iter().filter_map(move |write| {
            if let WriteMode::Journal { path, .. } = write {
                Some((Some(i), path))
            } else {
                None
            }
        })
    });
    let nvram = config.devices.iter().filter_map(|device| {
        if let assembly::DeviceKind::Rtc(assembly::CmosCfg {
            nvram: assembly::NvramSource::Journal { journal, .. },
            ..
        }) = &device.kind
        {
            Some((None, journal))
        } else {
            None
        }
    });
    for (drive_index, path) in drives.chain(nvram) {
        let error = |message| match drive_index {
            Some(drive_index) => ConfigError::Journal {
                drive_index,
                message,
            },
            None => ConfigError::Nvram { message },
        };
        if std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
            return Err(error(format!(
                "journal path {} is a directory",
                path.display()
            )));
        }
        let journal = std::fs::canonicalize(path)
            .ok()
            .or_else(|| canonical_parent_name(path));
        for input in images
            .iter()
            .map(PathBuf::as_path)
            .chain(origins.layer_files())
        {
            if is_retention_input(input)
                || journal.as_ref().is_some_and(|journal| {
                    canonical_parent_name(input)
                        .into_iter()
                        .chain(std::fs::canonicalize(input).ok())
                        .any(|candidate| JournalMedia::is_creation_temp(journal, &candidate))
                })
            {
                return Err(error(format!(
                    "journal {} reserves temporary input path {}",
                    path.display(),
                    input.display()
                )));
            }
            if aliases(path, input) {
                return Err(error(format!(
                    "journal {} aliases source input {}",
                    path.display(),
                    input.display()
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn is_retention_input(path: &Path) -> bool {
    JournalMedia::is_retention_temp_name(path)
        || std::fs::canonicalize(path)
            .ok()
            .is_some_and(|path| JournalMedia::is_retention_temp_name(&path))
}

fn canonical_parent_name(path: &Path) -> Option<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Some(std::fs::canonicalize(parent).ok()?.join(path.file_name()?))
}

fn aliases(left: &Path, right: &Path) -> bool {
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        if a == b {
            return true;
        }
    }
    #[cfg(unix)]
    if let (Ok(a), Ok(b)) = (std::fs::metadata(left), std::fs::metadata(right)) {
        use std::os::unix::fs::MetadataExt;
        return a.dev() == b.dev() && a.ino() == b.ino();
    }
    false
}
