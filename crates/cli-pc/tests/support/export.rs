// SPDX-License-Identifier: AGPL-3.0-or-later
use cli_core::image::machine_export::{self, FileSink, Snapshot};
use image_store::Store;
use std::{io, path::Path};
use unemu_core_api::{ConfigDocument, ContentReader, MediaDescription};

pub struct Source<'a> {
    pub snapshot: &'a pc_image::machine::export::Snapshot,
    pub store: &'a Store,
}
impl Snapshot for Source<'_> {
    fn config(&self) -> ConfigDocument<'_> {
        self.snapshot.document()
    }
    fn baseline(&self) -> anyhow::Result<Vec<MediaDescription>> {
        panic!("PC core describes its media")
    }
    fn current(&self, label: &str) -> anyhow::Result<Box<dyn ContentReader + '_>> {
        Ok(self.snapshot.current(self.store, label)?)
    }
}

pub trait Export {
    fn export(&self, store: &Store, slot: &str, directory: &Path) -> anyhow::Result<()>;
    fn export_with_hook(
        &self,
        store: &Store,
        slot: &str,
        directory: &Path,
        hook: impl FnMut(u64) -> io::Result<()>,
    ) -> anyhow::Result<()>;
}
impl Export for pc_image::machine::export::Snapshot {
    fn export(&self, store: &Store, slot: &str, directory: &Path) -> anyhow::Result<()> {
        self.export_with_hook(store, slot, directory, |_| Ok(()))
    }
    fn export_with_hook(
        &self,
        store: &Store,
        slot: &str,
        directory: &Path,
        mut hook: impl FnMut(u64) -> io::Result<()>,
    ) -> anyhow::Result<()> {
        let source = Source {
            snapshot: self,
            store,
        };
        let mut sink = FileSink::new(directory)?;
        sink.write_hook(&mut hook);
        machine_export::export(
            &unemu_pc_core::PcCore,
            pc_image::PLATFORM,
            &source,
            "raw",
            Some(slot),
            &mut sink,
        )?;
        Ok(())
    }
}
