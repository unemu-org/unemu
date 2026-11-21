// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where a journal is kept: one record of the contract
//! (`unemu_core_api::Record`), core-contract.md §12.5 (move 1).
//!
//! [`crate::JournalMedia`] reads, appends, cuts, synchronizes and replaces a
//! record. Behind it may be a journal file (supplied by its host), a buffer
//! in memory, or any storage a front end hands to the core. The journal's
//! bytes and recovery rules do not depend on which.

use std::io::{self, Read, Seek, SeekFrom};
use unemu_core_api as contract;

/// The contract's error as a host error with the same text, for the scans
/// and reports that speak `io::Error`.
pub(crate) fn io_error(error: contract::Error) -> io::Error {
    io::Error::other(error.message)
}

/// Bytes read at any offset: a record or a replacement.
pub(crate) trait ReadAt {
    fn len_now(&self) -> Result<u64, contract::Error>;
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), contract::Error>;
}

impl ReadAt for dyn contract::Record + '_ {
    fn len_now(&self) -> Result<u64, contract::Error> {
        self.size()
    }
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), contract::Error> {
        self.read_at(offset, buf)
    }
}

impl ReadAt for dyn contract::Replacement + '_ {
    fn len_now(&self) -> Result<u64, contract::Error> {
        self.size()
    }
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), contract::Error> {
        self.read_at(offset, buf)
    }
}

/// `Read` and `Seek` over bytes read at any offset, for the scans that take a
/// reader. The length is taken once, when the cursor is made.
pub(crate) struct Cursor<'a, T: ReadAt + ?Sized> {
    source: &'a mut T,
    len: u64,
    position: u64,
}

impl<'a, T: ReadAt + ?Sized> Cursor<'a, T> {
    pub(crate) fn new(source: &'a mut T) -> io::Result<Self> {
        let len = source.len_now().map_err(io_error)?;
        Ok(Self {
            source,
            len,
            position: 0,
        })
    }
}

impl<T: ReadAt + ?Sized> Read for Cursor<'_, T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let remaining =
            usize::try_from(self.len.saturating_sub(self.position)).unwrap_or(usize::MAX);
        let count = bytes.len().min(remaining);
        self.source
            .read_exact_at(self.position, &mut bytes[..count])
            .map_err(io_error)?;
        self.position += count as u64;
        Ok(count)
    }
}

impl<T: ReadAt + ?Sized> Seek for Cursor<'_, T> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.len) + i128::from(n),
        };
        self.position =
            u64::try_from(position).map_err(|_| io::Error::other("invalid record seek"))?;
        Ok(self.position)
    }
}
