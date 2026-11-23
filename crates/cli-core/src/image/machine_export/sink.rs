// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sink-owned temporary files and receipts; final names are never removed.
// The output directory belongs to the command's user. Export protects against accidents
// and concurrent unemu processes: no overwrites, removal of unowned files, store writes,
// or leftover temps. Hostile same-user entry swaps are outside the model; identity checks
// cheaply narrow the windows before publication and cleanup.

use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};
use unemu_core_api::{
    is_export_name, Error, ErrorKind, ExportOutput, ExportReport, ExportSink, ExportedOutput,
};

fn storage(error: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::Storage, error.to_string())
}

/// Exclusive temporary entries for one export attempt, including partial files.
pub struct FileSink<'a> {
    directory: Directory,
    next_temp: u64,
    outputs: BTreeMap<String, Output<'a>>,
    failed: bool,
    #[cfg(test)]
    fault: Option<unemu_core_api::test_support::ExportOperation>,
    hook: Option<&'a mut dyn FnMut(u64) -> std::io::Result<()>>,
}

impl<'a> FileSink<'a> {
    pub fn new(directory: &Path) -> std::io::Result<Self> {
        Ok(Self {
            directory: Directory::open(directory)?,
            next_temp: 0,
            outputs: BTreeMap::new(),
            failed: false,
            #[cfg(test)]
            fault: None,
            hook: None,
        })
    }
    #[cfg(test)]
    pub(super) fn fail_next(&mut self, operation: unemu_core_api::test_support::ExportOperation) {
        self.fault = Some(operation);
    }
    #[doc(hidden)]
    pub fn write_hook(&mut self, hook: &'a mut dyn FnMut(u64) -> std::io::Result<()>) {
        self.hook = Some(hook);
    }
    pub fn close(&mut self) {
        for output in self.outputs.values_mut() {
            output.closed = true;
        }
    }
    pub(super) fn refuse_store(&self, store: &Path) -> anyhow::Result<()> {
        if self.directory.is_within(store)? {
            return Err(
                super::usage("export destination must be outside the read-only store").into(),
            );
        }
        Ok(())
    }
    pub fn validate(&self, report: &ExportReport) -> Result<(), Error> {
        let mut names = BTreeSet::new();
        if self.failed || report.outputs.len() != self.outputs.len() {
            return Err(storage(
                "export report output count mismatch or failed reservation",
            ));
        }
        for receipt in &report.outputs {
            if !names.insert(&receipt.name)
                || self
                    .outputs
                    .get(&receipt.name)
                    .and_then(|o| o.receipt.as_ref())
                    != Some(receipt)
            {
                return Err(storage(
                    "export report has duplicate, unfinished or altered output",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn publish(&mut self, report: &ExportReport) -> Result<(), Error> {
        let mut published = Vec::new();
        let result = (|| {
            for receipt in &report.outputs {
                let output = self
                    .outputs
                    .get_mut(&receipt.name)
                    .ok_or_else(|| storage("unrecorded output"))?;
                let temp = output
                    .temp
                    .as_ref()
                    .ok_or_else(|| storage("output already published"))?;
                self.directory
                    .link(temp, &output.file, &output.name)
                    .map_err(|error| {
                        if error.kind() == std::io::ErrorKind::AlreadyExists {
                            Error::new(
                                ErrorKind::Usage,
                                format!(
                                    "cannot publish {}: output already exists: {error}",
                                    output.name
                                ),
                            )
                        } else {
                            storage(format!("cannot publish {}: {error}", output.name))
                        }
                    })?;
                published.push(output.name.clone());
                self.directory.unlink(temp, &output.file).map_err(storage)?;
                output.temp.take();
            }
            self.sync()
        })();
        result.map_err(|error: Error| {
            Error::new(
                error.kind,
                format!(
                    "{error}; published: {}",
                    if published.is_empty() {
                        "none".into()
                    } else {
                        published.join(", ")
                    }
                ),
            )
        })
    }
    fn sync(&self) -> Result<(), Error> {
        self.directory.sync().map_err(storage)
    }
    pub fn cleanup(&mut self) -> Result<(), Error> {
        self.close();
        let mut failure = None;
        for output in self.outputs.values_mut() {
            if let Some(temp) = &output.temp {
                match self.directory.unlink(temp, &output.file) {
                    Ok(()) => {
                        output.temp.take();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        output.temp.take();
                    }
                    Err(error) => failure = Some(storage(error)),
                }
            }
        }
        let sync = self.sync();
        failure.map_or(sync, Err)
    }
}
impl ExportSink for FileSink<'_> {
    fn create(&mut self, name: &str) -> Result<&mut (dyn ExportOutput + '_), Error> {
        if !is_export_name(name) || self.outputs.contains_key(name) {
            self.failed = true;
            return Err(Error::new(
                ErrorKind::Usage,
                "invalid or existing output name",
            ));
        }
        let (temp, file) = loop {
            let temp = format!(
                ".{name}.unemu-export-{}-{}.part",
                std::process::id(),
                self.next_temp
            );
            self.next_temp += 1;
            match self.directory.create(&temp) {
                Ok(file) => break (temp, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    self.failed = true;
                    return Err(storage(error));
                }
            }
        };
        let output = Output {
            name: name.into(),
            temp: Some(temp),
            file,
            closed: false,
            hash: Sha256::new(),
            size: 0,
            receipt: None,
            #[cfg(test)]
            fault: self.fault.take(),
            hook: self.hook.take(),
        };
        let output = self.outputs.entry(name.into()).or_insert(output);
        #[cfg(test)]
        if output.fault == Some(unemu_core_api::test_support::ExportOperation::Create) {
            output.closed = true;
            return Err(storage("planted create failure after reservation"));
        }
        Ok(output)
    }
}
struct Output<'a> {
    name: String,
    temp: Option<String>,
    file: File,
    closed: bool,
    hash: Sha256,
    size: u64,
    #[cfg(test)]
    fault: Option<unemu_core_api::test_support::ExportOperation>,
    receipt: Option<ExportedOutput>,
    hook: Option<&'a mut dyn FnMut(u64) -> std::io::Result<()>>,
}
impl ExportOutput for Output<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let result = (|| {
            if self.closed {
                return Err(storage("export output closed"));
            }
            self.file.write_all(bytes).map_err(storage)?;
            #[cfg(test)]
            if self.fault == Some(unemu_core_api::test_support::ExportOperation::Write) {
                return Err(storage("planted write failure after append"));
            }
            self.hash.update(bytes);
            self.size += bytes.len() as u64;
            if let Some(hook) = self.hook.as_mut() {
                hook(self.size).map_err(storage)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.closed = true;
        }
        result
    }
    fn finish(&mut self) -> Result<ExportedOutput, Error> {
        if self.closed {
            return Err(storage("export output closed"));
        }
        self.closed = true;
        let result = self
            .file
            .flush()
            .and_then(|()| self.file.sync_all())
            .map_err(storage);
        #[cfg(test)]
        if self.fault == Some(unemu_core_api::test_support::ExportOperation::Finish) {
            return Err(storage("planted finish failure after close"));
        }
        result?;
        let receipt =
            ExportedOutput::new(&self.name, self.size, self.hash.clone().finalize().into());
        self.receipt = Some(receipt.clone());
        Ok(receipt)
    }
}

#[cfg(unix)]
struct Directory(File);

#[cfg(unix)]
impl Directory {
    fn open(path: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map(Self)
    }
    fn is_within(&self, root: &Path) -> std::io::Result<bool> {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::{MetadataExt, OpenOptionsExt},
        };
        let root = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(root)?;
        let identity = |file: &File| -> std::io::Result<_> {
            let metadata = file.metadata()?;
            Ok((metadata.dev(), metadata.ino()))
        };
        let store = identity(&root)?;
        let mut current = self.0.try_clone()?;
        loop {
            let here = identity(&current)?;
            if here == store {
                return Ok(true);
            }
            // SAFETY: the held fd is live and the parent name is NUL-terminated.
            let fd = unsafe {
                libc::openat(
                    current.as_raw_fd(),
                    c"..".as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: openat returned a new fd, whose sole owner becomes this File.
            let parent = unsafe { File::from_raw_fd(fd) };
            if identity(&parent)? == here {
                return Ok(false);
            }
            current = parent;
        }
    }
    fn check_temp(&self, temp: &std::ffi::CStr, file: &File) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        let mut held = std::mem::MaybeUninit::<libc::stat>::uninit();
        let mut named = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: both fds and the C string remain live; each stat points to writable storage.
        let result = unsafe {
            libc::fstat(file.as_raw_fd(), held.as_mut_ptr()) == 0
                && libc::fstatat(
                    self.0.as_raw_fd(),
                    temp.as_ptr(),
                    named.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                ) == 0
        };
        if !result {
            return Err(std::io::Error::other(format!(
                "cannot verify temporary file {}: {}",
                temp.to_string_lossy(),
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: both stat calls succeeded and initialized their output.
        let (held, named) = unsafe { (held.assume_init(), named.assume_init()) };
        if (held.st_dev, held.st_ino) != (named.st_dev, named.st_ino) {
            return Err(std::io::Error::other(format!(
                "temporary file {} no longer matches the created file",
                temp.to_string_lossy()
            )));
        }
        Ok(())
    }
    fn create(&self, name: &str) -> std::io::Result<File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = std::ffi::CString::new(name)?;
        // SAFETY: the directory fd and NUL-terminated name remain live for the call.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            )
        };
        if fd < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            // SAFETY: openat returned a new fd, whose sole owner becomes this File.
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }
    fn link(&self, temp: &str, file: &File, name: &str) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        let temp = std::ffi::CString::new(temp)?;
        let name = std::ffi::CString::new(name)?;
        self.check_temp(&temp, file)?;
        // SAFETY: both names are live C strings, relative to the held directory fd.
        let result = unsafe {
            libc::linkat(
                self.0.as_raw_fd(),
                temp.as_ptr(),
                self.0.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        };
        if result < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    fn unlink(&self, temp: &str, file: &File) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        let temp = std::ffi::CString::new(temp)?;
        self.check_temp(&temp, file)?;
        // SAFETY: the temporary name is a live C string under the held directory fd.
        let result = unsafe { libc::unlinkat(self.0.as_raw_fd(), temp.as_ptr(), 0) };
        if result < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    fn sync(&self) -> std::io::Result<()> {
        self.0.sync_all()
    }
}

#[cfg(not(unix))]
struct Directory(std::path::PathBuf);

#[cfg(not(unix))]
impl Directory {
    fn open(path: &Path) -> std::io::Result<Self> {
        // Extended Windows paths keep names such as con and nul as ordinary entries.
        path.canonicalize().map(Self)
    }
    fn is_within(&self, root: &Path) -> std::io::Result<bool> {
        Ok(self.0.starts_with(root.canonicalize()?))
    }
    fn create(&self, temp: &str) -> std::io::Result<File> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.0.join(temp))
    }
    fn link(&self, temp: &str, _file: &File, name: &str) -> std::io::Result<()> {
        std::fs::hard_link(self.0.join(temp), self.0.join(name))
    }
    fn unlink(&self, temp: &str, _file: &File) -> std::io::Result<()> {
        std::fs::remove_file(self.0.join(temp))
    }
    fn sync(&self) -> std::io::Result<()> {
        // Windows has no portable directory flush through std.
        Ok(())
    }
}
