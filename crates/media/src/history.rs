// SPDX-License-Identifier: AGPL-3.0-or-later
//! Closed-prefix reconstruction for export and retention; persistent-media.md
//! § Offline commands and § Retention anchor. Payload history stays on disk.

use crate::{scan_overlay_reader, SectorSource, SparseScan, VerifiedBase};
use std::io::{self, Read, Seek, SeekFrom};

pub(crate) fn prefix<R: Read + Seek + ?Sized>(
    source: &mut R,
    end: usize,
    base: &VerifiedBase,
) -> io::Result<SparseScan> {
    scan_overlay_reader(
        &mut Prefix {
            source,
            end: end as u64,
            position: 0,
        },
        base,
        base.byte_len(),
    )
}

struct Prefix<'a, R: ?Sized> {
    source: &'a mut R,
    end: u64,
    position: u64,
}
impl<R: Read + Seek + ?Sized> Read for Prefix<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let length = bytes
            .len()
            .min(self.end.saturating_sub(self.position) as usize);
        self.source.seek(SeekFrom::Start(self.position))?;
        let read = self.source.read(&mut bytes[..length])?;
        self.position += read as u64;
        Ok(read)
    }
}
impl<R: Seek + ?Sized> Seek for Prefix<'_, R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.end) + i128::from(n),
        };
        self.position =
            u64::try_from(position).map_err(|_| io::Error::other("invalid prefix seek"))?;
        Ok(self.position)
    }
}
