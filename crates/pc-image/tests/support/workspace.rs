// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic workspace fixture shared by isolated persistence test binaries.
use image_store::{BlobSource, Descriptor, Store};
use pc_image::{workspace, PcProvider};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub struct Fixture {
    pub path: PathBuf,
    pub store: Store,
    pub image: Descriptor,
}
impl Fixture {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "image-workspaces-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (from, to) in [
            ("vendor/kiselev-bios/bios-xi8088.bin", "bios.bin"),
            (
                "vendor/xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
                "xtide.bin",
            ),
        ] {
            fs::copy(root.join(from), path.join(to)).unwrap();
        }
        fs::write(path.join("fixed.img"), vec![0; 32 * 512]).unwrap();
        fs::write(path.join("one.img"), vec![0; 360 * 1024]).unwrap();
        fs::write(path.join("two.img"), vec![0; 360 * 1024]).unwrap();
        fs::write(path.join("cmos.bin"), [0; 64]).unwrap();
        let file = path.join("machine.toml");
        fs::write(&file, "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[[firmware.option_roms]]\nimage='xtide.bin'\nbase=0xC8000\nprovides=['fixed-disk']\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='journal'\njournal='source-cmos.upfj'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\n[[card.drives.media]]\nimage='one.img'\nwrite='journal'\njournal='source-one.upfj'\n[[card.drives.media]]\nimage='two.img'\nwrite='journal'\njournal='source-two.upfj'\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={cylinders=1,heads=1,sectors=32}\nimage='fixed.img'\nwrite='journal'\njournal='source-fixed.upfj'\nkeep_generations=3\n").unwrap();
        let store = Store::open(path.join("store")).unwrap();
        let image = pc_image::import_machine(&store, &file, "test:source", &Default::default())
            .unwrap()
            .manifest;
        Self { path, store, image }
    }
    pub fn changed(&self, name: &str, value: u8) {
        self.changed_from(name, "test:source", value);
    }
    pub fn changed_from(&self, name: &str, tag: &str, value: u8) {
        let owner = self.store.workspace(name).unwrap();
        let machine = workspace::start(&owner, tag).unwrap();
        let image = self
            .store
            .resolve_descriptor(&self.store.tag_descriptor(tag).unwrap(), &PcProvider)
            .unwrap();
        let config: toml::Table = std::str::from_utf8(&image.config).unwrap().parse().unwrap();
        for (drive, slot) in machine
            .drives
            .iter()
            .zip(config["slots"].as_array().unwrap())
        {
            let entries: Vec<_> = drive
                .media
                .as_ref()
                .map(|list| list.entries.iter().map(|m| &m.image).collect())
                .unwrap_or_else(|| vec![&drive.image]);
            for (source, id) in entries
                .into_iter()
                .zip(slot["catalogue"].as_array().unwrap())
            {
                let assembly::ImageSource::File(path) = source else {
                    panic!()
                };
                let bytes = fs::read(path).unwrap();
                let id = id.as_str().unwrap();
                let mut journal = media::JournalMedia::open(
                    &owner.path().join("media").join(format!("{id}.upfj")),
                    &bytes,
                    bytes.len() as u64,
                    Default::default(),
                )
                .unwrap();
                journal.write_batch(&[(0, [value; 512])]);
                journal.finish().unwrap();
            }
        }
        let d = image
            .manifest
            .layers
            .iter()
            .find(|d| d.media_type == pc_image::CMOS)
            .unwrap();
        let bytes = self.store.read_verified(d, 64).unwrap();
        let mut journal = media::JournalMedia::open(
            &owner.path().join("media/cmos.upfj"),
            &bytes,
            64,
            Default::default(),
        )
        .unwrap();
        let mut sector = [0; 512];
        sector[32] = value;
        journal.write_batch(&[(0, sector)]);
        journal.finish().unwrap();
    }
    pub fn assert_complete(&self, tag: &str, value: u8) {
        let config = pc_image::resolve(&self.store, tag, &self.path.join("inspect")).unwrap();
        for drive in &config.drives {
            let sources: Vec<_> = drive
                .media
                .as_ref()
                .map(|l| l.entries.iter().map(|m| &m.image).collect())
                .unwrap_or_else(|| vec![&drive.image]);
            for source in sources {
                let assembly::ImageSource::File(path) = source else {
                    panic!("expected file")
                };
                let base = media::VerifiedBase::open(path).unwrap();
                assert_eq!(
                    media::SectorSource::read_sector(&base, 0).unwrap(),
                    [value; 512]
                );
            }
        }
        let image = self.store.resolve(tag, &PcProvider).unwrap();
        let d = image
            .manifest
            .layers
            .iter()
            .find(|d| d.media_type == pc_image::CMOS)
            .unwrap();
        assert_eq!(self.store.read_verified(d, 64).unwrap()[32], value);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}
