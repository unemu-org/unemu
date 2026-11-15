// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic workspace fixture shared by isolated persistence test binaries.
use image_store::{BlobSource, Descriptor, Store};
use pc_image::{workspace, PcProvider};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
#[path = "../../../cli/tests/support/fixtures.rs"]
mod fixtures;
static NEXT: AtomicU64 = AtomicU64::new(0);
/// Bytes of a resolved medium: its file, its inline bytes, or, for a
/// compressed floppy held in memory, its verified decoded payload.
pub fn medium_bytes(
    image: &image_store::ResolvedImage,
    source: &assembly::ImageSource,
    expected: Option<[u8; 32]>,
) -> Vec<u8> {
    match source {
        assembly::ImageSource::File(path) if path.is_file() => fs::read(path).unwrap(),
        assembly::ImageSource::Inline(bytes) => bytes.clone(),
        _ => {
            let hex: String = expected
                .unwrap()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let mut bytes = Vec::new();
            image
                .content_reader(&format!("sha256:{hex}").parse().unwrap())
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            bytes
        }
    }
}
pub struct Fixture {
    pub path: PathBuf,
    pub store: Store,
    pub image: Descriptor,
}
impl Fixture {
    /// `None` when a fetched ROM fixture is missing, which skips the test.
    pub fn new() -> Option<Self> {
        Self::build(true)
    }
    /// The same machine with a stored CMOS image that is copied afresh at
    /// every start instead of journaled.
    #[allow(dead_code)]
    pub fn without_cmos_journal() -> Option<Self> {
        Self::build(false)
    }
    fn build(cmos_journal: bool) -> Option<Self> {
        let mut roms = Vec::new();
        for (from, to) in [
            ("kiselev-bios/bios-xi8088.bin", "bios.bin"),
            (
                "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
                "xtide.bin",
            ),
        ] {
            roms.push((fixtures::fixture(from)?, to));
        }
        let path = std::env::temp_dir().join(format!(
            "image-workspaces-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        for (bytes, to) in roms {
            fs::write(path.join(to), bytes).unwrap();
        }
        fs::write(path.join("fixed.img"), vec![0; 32 * 512]).unwrap();
        fs::write(path.join("one.img"), vec![0; 360 * 1024]).unwrap();
        fs::write(path.join("two.img"), vec![0; 360 * 1024]).unwrap();
        fs::write(path.join("cmos.bin"), [0; 64]).unwrap();
        let file = path.join("machine.toml");
        let text = "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[[firmware.option_roms]]\nimage='xtide.bin'\nbase=0xC8000\nprovides=['fixed-disk']\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='journal'\njournal='source-cmos.upfj'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\n[[card.drives.media]]\nimage='one.img'\nwrite='journal'\njournal='source-one.upfj'\n[[card.drives.media]]\nimage='two.img'\nwrite='journal'\njournal='source-two.upfj'\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={cylinders=1,heads=1,sectors=32}\nimage='fixed.img'\nwrite='journal'\njournal='source-fixed.upfj'\nkeep_generations=3\n";
        let text = if cmos_journal {
            text.to_owned()
        } else {
            text.replace(
                "write='journal'\njournal='source-cmos.upfj'\n[[card]]",
                "[[card]]",
            )
        };
        fs::write(&file, text).unwrap();
        let store = Store::open(path.join("store")).unwrap();
        let image = pc_image::import_machine(
            &store,
            &file,
            "test:source",
            &pc_image::ImportOptions {
                no_compress: true,
                ..Default::default()
            },
        )
        .unwrap()
        .manifest;
        Some(Self { path, store, image })
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
                .map(|list| {
                    list.entries
                        .iter()
                        .map(|m| (&m.image, m.expected_sha256))
                        .collect()
                })
                .unwrap_or_else(|| vec![(&drive.image, drive.expected_sha256)]);
            for ((source, expected), id) in entries
                .into_iter()
                .zip(slot["catalogue"].as_array().unwrap())
            {
                let bytes = medium_bytes(&image, source, expected);
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
        let image = self.store.resolve(tag, &PcProvider).unwrap();
        for drive in &config.drives {
            let sources: Vec<_> = drive
                .media
                .as_ref()
                .map(|l| {
                    l.entries
                        .iter()
                        .map(|m| (&m.image, m.expected_sha256))
                        .collect()
                })
                .unwrap_or_else(|| vec![(&drive.image, drive.expected_sha256)]);
            for (source, expected) in sources {
                assert_eq!(
                    &medium_bytes(&image, source, expected)[..512],
                    &[value; 512]
                );
            }
        }
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
