// SPDX-License-Identifier: AGPL-3.0-or-later
//! The journal's own file code: a version-1 journal file on the host as one
//! record of the contract (`unemu_core_api::Record`), core-contract.md §12.5
//! (move 1) and §14.4; persistent-media.md § Persist at command completion,
//! § Open and tail recovery and § Locking and retention.
//!
//! The journal's logic ([`crate::JournalMedia`]) reads, appends, cuts,
//! synchronizes and replaces a record and never names a path. This module is
//! everything the journal did with host files before it sat on records, kept
//! byte for byte and step for step:
//!
//! - **Exclusive use** is the stable sidecar lock `FILE.lock` beside the
//!   journal (`flock`-style, never waits, never removed), the same lock the
//!   offline tools take, so a running machine and `media verify`, `rollback`
//!   or `commit` still exclude each other. The contract's file-backed storage
//!   (`unemu-core-files`) names its lock `FILE_lock`; the journal keeps its own
//!   name (core-contract.md §14.6 Q8) until that crate learns it.
//! - **Creation** writes the header to `FILE.create-N.tmp`, synchronizes it and
//!   links it into place without replacing an existing file, so a journal with
//!   half a header never exists. After it, and at every open of an existing
//!   journal, the canonical path is checked again, leftovers of earlier
//!   creations and retentions are removed under the lock and the directory is
//!   synchronized.
//! - **Appends and cuts** go to the open journal file; `sync` is
//!   `File::sync_all` (`F_FULLFSYNC` on Apple targets).
//! - **A replacement** (retention) is written to `FILE.retain-N.tmp` with the
//!   journal's exact permission bits, synchronized, renamed over the journal
//!   and made durable with a directory sync.
//!
//! The record's length is asked of the host (a seek to the end), which also
//! leaves the next append at the end; reads seek to their offset. These are
//! the same host calls in the same order as before the journal sat on
//! records, which the journal's model tests pin.

use crate::offline::{HostIo, ToolIo};
use crate::temporary::{cleanup_creation_temp, cleanup_retention_temp, HostCreationTempIo};
use crate::{Header, JournalMedia};
use std::any::Any;
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use unemu_core_api as contract;
use unemu_core_api::ErrorKind;

/// A version-1 journal file on the host, held exclusively through its
/// `FILE.lock`, as a record of the contract.
///
/// This is what a front end's storage hands to the PC core for a disk or CMOS
/// journal (core-contract.md §12.9, corrected 2026-11-19: a table from record
/// name to journal file). [`crate::JournalMedia::open_record`] mounts it.
/// The path constructors of `JournalMedia` open the same file code directly.
///
/// A journal file that does not exist opens empty; the first replacement's
/// commit creates it through a temporary header file, never in place, and an
/// append before that fails with `ErrorKind::Usage`. That commit publishes
/// only the header the file was opened for. A file that exists with no bytes
/// is refused at open, as the path constructors refuse it: the publication
/// protocol never leaves one.
pub struct JournalFile(FileRecord);

impl JournalFile {
    /// Open the journal at `path` for a session: resolve it without following
    /// a dangling link or accepting a reserved temporary name, take
    /// `path.lock`, open the file and, when it exists, check its canonical
    /// path again, remove leftovers of interrupted creations and retentions,
    /// and synchronize the directory.
    ///
    /// `base` is the identity of the disk the journal belongs to (its length
    /// and SHA-256; the anchor is ignored): the cleanup removes a creation
    /// leftover only when its bytes are a prefix of that journal's header.
    /// `base_metadata`, when the base is a host file, keeps that file from
    /// being removed even when it is linked under a reserved name.
    ///
    /// Fails with `ErrorKind::Usage` when another session or an offline tool
    /// holds the lock, and with `ErrorKind::Storage` when the host failed.
    /// The message names the step, as the journal's own failures do.
    pub fn open(
        path: &Path,
        base: &Header,
        base_metadata: Option<&std::fs::Metadata>,
    ) -> Result<Self, contract::Error> {
        let fail = |step: &str, error: io::Error| {
            let kind = if error.kind() == io::ErrorKind::WouldBlock {
                ErrorKind::Usage
            } else {
                ErrorKind::Storage
            };
            contract::Error::new(kind, format!("{step}: {error}"))
        };
        let path = resolve_journal_path(path).map_err(|e| fail("resolve journal path", e))?;
        let header = Header {
            anchor_generation: 0,
            ..base.clone()
        }
        .encode()
        .map_err(|error| contract::Error::new(ErrorKind::Usage, error.to_string()))?;
        let record = FileRecord::open(
            &mut HostFilesystem,
            &path,
            None,
            header.to_vec(),
            base_metadata,
        )
        .map_err(|(step, error)| fail(step, error))?;
        // The publication protocol never leaves a journal without its
        // header; the path constructors refuse such a file, and so does this.
        if record.exists() && record.size()? == 0 {
            return Err(contract::Error::new(
                ErrorKind::Storage,
                "journal file has no header: it was not made by the journal",
            ));
        }
        Ok(Self(record))
    }
}

impl contract::Record for JournalFile {
    fn size(&self) -> Result<u64, contract::Error> {
        self.0.size()
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), contract::Error> {
        self.0.read_at(offset, buf)
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), contract::Error> {
        self.0.append(bytes)
    }
    fn truncate(&mut self, size: u64) -> Result<(), contract::Error> {
        self.0.truncate(size)
    }
    fn sync(&mut self) -> Result<(), contract::Error> {
        self.0.sync()
    }
    fn replace(&mut self) -> Result<Box<dyn contract::Replacement>, contract::Error> {
        self.0.replace()
    }
}

/// A failed host step: its name in the journal's failure reports, and the
/// host's error.
pub(crate) type Step = (&'static str, io::Error);

/// The journal file behind a record. Shared with its replacement, which puts
/// new contents in place.
pub(crate) struct FileRecord {
    inner: Rc<RefCell<FileInner>>,
}

struct FileInner {
    /// The canonical path the lock and every name beside it derive from.
    path: PathBuf,
    /// The journal file; `None` until a creation published it.
    output: Option<Box<dyn JournalIo>>,
    lock: Option<Box<dyn Any>>,
    /// The header a creation writes; creation leftovers hold a prefix of it.
    creation_header: Vec<u8>,
    base_metadata: Option<std::fs::Metadata>,
    /// The length, once the host told it or the record changed it.
    len: Option<u64>,
    /// Whether the file's cursor is known to be at its end.
    at_end: bool,
    /// A write ended with an uncertain outcome; only reads remain.
    poisoned: bool,
    /// The living replacement, by its number within this opening.
    replacement: Option<u64>,
    replacements: u64,
    released: bool,
}

fn storage(error: io::Error) -> contract::Error {
    let kind = if error.kind() == io::ErrorKind::UnexpectedEof {
        ErrorKind::Usage
    } else {
        ErrorKind::Storage
    };
    contract::Error::new(kind, error.to_string())
}

fn uncertain(error: io::Error) -> contract::Error {
    contract::Error::new(ErrorKind::StorageUncertain, error.to_string())
}

fn usage(message: &str) -> contract::Error {
    contract::Error::new(ErrorKind::Usage, message)
}

impl FileRecord {
    /// Take the lock (unless the caller already holds it), open the journal
    /// and, when it exists, settle it: path check, leftover cleanup and
    /// directory sync, in that order.
    pub(crate) fn open(
        fs: &mut dyn Filesystem,
        path: &Path,
        lock: Option<Box<dyn Any>>,
        creation_header: Vec<u8>,
        base_metadata: Option<&std::fs::Metadata>,
    ) -> Result<Self, Step> {
        let lock = match lock {
            Some(lock) => lock,
            None => fs.lock(path).map_err(|e| ("lock", e))?,
        };
        let output = fs.open(path).map_err(|e| ("open", e))?;
        let exists = output.is_some();
        let record = Self {
            inner: Rc::new(RefCell::new(FileInner {
                path: path.to_owned(),
                output,
                lock: Some(lock),
                creation_header,
                base_metadata: base_metadata.cloned(),
                len: None,
                at_end: false,
                poisoned: false,
                replacement: None,
                replacements: 0,
                released: false,
            })),
        };
        if exists {
            record.inner.borrow().settle(fs)?;
        }
        Ok(record)
    }

    /// Whether the journal file exists.
    pub(crate) fn exists(&self) -> bool {
        self.inner.borrow().output.is_some()
    }

    /// Create the journal with its creation header, as a mount of a missing
    /// journal does.
    pub(crate) fn create(&mut self, fs: &mut dyn Filesystem) -> Result<(), Step> {
        let mut inner = self.inner.borrow_mut();
        let header = inner.creation_header.clone();
        inner.create(fs, &header).map_err(|(step, _)| step)
    }

    fn replace_with_tool<'t>(
        &mut self,
        tool: Tool<'t>,
    ) -> Result<Box<dyn contract::Replacement + 't>, contract::Error> {
        let mut inner = self.inner.borrow_mut();
        if inner.poisoned {
            return Err(usage(
                "the record must be opened again before it is written",
            ));
        }
        if inner.replacement.is_some() {
            return Err(usage("the record already has a replacement"));
        }
        let kind = if inner.output.is_none() {
            Kind::Creation { bytes: Vec::new() }
        } else {
            let mut tool = tool;
            let permissions = std::fs::metadata(&inner.path)
                .map_err(storage)?
                .permissions();
            let (file, temporary) = tool
                .get()
                .create_temporary(&inner.path, "retain", Some(&permissions))
                .map_err(storage)?;
            // The requested creation mode is still filtered by the process umask.
            if let Err(error) = tool.get().set_permissions(&file, permissions) {
                let _ = std::fs::remove_file(&temporary);
                return Err(storage(error));
            }
            Kind::Retention {
                file: Some(file),
                temporary: Some(temporary),
                len: 0,
                tool,
                broken: false,
            }
        };
        inner.replacements += 1;
        let number = inner.replacements;
        inner.replacement = Some(number);
        Ok(Box::new(FileReplacement {
            inner: Rc::clone(&self.inner),
            number,
            kind,
        }))
    }
}

impl FileInner {
    fn output(&mut self) -> Result<&mut Box<dyn JournalIo>, contract::Error> {
        self.output
            .as_mut()
            .ok_or_else(|| usage("a journal file is created by a replacement"))
    }

    fn check_writable(&self) -> Result<(), contract::Error> {
        if self.poisoned {
            return Err(usage(
                "the record must be opened again before it is written",
            ));
        }
        Ok(())
    }

    /// Check the canonical path, remove leftovers of interrupted creations
    /// and retentions under the lock, and synchronize the directory, which
    /// covers both a prior publication and the removal of its leftover link.
    fn settle(&self, fs: &mut dyn Filesystem) -> Result<(), Step> {
        fs.verify_path(&self.path)
            .map_err(|e| ("verify journal path", e))?;
        fs.cleanup_creation_temps(
            &self.path,
            &self.creation_header,
            self.base_metadata.as_ref(),
        )
        .map_err(|e| ("remove creation temporaries", e))?;
        fs.sync_directory(&self.path)
            .map_err(|e| ("directory sync", e))
    }

    /// Publish `bytes` as the new journal through a synchronized temporary
    /// header file, then settle. The flag tells whether the journal was in
    /// place when a step failed.
    fn create(&mut self, fs: &mut dyn Filesystem, bytes: &[u8]) -> Result<(), (Step, bool)> {
        let path = self.path.clone();
        let (mut output, temporary) = fs
            .create_temp(&path)
            .map_err(|e| (("create temporary header", e), false))?;
        let mut published = false;
        let initialization = (|| {
            output.write_all(bytes).map_err(|e| ("header write", e))?;
            barrier(output.as_mut()).map_err(|e| ("header sync", e))?;
            fs.publish(&temporary, &path)
                .map_err(|e| ("publish header", e))?;
            published = true;
            fs.remove_temp(&temporary)
                .map_err(|e| ("unlink temporary header", e))
        })();
        if let Err(step) = initialization {
            // Unpublished leftovers are never mounted. Best-effort cleanup
            // must not replace the original failure.
            let _ = fs.remove_temp(&temporary);
            return Err((step, published));
        }
        self.output = Some(output);
        self.len = Some(bytes.len() as u64);
        self.at_end = true;
        self.settle(fs).map_err(|step| (step, true))
    }
}

impl FileRecord {
    pub(crate) fn size(&self) -> Result<u64, contract::Error> {
        let mut inner = self.inner.borrow_mut();
        let Some(output) = inner.output.as_mut() else {
            return Ok(0);
        };
        let len = output.seek(SeekFrom::End(0)).map_err(storage)?;
        inner.len = Some(len);
        inner.at_end = true;
        Ok(len)
    }

    pub(crate) fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), contract::Error> {
        let mut inner = self.inner.borrow_mut();
        if inner.output.is_none() {
            // A journal that does not exist yet reads as empty.
            return if buf.is_empty() && offset == 0 {
                Ok(())
            } else {
                Err(usage("read past the end"))
            };
        }
        inner.at_end = false;
        let output = inner.output()?;
        output.seek(SeekFrom::Start(offset)).map_err(storage)?;
        output.read_exact(buf).map_err(storage)
    }

    pub(crate) fn append(&mut self, bytes: &[u8]) -> Result<(), contract::Error> {
        let mut inner = self.inner.borrow_mut();
        inner.check_writable()?;
        let at_end = inner.at_end;
        let output = inner.output()?;
        if !at_end {
            output.seek(SeekFrom::End(0)).map_err(storage)?;
        }
        // write_all is the standard library's partial and interrupted-write
        // loop; a failure may leave part of the bytes, which only recovery
        // can classify, so the outcome is uncertain.
        if let Err(error) = output.write_all(bytes) {
            inner.poisoned = true;
            inner.at_end = false;
            return Err(uncertain(error));
        }
        inner.at_end = true;
        if let Some(len) = inner.len.as_mut() {
            *len += bytes.len() as u64;
        }
        Ok(())
    }

    pub(crate) fn truncate(&mut self, size: u64) -> Result<(), contract::Error> {
        let mut inner = self.inner.borrow_mut();
        inner.check_writable()?;
        if inner.output.is_none() && size == 0 {
            return Ok(());
        }
        if inner.len.is_some_and(|len| size > len) {
            return Err(usage("a cut never lengthens a record"));
        }
        inner.at_end = false;
        if let Err(error) = inner.output()?.truncate(size) {
            inner.poisoned = true;
            return Err(uncertain(error));
        }
        inner.len = Some(size);
        Ok(())
    }

    pub(crate) fn sync(&mut self) -> Result<(), contract::Error> {
        let mut inner = self.inner.borrow_mut();
        inner.check_writable()?;
        let Some(output) = inner.output.as_mut() else {
            return Ok(());
        };
        if let Err(error) = barrier(output.as_mut()) {
            inner.poisoned = true;
            return Err(uncertain(error));
        }
        Ok(())
    }

    pub(crate) fn replace(&mut self) -> Result<Box<dyn contract::Replacement>, contract::Error> {
        self.replace_with_tool(Tool::Host(HostIo))
    }
}

impl contract::Record for FileRecord {
    fn size(&self) -> Result<u64, contract::Error> {
        FileRecord::size(self)
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), contract::Error> {
        FileRecord::read_at(self, offset, buf)
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), contract::Error> {
        FileRecord::append(self, bytes)
    }
    fn truncate(&mut self, size: u64) -> Result<(), contract::Error> {
        FileRecord::truncate(self, size)
    }
    fn sync(&mut self) -> Result<(), contract::Error> {
        FileRecord::sync(self)
    }
    fn replace(&mut self) -> Result<Box<dyn contract::Replacement>, contract::Error> {
        FileRecord::replace(self)
    }
}

impl crate::store::JournalRecord for FileRecord {
    fn replace_with<'t>(
        &mut self,
        tool: &'t mut dyn ToolIo,
    ) -> Result<Box<dyn contract::Replacement + 't>, contract::Error> {
        self.replace_with_tool(Tool::Borrowed(tool))
    }
}

impl Drop for FileRecord {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.released = true;
            inner.replacement = None;
            inner.output = None;
            inner.lock = None;
        }
    }
}

/// The host operations of a retention: the production ones, or a test's.
enum Tool<'t> {
    Host(HostIo),
    Borrowed(&'t mut dyn ToolIo),
}

impl Tool<'_> {
    fn get(&mut self) -> &mut dyn ToolIo {
        match self {
            Tool::Host(io) => io,
            Tool::Borrowed(io) => &mut **io,
        }
    }
}

enum Kind<'t> {
    /// The first contents of a journal that does not exist yet, published by
    /// the creation protocol at commit.
    Creation { bytes: Vec<u8> },
    /// New contents of an existing journal in `FILE.retain-N.tmp`.
    Retention {
        file: Option<File>,
        temporary: Option<PathBuf>,
        len: u64,
        tool: Tool<'t>,
        /// A failed append could not be cut back: the contents are unknown
        /// and the replacement can only be dropped.
        broken: bool,
    },
}

struct FileReplacement<'t> {
    inner: Rc<RefCell<FileInner>>,
    number: u64,
    kind: Kind<'t>,
}

impl contract::Replacement for FileReplacement<'_> {
    fn size(&self) -> Result<u64, contract::Error> {
        Ok(match &self.kind {
            Kind::Creation { bytes } => bytes.len() as u64,
            Kind::Retention { len, .. } => *len,
        })
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), contract::Error> {
        match &mut self.kind {
            Kind::Creation { bytes } => {
                let start = usize::try_from(offset).map_err(|_| usage("read past the end"))?;
                let source = start
                    .checked_add(buf.len())
                    .and_then(|end| bytes.get(start..end))
                    .ok_or_else(|| usage("read past the end"))?;
                buf.copy_from_slice(source);
                Ok(())
            }
            Kind::Retention { file, .. } => {
                let file = file
                    .as_mut()
                    .ok_or_else(|| usage("the replacement is closed"))?;
                file.seek(SeekFrom::Start(offset)).map_err(storage)?;
                file.read_exact(buf).map_err(storage)
            }
        }
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), contract::Error> {
        match &mut self.kind {
            Kind::Creation { bytes: contents } => {
                contents.extend_from_slice(bytes);
                Ok(())
            }
            Kind::Retention {
                file,
                len,
                tool,
                broken,
                ..
            } => {
                if *broken {
                    return Err(usage("the replacement failed and can only be dropped"));
                }
                let file = file
                    .as_mut()
                    .ok_or_else(|| usage("the replacement is closed"))?;
                match tool.get().write_temporary(file, bytes) {
                    Ok(()) => {
                        *len += bytes.len() as u64;
                        Ok(())
                    }
                    Err(error) => {
                        // Cut a partial write back. A replacement's bytes are
                        // its own temporary file, never the record's, so a
                        // failure here leaves the record as it was: it is
                        // `Storage` and does not poison the record. When the
                        // cut fails too, the replacement refuses everything
                        // but being dropped, which removes its file.
                        if file
                            .set_len(*len)
                            .and_then(|()| file.seek(SeekFrom::Start(*len)).map(|_| ()))
                            .is_err()
                        {
                            *broken = true;
                        }
                        Err(storage(error))
                    }
                }
            }
        }
    }

    fn commit(mut self: Box<Self>) -> Result<(), contract::Error> {
        let inner = Rc::clone(&self.inner);
        let mut inner = inner.borrow_mut();
        if inner.released || inner.replacement != Some(self.number) {
            return Err(usage("the record was released"));
        }
        inner.check_writable()?;
        inner.replacement = None;
        match &mut self.kind {
            Kind::Creation { bytes } => {
                let bytes = std::mem::take(bytes);
                // The leftover cleanup recognises creations by the header the
                // file was opened with; a journal of another base must not be
                // published under it.
                if bytes.get(..inner.creation_header.len()) != Some(&inner.creation_header[..]) {
                    return Err(usage(
                        "the new journal's header is not the one its file was opened for",
                    ));
                }
                inner
                    .create(&mut HostFilesystem, &bytes)
                    .map_err(|((step, error), in_place)| {
                        // Through a record a creation failure has one name in
                        // the journal's report, "publish header"; the file's
                        // own step starts the message.
                        let message = format!("{step}: {error}");
                        if in_place {
                            inner.poisoned = true;
                            contract::Error::new(ErrorKind::StorageUncertain, message)
                        } else {
                            contract::Error::new(ErrorKind::Storage, message)
                        }
                    })
            }
            Kind::Retention {
                file,
                temporary,
                len,
                tool,
                broken,
            } => {
                if *broken {
                    return Err(usage("the replacement failed and can only be dropped"));
                }
                let (Some(mut file), Some(name)) = (file.take(), temporary.take()) else {
                    return Err(usage("the replacement is closed"));
                };
                let path = inner.path.clone();
                let result = (|| {
                    file.seek(SeekFrom::End(0)).map_err(storage)?;
                    tool.get().sync(&file, "temporary sync").map_err(storage)?;
                    // From here on a failure cannot say which file the name
                    // holds.
                    tool.get()
                        .publish_temporary(&name, &path, true)
                        .map_err(uncertain)?;
                    inner.output = Some(Box::new(file));
                    inner.len = Some(*len);
                    inner.at_end = false;
                    let directory = File::open(crate::offline::parent(&path)).map_err(uncertain)?;
                    tool.get()
                        .sync(&directory, "publication directory sync")
                        .map_err(uncertain)
                })();
                // Only this replacement's own create_new file is removed; after
                // the rename the name no longer exists.
                let _ = std::fs::remove_file(&name);
                if result
                    .as_ref()
                    .is_err_and(|e| e.kind == ErrorKind::StorageUncertain)
                {
                    inner.poisoned = true;
                }
                result
            }
        }
    }
}

impl Drop for FileReplacement<'_> {
    fn drop(&mut self) {
        if let Kind::Retention {
            file, temporary, ..
        } = &mut self.kind
        {
            *file = None;
            if let Some(name) = temporary.take() {
                let _ = std::fs::remove_file(name);
            }
        }
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            if inner.replacement == Some(self.number) {
                inner.replacement = None;
            }
        }
    }
}

/// Injectable host boundary; write_all remains the standard library's partial /
/// interrupted-write loop, so tests exercise exactly the production append path.
pub(crate) trait JournalIo: Read + Write + Seek {
    fn sync(&mut self) -> io::Result<()>;
    fn truncate(&mut self, length: u64) -> io::Result<()>;
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
    fn sync(&mut self) -> io::Result<()> {
        platform_sync(self)
    }
    fn truncate(&mut self, length: u64) -> io::Result<()> {
        self.set_len(length)
    }
}

/// Refuse a reserved temporary name, then resolve the journal's canonical
/// path (a missing journal's resolved parent forms it).
pub(crate) fn resolve_journal_path(path: &Path) -> io::Result<PathBuf> {
    refuse_creation_temp_name(path)?;
    let canonical = canonical_journal_path(path)?;
    refuse_creation_temp_name(&canonical)?;
    Ok(canonical)
}

pub(crate) fn refuse_creation_temp_name(path: &Path) -> io::Result<()> {
    if JournalMedia::is_creation_temp_name(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "journal path {} uses a reserved creation-temporary name or replacement-temporary name",
                path.display()
            ),
        ));
    }
    Ok(())
}

pub(crate) fn canonical_journal_path(path: &Path) -> io::Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "journal path is a dangling symlink",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            // This is the expected canonical name for post-open verification,
            // not an additional locking mechanism for parent-directory aliases.
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = path.file_name().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "journal path must have a filename",
                )
            })?;
            Ok(parent.canonicalize()?.join(name))
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
pub(crate) fn platform_sync(file: &File) -> io::Result<()> {
    // std uses F_FULLFSYNC on Apple targets with an Interrupted retry. Calling
    // fcntl again would duplicate the device-cache flush for every command.
    file.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn platform_sync(_file: &File) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "journal durability is supported only on Unix",
    ))
}

pub(crate) fn journal_lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

pub(crate) fn inspection_lock(path: &Path) -> io::Result<Option<File>> {
    match lock_journal(path, true) {
        Ok(file) => Ok(Some(file)),
        Err(error) => {
            #[cfg(unix)]
            if matches!(
                error.raw_os_error(),
                Some(libc::EACCES) | Some(libc::EPERM) | Some(libc::EROFS)
            ) && matches!(std::fs::symlink_metadata(journal_lock_path(path)), Err(e) if e.kind() == io::ErrorKind::NotFound)
            {
                // A writing session must have created this entry. Never bypass
                // an existing lock, even if it is unreadable or a dangling link.
                return Ok(None);
            }
            Err(error)
        }
    }
}

pub(crate) fn lock_journal(path: &Path, shared: bool) -> io::Result<File> {
    let name = journal_lock_path(path);
    let file = if shared {
        match File::open(&name) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&name)
                {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        File::open(&name)?
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        }
    } else {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(name)?
    };
    let result = if shared {
        file.try_lock_shared()
    } else {
        file.try_lock()
    };
    result.map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "journal is locked by another session or tool",
        ),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(file)
}
