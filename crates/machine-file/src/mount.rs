// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host file mounts for PC assembly; core-contract.md §12.5 and persistent-media.md § Integration.
use assembly::{ConfigError, DiskBase, ImageBytes, JournalStore, MachineConfig, Mount, MountError};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
};
/// The mount over host files: each path is the file it names. This is what
/// every `machine_file::assemble*` function uses, and what
/// a medium appended by path uses.
///
/// Bases the caller verified already are taken from its table by path and
/// never hashed again; every other base is hashed in full when it is mounted.
#[derive(Clone, Debug, Default)]
pub struct FileMount {
    verified: BTreeMap<PathBuf, journal_files::VerifiedBase>,
}

impl FileMount {
    /// Open every path on the host.
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the bases in `verified` by their exact path instead of opening
    /// and hashing those files; their metadata is checked again.
    pub fn with_verified_bases(verified: BTreeMap<PathBuf, journal_files::VerifiedBase>) -> Self {
        Self { verified }
    }
}

impl Mount for FileMount {
    fn is_file_mount(&self) -> bool {
        true
    }
    fn image(&self, path: &Path) -> Result<ImageBytes, MountError> {
        let mut file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(ImageBytes::with_origin(bytes, Rc::new(metadata)))
    }

    fn disk_base(
        &self,
        path: &Path,
        check_length: &dyn Fn(u64) -> Result<(), ConfigError>,
    ) -> Result<DiskBase, MountError> {
        use journal_files::SectorSource;
        if let Some(base) = self.verified.get(path) {
            base.check_unchanged()?;
            return Ok(DiskBase::from((**base).clone()).with_origin(
                base.metadata()
                    .cloned()
                    .map(|m| Rc::new(m) as Rc<dyn std::any::Any>),
            ));
        }
        let file = std::fs::File::open(path)?;
        check_length(file.metadata()?.len())?;
        let base = journal_files::VerifiedBase::from_file(file)?;
        let origin = base
            .metadata()
            .cloned()
            .map(|m| Rc::new(m) as Rc<dyn std::any::Any>);
        Ok(DiskBase::from((*base).clone()).with_origin(origin))
    }

    fn journal(&self, path: &Path) -> Result<JournalStore, MountError> {
        let path = path.to_path_buf();
        Ok(JournalStore::new(move |base, length, status| {
            let verified = journal_files::VerifiedBase::from_verified_source(
                Rc::new(base.verified().clone()),
                base.verified().identity().clone(),
            )
            .map_err(|error| {
                journal::OpenError::Host(journal::SinkFailure {
                    operation: "verify base",
                    message: error.to_string(),
                    last_confirmed_sequence: 0,
                })
            })?;
            journal_files::JournalMedia::open_verified_with_metadata(
                &path,
                &verified,
                length,
                status,
                base.origin().and_then(|origin| origin.downcast_ref()),
            )
            .map(|journal| Box::new(journal) as Box<dyn journal::JournalSession>)
        }))
    }

    fn check_outputs(&self, media: &MachineConfig) -> Result<(), ConfigError> {
        crate::mount_paths::validate_paths(media)
    }
}
