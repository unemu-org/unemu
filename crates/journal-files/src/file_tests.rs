// SPDX-License-Identifier: AGPL-3.0-or-later
//! Injected mount boundaries for file tests. Record writes, cuts, barriers and
//! replacements always use the shared engine; no alternate record lives here.
use super::*;
use crate::temporary::{cleanup_creation_temp, cleanup_retention_temp, HostCreationTempIo};
use std::any::Any;
pub(crate) trait JournalIo: Read + Write + Seek {
    fn sync(&mut self) -> io::Result<()>;
    fn truncate(&mut self, length: u64) -> io::Result<()>;
    fn metadata(&self) -> io::Result<std::fs::Metadata>;
}

pub(crate) fn barrier(output: &mut dyn JournalIo) -> io::Result<()> {
    output.flush()?;
    output.sync()
}

/// Namespace and lock operations are separate from the journal inode so the
/// lock remains stable across later atomic retention replacements.
pub(crate) trait Filesystem {
    fn lock(&mut self, path: &Path) -> io::Result<Box<dyn Any>>;
    fn open(&mut self, path: &Path) -> io::Result<Option<Box<dyn JournalIo>>>;
    fn verify_path(&mut self, path: &Path) -> io::Result<()>;
    fn cleanup_creation_temps(
        &mut self,
        path: &Path,
        header: &[u8],
        base_metadata: Option<&std::fs::Metadata>,
    ) -> io::Result<()>;
    fn create_temp(&mut self, path: &Path) -> io::Result<(Box<dyn JournalIo>, PathBuf)>;
    fn publish(&mut self, temporary: &Path, path: &Path) -> io::Result<()>;
    fn remove_temp(&mut self, temporary: &Path) -> io::Result<()>;
    fn sync_directory(&mut self, path: &Path) -> io::Result<()>;
}

pub(crate) struct ProtectedBaseIo<'a>(&'a std::fs::Metadata);
impl crate::temporary::CreationTempIo for ProtectedBaseIo<'_> {
    fn metadata(&mut self, path: &Path) -> io::Result<std::fs::Metadata> {
        HostCreationTempIo.metadata(path)
    }
    fn open(&mut self, path: &Path) -> io::Result<File> {
        HostCreationTempIo.open(path)
    }
    fn remove(&mut self, path: &Path) -> io::Result<()> {
        HostCreationTempIo.remove(path)
    }
    fn protected_base(&self) -> Option<&std::fs::Metadata> {
        Some(self.0)
    }
}

pub(crate) struct HostFilesystem;

impl Filesystem for HostFilesystem {
    fn lock(&mut self, path: &Path) -> io::Result<Box<dyn Any>> {
        Ok(Box::new(lock_journal(path, false)?))
    }

    fn open(&mut self, path: &Path) -> io::Result<Option<Box<dyn JournalIo>>> {
        match OpenOptions::new().read(true).write(true).open(path) {
            Ok(file) => Ok(Some(Box::new(file))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn verify_path(&mut self, path: &Path) -> io::Result<()> {
        if path.canonicalize()? != path {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal canonical path changed after locking",
            ));
        }
        Ok(())
    }

    fn cleanup_creation_temps(
        &mut self,
        path: &Path,
        header: &[u8],
        base_metadata: Option<&std::fs::Metadata>,
    ) -> io::Result<()> {
        let parent = path.parent().unwrap_or(Path::new("."));
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let candidate = entry.path();
            let mut protected;
            let io: &mut dyn crate::temporary::CreationTempIo = if let Some(base) = base_metadata {
                protected = ProtectedBaseIo(base);
                &mut protected
            } else {
                &mut HostCreationTempIo
            };
            if JournalMedia::is_creation_temp(path, &candidate) {
                cleanup_creation_temp(io, &candidate, header)?;
            } else if JournalMedia::is_retention_temp(path, &candidate) {
                cleanup_retention_temp(io, &candidate)?;
            }
        }
        Ok(())
    }

    fn create_temp(&mut self, path: &Path) -> io::Result<(Box<dyn JournalIo>, PathBuf)> {
        for counter in 0..u64::MAX {
            let mut name = path.as_os_str().to_os_string();
            name.push(format!(".create-{counter}.tmp"));
            let temporary = PathBuf::from(name);
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => return Ok((Box::new(file), temporary)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other("temporary header names exhausted"))
    }

    fn publish(&mut self, temporary: &Path, path: &Path) -> io::Result<()> {
        std::fs::hard_link(temporary, path)
    }

    fn remove_temp(&mut self, temporary: &Path) -> io::Result<()> {
        std::fs::remove_file(temporary)
    }

    fn sync_directory(&mut self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        platform_sync(&File::open(parent)?)
    }
}

impl JournalIo for File {
    fn metadata(&self) -> io::Result<std::fs::Metadata> {
        File::metadata(self)
    }
    fn sync(&mut self) -> io::Result<()> {
        platform_sync(self)
    }
    fn truncate(&mut self, length: u64) -> io::Result<()> {
        self.set_len(length)
    }
}

/// Prepare an injected opening, then hand its inode and lock to the same
/// record engine used by ordinary mounts.
pub(crate) fn open(
    fs: &mut dyn Filesystem,
    path: &Path,
    header: &[u8],
    protected: Option<&Metadata>,
) -> Result<(unemu_core_files::FileRecord, bool), (&'static str, io::Error)> {
    let lock = fs.lock(path).map_err(|e| ("lock", e))?;
    let mut output = fs.open(path).map_err(|e| ("open", e))?;
    let created = output.is_none();
    if created {
        let (mut file, temp) = fs
            .create_temp(path)
            .map_err(|e| ("create temporary header", e))?;
        let result = (|| {
            file.write_all(header).map_err(|e| ("header write", e))?;
            barrier(file.as_mut()).map_err(|e| ("header sync", e))?;
            fs.publish(&temp, path).map_err(|e| ("publish header", e))?;
            fs.remove_temp(&temp)
                .map_err(|e| ("unlink creation temporary", e))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs.remove_temp(&temp);
        }
        result?;
        output = Some(file);
    }
    fs.verify_path(path)
        .map_err(|e| ("verify journal path", e))?;
    fs.cleanup_creation_temps(path, header, protected)
        .map_err(|e| ("remove creation temporaries", e))?;
    fs.sync_directory(path).map_err(|e| ("directory sync", e))?;
    let prepared = Rc::new(Prepared {
        output: RefCell::new(output),
        lock: RefCell::new(Some(lock)),
    });
    unemu_core_files::FileRecord::open_with(prepared, path, None, header, None)
        .map(|record| (record, created))
        .map_err(|e| (journal_stage(e.stage), e.source))
}
struct Prepared {
    output: RefCell<Option<Box<dyn JournalIo>>>,
    lock: RefCell<Option<Box<dyn Any>>>,
}
impl HostOps for Prepared {
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        Ok(path.to_owned())
    }
    fn open(&self, _: &Path, _: bool) -> io::Result<Box<dyn FileIo>> {
        self.output
            .borrow_mut()
            .take()
            .map(|io| Box::new(PreparedIo(io)) as Box<dyn FileIo>)
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }
    fn entries(&self, _: &Path) -> io::Result<Vec<PathBuf>> {
        Ok(vec![])
    }
    fn lock(&self, _: &Path, _: bool) -> io::Result<Box<dyn Any>> {
        self.lock
            .borrow_mut()
            .take()
            .ok_or_else(|| io::Error::other("lock already taken"))
    }
    fn sync_directory(&self, _: &Path) -> io::Result<()> {
        Ok(())
    }
}
struct PreparedIo(Box<dyn JournalIo>);
impl Read for PreparedIo {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.0.read(bytes)
    }
}
impl Seek for PreparedIo {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }
}
impl Write for PreparedIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
impl FileIo for PreparedIo {
    fn metadata(&self) -> io::Result<Metadata> {
        self.0.metadata()
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.0.truncate(len)
    }
    fn sync_all(&mut self) -> io::Result<()> {
        self.0.sync()
    }
    fn set_permissions(&mut self, _: Permissions) -> io::Result<()> {
        Ok(())
    }
}
