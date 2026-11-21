// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless desktop sessions use synthetic firmware, disks, catalogue and CMOS.
//!
//! The library's test support, for this crate's tests and the desktop's
//! (feature `test-support`): a store with a synthetic machine image, a store
//! with the open firmware packages, installation sets imported as the command
//! line imports them, and a synthetic firmware catalogue.
use image_store::{Digest, Store};
use library_model::listing::ImageRow;
use std::{fs, path::Path};
#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;
/// One medium of an installation set, as `unemu image media import` takes
/// it: the file, and its `--label N=TEXT` and `--format N=FORMAT` if given.
pub struct Medium<'a> {
    pub path: &'a Path,
    pub label: Option<&'a str>,
    pub format: Option<&'a str>,
}
impl<'a> Medium<'a> {
    pub fn new(path: &'a Path) -> Self {
        Self {
            path,
            label: None,
            format: None,
        }
    }
    pub fn label(self, label: &'a str) -> Self {
        Self {
            label: Some(label),
            ..self
        }
    }
    pub fn format(self, format: &'a str) -> Self {
        Self {
            format: Some(format),
            ..self
        }
    }
}

/// Publish an installation set titled `title` as `tag` into `store`, as
/// `unemu image media import` does with the same files, labels, formats and
/// extras (each with its DOS name, if given): compressed, and never moving an
/// existing tag. Returns the set's manifest.
pub fn import_media_set(
    store: &Store,
    tag: &str,
    title: &str,
    media: &[Medium<'_>],
    extras: &[(&Path, Option<&str>)],
) -> image_store::Descriptor {
    use pc_image::media_import;
    store.check_new_tag(tag).unwrap();
    let request = media_import::Request {
        tag: tag.into(),
        title: image_store::metadata::Title {
            title: title.into(),
            ..Default::default()
        },
        labels: Vec::new(),
        media: media
            .iter()
            .map(|medium| media_import::Source {
                path: medium.path.to_owned(),
                label: medium.label.map(Into::into),
                format: medium.format.map(Into::into),
                file_name: None,
            })
            .collect(),
        extras: extras
            .iter()
            .map(|(path, dos_name)| media_import::ExtraSource {
                path: path.to_path_buf(),
                dos_name: dos_name.map(Into::into),
                kind: None,
            })
            .collect(),
        compress: true,
    };
    let plan = media_import::plan_with(&request, None).unwrap();
    media_import::publish(store, &plan, &|_, _| {}, &|| false)
        .unwrap()
        .descriptor()
        .clone()
}

pub struct Fixture {
    pub temp: assembly::test_support::TempDir,
    pub store: Store,
    pub row: ImageRow,
}
impl Fixture {
    // Each call writes a new store to disk; a `Default` would hide that.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let temp = assembly::test_support::TempDir::new("desktop-session");
        fs::write(temp.path().join("bios.bin"), [0xf4; 32768]).unwrap();
        fs::write(temp.path().join("disk.img"), [0; 16384]).unwrap();
        fs::write(temp.path().join("one.img"), vec![0; 360 * 1024]).unwrap();
        fs::write(temp.path().join("two.img"), vec![0; 360 * 1024]).unwrap();
        fs::write(temp.path().join("cmos.bin"), [0; 64]).unwrap();
        let path = temp.path().join("machine.toml");
        fs::write(&path,"schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nbase=0xF8000\nprovides=['floppy','fixed-disk']\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='journal'\njournal='cmos.upfj'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\n[[card.drives.media]]\nimage='one.img'\nwrite='journal'\njournal='one.upfj'\n[[card.drives.media]]\nimage='two.img'\nwrite='journal'\njournal='two.upfj'\n[[card]]\nkind='ide'\n[[card.drives]]\nchannel='master'\ngeometry={cylinders=1,heads=1,sectors=32}\nimage='disk.img'\nwrite='journal'\njournal='disk.upfj'\n").unwrap();
        let store = Store::open(temp.path().join("store")).unwrap();
        let descriptor = pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::import_machine(&store, &path, "test:source", &Default::default())
        })
        .unwrap()
        .manifest;
        let row = ImageRow {
            reference: "test:source".into(),
            aliases: Vec::new(),
            descriptor,
            summary: None,
            failure: None,
        };
        Self { temp, store, row }
    }
    /// Import a one-disk 360K installation set, `disks-set:r1`, as the command
    /// line does; returns its manifest.
    pub fn import_set(&self) -> image_store::Descriptor {
        let disk = self.temp.path().join("set-disk.img");
        let args: Vec<std::ffi::OsString> = vec![
            "floppy".into(),
            "--type".into(),
            "360K".into(),
            "--format".into(),
            disk.clone().into_os_string(),
        ];
        pc_image::media_create::execute(
            pc_image::media_create::parse(&args).unwrap(),
            &mut std::io::sink(),
        )
        .unwrap();
        import_media_set(
            &self.store,
            "disks-set:r1",
            "Session disks",
            &[Medium::new(&disk)],
            &[],
        );
        self.store.tag_descriptor("disks-set:r1").unwrap()
    }
    pub fn change(&self, prepared: &pc_image::workspace::PreparedSession, value: u8) {
        let change = |source: &assembly::ImageSource, write: &assembly::WriteMode| {
            let assembly::ImageSource::File(path) = source else {
                panic!("expected file")
            };
            let assembly::WriteMode::Journal { path: journal, .. } = write else {
                panic!("expected journal")
            };
            // A compressed floppy has no file; its verified base holds the bytes.
            let bytes = match prepared.bases.get(path) {
                Some(base) if base.metadata().is_none() => {
                    let mut bytes = vec![0; journal_files::SectorSource::byte_len(base) as usize];
                    journal_files::SectorSource::read_range(base, 0, &mut bytes).unwrap();
                    bytes
                }
                _ => fs::read(path).unwrap(),
            };
            let mut media = journal_files::JournalMedia::open(
                journal,
                &bytes,
                bytes.len() as u64,
                Default::default(),
            )
            .unwrap();
            media.write_batch(&[(0, [value; 512])]);
            media.finish().unwrap();
        };
        for drive in &prepared.machine.drives {
            if let Some(list) = &drive.media {
                for entry in &list.entries {
                    change(&entry.image, &entry.write);
                }
            } else {
                change(&drive.image, &drive.write);
            }
        }
        for device in &prepared.machine.devices {
            if let assembly::DeviceKind::Rtc(rtc) = &device.kind {
                if let assembly::NvramSource::Journal { image, journal, .. } = &rtc.nvram {
                    let bytes = fs::read(image).unwrap();
                    let mut media =
                        journal_files::JournalMedia::open(journal, &bytes, 64, Default::default())
                            .unwrap();
                    let mut sector = [0; 512];
                    sector[..64].copy_from_slice(&bytes);
                    sector[32] = value;
                    media.write_batch(&[(0, sector)]);
                    media.finish().unwrap();
                }
            }
        }
    }
}

/// The source archive the open entry's fetch script builds and the catalogue
/// pins, so the copyleft package is open together with its source.
pub fn source_archive(candidate: &pc_image::firmware_import::Candidate) -> &'static str {
    match candidate.distribution {
        assembly::FirmwareDistribution::Open { fetch, .. } => match fetch {
            "vendor/unemu-at-bios/fetch.sh" => {
                "vendor/unemu-at-bios/unemu-at-bios-1.0.2-unemu.3-src.tar"
            }
            "vendor/vgabios/fetch.sh" => "vendor/vgabios/vgabios-0.9d-unemu.1-src.tar",
            "vendor/xtide-universal-bios/fetch.sh" => {
                "vendor/xtide-universal-bios/xtide-universal-bios-r638-src.tar"
            }
            other => panic!("no source archive for {other}"),
        },
        assembly::FirmwareDistribution::UserSupplied => panic!("not open firmware"),
    }
}

/// A store with the open firmware packages (the open AT BIOS, the open VGA
/// BIOS and XTIDE) and, with `misfits`, a user-supplied package of the
/// Kiselev Xi 8088 build plus synthetic packages that must not fit: an
/// XT-class BIOS and one naming no board. `None` when a fetched fixture it reads is missing,
/// which skips the test.
pub struct FirmwareStore {
    pub temp: assembly::test_support::TempDir,
    pub store: Store,
}
impl FirmwareStore {
    pub fn new(misfits: bool) -> Option<Self> {
        Self::build(misfits, true)
    }
    /// The open packages imported without their source archives, which
    /// keeps the copyleft ones user-supplied.
    pub fn without_sources() -> Option<Self> {
        Self::build(false, false)
    }
    fn build(misfits: bool, sources: bool) -> Option<Self> {
        let mut images = Vec::new();
        for (from, to) in [
            ("unemu-at-bios/bios-unemu-at.bin", "bios.bin"),
            ("vgabios/vgabios-std.bin", "vga.bin"),
            (
                "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
                "xtide.bin",
            ),
        ] {
            images.push((fixtures::fixture(from)?, to));
        }
        if sources {
            for archive in [
                "unemu-at-bios/unemu-at-bios-1.0.2-unemu.3-src.tar",
                "vgabios/vgabios-0.9d-unemu.1-src.tar",
                "xtide-universal-bios/xtide-universal-bios-r638-src.tar",
            ] {
                fixtures::fixture(archive)?;
            }
        }
        let misfit = if misfits {
            Some(fixtures::fixture("kiselev-bios/bios-xi8088.bin")?)
        } else {
            None
        };
        let temp = assembly::test_support::TempDir::new("new-machine-firmware");
        let roms = temp.path().join("roms");
        fs::create_dir(&roms).unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (bytes, to) in images {
            fs::write(roms.join(to), bytes).unwrap();
        }
        let store = Store::open(temp.path().join("store")).unwrap();
        let catalogue = pc_image::firmware::catalogue();
        let scan = pc_image::firmware_import::scan(std::slice::from_ref(&roms), catalogue).unwrap();
        assert_eq!(scan.candidates.len(), 3, "{:?}", scan.issues);
        for candidate in &scan.candidates {
            // The copyleft packages are open only with the archives their
            // fetch scripts build, which the catalogue pins.
            let statement = pc_image::firmware_import::Statement {
                source: sources.then(|| root.join(source_archive(candidate))),
                ..Default::default()
            };
            pc_image::firmware_import::publish_with(&store, candidate, &[], &statement, catalogue)
                .unwrap();
        }
        if let Some(bios) = misfit {
            fs::write(temp.path().join("xi8088.bin"), &bios).unwrap();
            let digest = image_store::Digest::of(&bios);
            let synthetic = vec![0xf4; 32768];
            let other = image_store::Digest::of(&synthetic);
            fs::write(roms.join("synthetic.bin"), &synthetic).unwrap();
            for (tag, name, catalogue_id, fits, source, digest, distribution) in [
                (
                    "private-kiselev",
                    "Kiselev AT BIOS (private copy)",
                    "catalogue_id='kiselev-xi8088-1.0.2'\n",
                    "boards=['ibm-5170']\nmachine_classes=['at']\n",
                    "../xi8088.bin",
                    &digest,
                    assembly::FirmwareDistribution::UserSupplied,
                ),
                (
                    "xt-bios",
                    "Synthetic XT BIOS",
                    "",
                    "boards=['ibm-5160']\nmachine_classes=['xt']\n",
                    "synthetic.bin",
                    &other,
                    assembly::FirmwareDistribution::UserSupplied,
                ),
                (
                    "unlisted-bios",
                    "Unlisted 286 BIOS",
                    "",
                    "machine_classes=['at']\n",
                    "synthetic.bin",
                    &other,
                    assembly::FirmwareDistribution::UserSupplied,
                ),
            ] {
                let config = image_store::firmware::Config::decode(
                    format!(
                        "schema=1\nplatform='ibm-pc'\nname='{name}'\nvendor='Synthetic vendor'\n[fits]\n{fits}[[roms]]\nid='system-bios'\nrole='system-bios'\n{catalogue_id}size=32768\nsha256='{}'\nblob='{}'\n",
                        digest.hex(),
                        digest
                    )
                    .as_bytes(),
                )
                .unwrap();
                let candidate = pc_image::firmware_import::Candidate {
                    catalogue_id: tag.into(),
                    config,
                    sources: vec![roms.join(source)],
                    distribution,
                };
                pc_image::firmware_import::publish(&store, &candidate, &[], catalogue).unwrap();
            }
        }
        Some(Self { temp, store })
    }
    /// Packages, tags and restricted blobs, as the worker hands them to the wizard.
    pub fn choices(&self) -> crate::library_worker::StoreChoices {
        crate::library_worker::store_choices(&self.store).unwrap()
    }
}

fn leaked(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}
fn hash(bytes: &[u8]) -> &'static str {
    leaked(Digest::of(bytes).hex().into())
}
pub fn two_chip(id: &'static str, even: u8, odd: u8) -> assembly::FirmwareEntry {
    let part = |label, part_number, lane, byte: u8| assembly::KnownPart {
        label,
        part_number,
        lane,
        offset: 0,
        size: 16384,
        sha256: hash(&[byte; 16384]),
    };
    let layout: &'static [assembly::KnownPart] = Box::leak(Box::new([
        part("U27", "test-even", assembly::Lane::Even, even),
        part("U47", "test-odd", assembly::Lane::Odd, odd),
    ]));
    assembly::FirmwareEntry {
        id,
        sha256: hash(&[even, odd].repeat(16384)),
        size: 32768,
        parts: Box::leak(Box::new([layout])),
        distribution: assembly::FirmwareDistribution::UserSupplied,
        ..assembly::KISELEV_XI8088
    }
}
pub fn single(id: &'static str, byte: u8, open: bool) -> assembly::FirmwareEntry {
    assembly::FirmwareEntry {
        id,
        sha256: hash(&[byte; 32768]),
        size: 32768,
        distribution: if open {
            assembly::FirmwareDistribution::Open {
                fetch: "synthetic/fetch.sh",
                license: "MIT",
                provenance: "synthetic build",
                source_archive_sha256: None,
            }
        } else {
            assembly::FirmwareDistribution::UserSupplied
        },
        ..assembly::KISELEV_XI8088
    }
}
/// A POSIX tar archive (ustar magic at offset 257) of `size` bytes, `fill`
/// after the header, as the synthetic copyleft BIOS's source.
pub fn tar(size: usize, fill: u8) -> Vec<u8> {
    let mut bytes = vec![fill; size.max(512)];
    bytes[..257].fill(0);
    bytes[..10].copy_from_slice(b"src/a.asm\0");
    bytes[257..263].copy_from_slice(b"ustar\0");
    bytes
}
/// The archive the synthetic catalogue pins for its copyleft BIOS.
pub fn pinned_tar() -> Vec<u8> {
    tar(6000, 0x5a)
}
/// The open image has the bytes of the synthetic machine BIOS, so that
/// machine is a "Used by" entry of the open package.
pub fn synthetic_catalogue() -> &'static [assembly::FirmwareEntry] {
    static CATALOGUE: std::sync::OnceLock<Vec<assembly::FirmwareEntry>> =
        std::sync::OnceLock::new();
    CATALOGUE.get_or_init(|| {
        vec![
            two_chip("synthetic-two-chip-bios", 0x31, 0x72),
            two_chip("synthetic-incomplete-bios", 0x41, 0x82),
            single("synthetic-open-bios", 0xf4, true),
            assembly::FirmwareEntry {
                distribution: assembly::FirmwareDistribution::Open {
                    fetch: "synthetic/copyleft-fetch.sh",
                    license: "GPL-3.0-or-later",
                    provenance: "synthetic copyleft build",
                    source_archive_sha256: Some(hash(&pinned_tar())),
                },
                ..single("synthetic-copyleft-bios", 0xc5, true)
            },
        ]
    })
}
/// Tag `tag` as a newer release would write it into a store both share:
/// `source`'s artifact with `marker` as its `unemu.min-app-version`, and,
/// when `future_key`, a config led by a key this release's strict decoder
/// refuses (machine-images §5.6). Returns its manifest.
pub fn tag_newer(
    store: &Store,
    source: &str,
    tag: &str,
    marker: Option<&str>,
    future_key: bool,
) -> image_store::Descriptor {
    use image_store::{compat, BlobSource, Manifest, MAX_SMALL_BLOB};
    let source = store.tag_descriptor(source).unwrap();
    let mut manifest = Manifest::decode_with_types(
        &store.read_verified(&source, MAX_SMALL_BLOB).unwrap(),
        pc_image::PAYLOAD_TYPES,
    )
    .unwrap();
    if future_key {
        let mut config = b"future_option = 1\n".to_vec();
        config.extend(
            store
                .read_verified(&manifest.config, MAX_SMALL_BLOB)
                .unwrap(),
        );
        let blob = store.import(config.as_slice(), None).unwrap();
        manifest.config.digest = blob.digest;
        manifest.config.size = blob.size;
    }
    match marker {
        Some(marker) => manifest
            .annotations
            .insert(compat::MIN_APP_VERSION.into(), marker.into()),
        None => manifest.annotations.remove(compat::MIN_APP_VERSION),
    };
    let image = store
        .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
        .unwrap();
    store.tag_as_newer_release(tag, &image).unwrap();
    store.tag_descriptor(tag).unwrap()
}

/// A summary cache of `store` with every image summarised.
pub fn warm(store: &Store) -> image_store::summary::SummaryCache {
    let mut cache = image_store::summary::SummaryCache::open(store).unwrap();
    while !cache.pending().is_empty() {
        cache.rebuild(store, 8, &[], &[]).unwrap();
    }
    cache
}
