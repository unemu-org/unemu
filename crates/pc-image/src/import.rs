// SPDX-License-Identifier: AGPL-3.0-or-later
//! Read-only machine-file ingest and private journal recovery; machine-images §8.
//! Persistent-media § Open and tail recovery supplies the scanner and repair rules.

use crate::*;
use image_store::{media_type as mt, ConfigValidator, Store};
use machine_file::{schema::v1, ImageReader, Origins};
use media::SectorSource;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// A choice for exactly one named medium; generation ids are independent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryChoice {
    Recover,
    Generation(u64),
}
/// Explicit host inputs for an import. Unnamed clean journals use their tip.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct ImportOptions {
    pub recovery: BTreeMap<String, RecoveryChoice>,
    pub rom_dirs: Vec<PathBuf>,
    /// Store every payload raw, for fixtures that inspect exact blob bytes.
    pub no_compress: bool,
}
impl ImportOptions {
    /// Payload codecs offered to ingest; CMOS declares no compressed variant.
    fn codecs(&self) -> &'static [image_store::PayloadType] {
        if self.no_compress {
            &[]
        } else {
            PAYLOAD_TYPES
        }
    }
}
/// Local evidence for one independent medium, never a synchronized snapshot.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChosenGeneration {
    pub medium: String,
    pub policy: Policy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    pub recovered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_generations: Option<u32>,
}
/// Local import provenance. Stored outside immutable config/manifest identities.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRecord {
    pub schema: u32,
    pub manifest: Descriptor,
    pub generations: Vec<ChosenGeneration>,
    #[serde(skip)]
    pub warnings: Vec<String>,
}
impl ImportRecord {
    /// Read optional host-local provenance for a resolved immutable image.
    pub fn read(store: &Store, digest: &Digest) -> Result<Option<Self>> {
        let path = store
            .root()
            .join("local/imports")
            .join(format!("{}.toml", digest.hex()));
        match fs::read_to_string(path) {
            Ok(text) => {
                let record: Self = toml::from_str(&text).map_err(|e| invalid(e.to_string()))?;
                if record.schema != 1 || &record.manifest.digest != digest {
                    return Err(invalid("invalid local import record"));
                }
                Ok(Some(record))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn save(&self, store: &Store, scratch: &Scratch) -> Result<()> {
        let dir = store.root().join("local/imports");
        fs::create_dir_all(&dir)?;
        let temporary = scratch.path.join("record.toml");
        let mut file = fs::File::create_new(&temporary)?;
        file.write_all(
            toml::to_string(self)
                .map_err(|e| invalid(e.to_string()))?
                .as_bytes(),
        )?;
        file.sync_all()?;
        fs::rename(
            temporary,
            dir.join(format!("{}.toml", self.manifest.digest.hex())),
        )?;
        fs::File::open(dir)?.sync_all()?;
        fs::File::open(store.root().join("local"))?.sync_all()?;
        fs::File::open(store.root())?.sync_all()?;
        Ok(())
    }
}
fn source_path(origins: &Origins, field: &str, name: &str) -> Result<PathBuf> {
    Ok(origins.image_path(field, name))
}
fn inputs(document: &v1::MachineFile, origins: &Origins) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut hardware = encode::machine(document);
    firmware_images(&mut hardware, |field, rom| {
        if let Some(image) = rom.get("image").and_then(toml::Value::as_str) {
            paths.push(source_path(origins, &format!("{field}.image"), image)?);
        }
        Ok(())
    })?;
    if let Some(v1::Nvram::Image(n)) = document
        .board
        .as_ref()
        .and_then(|b| b.rtc.as_ref())
        .and_then(|r| r.nvram.as_ref())
    {
        if let Some(image) = &n.image {
            paths.push(source_path(origins, "board.rtc.nvram.image", image)?);
        }
    }
    for (c, card) in document.card.iter().flatten().enumerate() {
        for (d, drive) in card.drives.iter().flatten().enumerate() {
            let field = format!("card[{c}].drives[{d}]");
            if let Some(image) = &drive.image {
                paths.push(source_path(origins, &format!("{field}.image"), image)?);
            }
            for (m, medium) in drive.media.iter().flatten().enumerate() {
                paths.push(source_path(
                    origins,
                    &format!("{field}.media[{m}].image"),
                    &medium.image,
                )?);
            }
        }
    }
    Ok(paths)
}
fn put(store: &Store, kind: &str, bytes: &[u8]) -> Result<Descriptor> {
    let b = store.import(bytes, None)?;
    Ok(Descriptor::new(kind, b.digest, b.size))
}
fn expected(hash: [u8; 32]) -> Result<Digest> {
    Ok(format!(
        "sha256:{}",
        hash.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
    .parse()?)
}
#[derive(Default)]
struct BaseReader<'a> {
    bases: BTreeMap<PathBuf, media::VerifiedBase>,
    /// ROMs decoded from compressed firmware packages during resolution.
    prepared: Option<&'a machine_file::PreparedMachineFile>,
}
impl ImageReader for BaseReader<'_> {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        if let Some(bytes) = self.prepared.and_then(|p| p.firmware_image(path)) {
            return Ok(bytes.to_vec());
        }
        machine_file::FileImageReader.read(path)
    }
    fn read_journal_base(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        machine_file::FileImageReader.read_journal_base(path)
    }
    fn verify_journal_base(&mut self, path: &Path) -> io::Result<media::VerifiedBase> {
        if let Some(base) = self.bases.get(path) {
            base.check_unchanged()?;
            return Ok(base.clone());
        }
        let base = media::VerifiedBase::open(path)?;
        self.bases.insert(path.into(), base.clone());
        Ok(base)
    }
}
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch {
    path: PathBuf,
    _lock: fs::File,
}
impl Scratch {
    fn new(store: &Store) -> Result<Self> {
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(store.root().join("locks/pc-import.lock"))?;
        lock.lock()?;
        // The exclusive importer lock makes abandoned private snapshots sweepable.
        for entry in fs::read_dir(store.root().join("staging"))? {
            let entry = entry?;
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("pc-import-v1-")
                && entry.file_type()?.is_dir()
            {
                fs::remove_dir_all(entry.path())?;
            }
        }
        loop {
            let path = store.root().join("staging").join(format!(
                "pc-import-v1-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self { path, _lock: lock }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
struct SectorReader<'a> {
    source: &'a dyn SectorSource,
    position: u64,
}
impl Read for SectorReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = bytes
            .len()
            .min((self.source.byte_len() - self.position) as usize);
        if count == 0 {
            self.source.check_unchanged()?;
            return Ok(0);
        }
        if self.position.is_multiple_of(512)
            && (count.is_multiple_of(512) || self.source.byte_len() == 64)
        {
            self.source
                .read_range(self.position / 512, &mut bytes[..count])?;
        } else {
            let mut copied = 0;
            while copied < count {
                let position = self.position + copied as u64;
                let sector = self.source.read_sector(position / 512)?;
                let start = position as usize % 512;
                let n = (count - copied).min(512 - start);
                bytes[copied..copied + n].copy_from_slice(&sector[start..start + n]);
                copied += n;
            }
        }
        self.position += count as u64;
        Ok(count)
    }
}
struct Freezer<'a> {
    store: &'a Store,
    scratch: &'a Scratch,
    reader: &'a BaseReader<'a>,
    options: &'a ImportOptions,
    generations: Vec<ChosenGeneration>,
}
impl Freezer<'_> {
    fn freeze(
        &mut self,
        source: &assembly::ImageSource,
        hash: Option<[u8; 32]>,
        write: &assembly::WriteMode,
        kind: &str,
        id: &str,
    ) -> Result<Descriptor> {
        let policy = Policy::from_write(write);
        let mut record = ChosenGeneration {
            medium: id.into(),
            policy,
            generation: None,
            recovered: false,
            keep_generations: None,
        };
        let pin = hash.map(expected).transpose()?;
        let blob = if let assembly::WriteMode::Journal {
            path,
            keep_generations,
        } = write
        {
            record.keep_generations = Some(*keep_generations);
            let assembly::ImageSource::File(base_path) = source else {
                return Err(invalid("journal base must be a file"));
            };
            let owned;
            let base = if let Some(base) = self.reader.bases.get(base_path) {
                base
            } else {
                owned = media::VerifiedBase::open(base_path)?;
                &owned
            };
            if pin
                .as_ref()
                .is_some_and(|p| expected(base.identity().base_digest).as_ref().ok() != Some(p))
            {
                return Err(invalid(format!("base digest mismatch for {id}")));
            }
            let choice = self.options.recovery.get(id).copied();
            match fs::symlink_metadata(path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    if matches!(choice, Some(RecoveryChoice::Generation(n)) if n != 0) {
                        return Err(invalid(format!(
                            "{id}: generation unavailable; recoverable generations: 0"
                        )));
                    }
                    record.generation = Some(0);
                    self.store.import_payload(
                        SectorReader {
                            source: base,
                            position: 0,
                        },
                        kind,
                        base.byte_len(),
                        pin.as_ref(),
                        self.options.codecs(),
                    )?
                }
                Err(e) => return Err(e.into()),
                Ok(_) => {
                    let canonical = path.canonicalize()?;
                    let mut lock_name = canonical.as_os_str().to_os_string();
                    lock_name.push(".lock");
                    let _lock = match fs::File::open(Path::new(&lock_name)) {
                        Ok(lock) => {
                            lock.try_lock_shared().map_err(io::Error::other)?;
                            Some(lock)
                        }
                        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                        Err(e) => return Err(e.into()),
                    };
                    let mut original = fs::File::open(canonical)?;
                    let before = original.metadata()?;
                    let mut snapshot = fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create_new(true)
                        .open(self.scratch.path.join(format!("{id}.upfj")))?;
                    io::copy(&mut original, &mut snapshot)?;
                    let after = original.metadata()?;
                    if before.len() != after.len() || before.modified()? != after.modified()? {
                        return Err(invalid(format!(
                            "{id}: journal changed while taking snapshot"
                        )));
                    }
                    let scan = media::scan_overlay_reader(&mut snapshot, base, base.byte_len())?;
                    let report = scan.report;
                    if matches!(report.status, media::Status::Refused(_)) {
                        return Err(invalid(format!(
                            "journal for {id} refused: {:?}",
                            report.status
                        )));
                    }
                    let available = report
                        .generations
                        .iter()
                        .map(|g| g.id.to_string())
                        .chain(
                            (report
                                .header
                                .as_ref()
                                .is_some_and(|h| h.anchor_generation == 0))
                            .then_some("0".to_owned()),
                        )
                        .collect::<Vec<_>>()
                        .join(",");
                    if report.needs_recovery() && choice.is_none() {
                        return Err(invalid(format!("journal for {id} requires an explicit recovery choice; recoverable generations: {available}; tip: {:?}", report.unsealed_tip.as_ref().map(|t| t.generation))));
                    }
                    let generation = match choice {
                        Some(RecoveryChoice::Generation(n)) => {
                            let end = if n == 0
                                && report
                                    .header
                                    .as_ref()
                                    .is_some_and(|h| h.anchor_generation == 0)
                            {
                                media::HEADER_LEN
                            } else {
                                report.generations.iter().find(|g| g.id == n).map(|g| g.end_offset).ok_or_else(|| invalid(format!("{id}: generation {n} is not a retained closed generation; recoverable generations: {available}")))?
                            };
                            snapshot.set_len(end as u64)?;
                            n
                        }
                        _ => {
                            if report.needs_recovery() {
                                snapshot.set_len(report.safe_offset as u64)?;
                                snapshot.seek(SeekFrom::End(0))?;
                                if let Some(seal) =
                                    report.recovery_seal().map_err(|e| invalid(e.to_string()))?
                                {
                                    snapshot.write_all(
                                        &seal.encode().map_err(|e| invalid(e.to_string()))?,
                                    )?;
                                }
                            }
                            report
                                .unsealed_tip
                                .as_ref()
                                .map(|t| t.generation)
                                .or(report.last_closed_generation)
                                .ok_or_else(|| invalid("no closed generation"))?
                        }
                    };
                    let chosen = media::scan_overlay_reader(&mut snapshot, base, base.byte_len())?;
                    record.generation = Some(generation);
                    record.recovered =
                        chosen.report.generations.iter().any(|g| {
                            g.id == generation && g.reason == media::ClosingReason::Recovered
                        });
                    let overlay = chosen
                        .overlay
                        .ok_or_else(|| invalid("generation reconstruction refused"))?;
                    self.store.import_payload(
                        SectorReader {
                            source: &overlay,
                            position: 0,
                        },
                        kind,
                        overlay.byte_len(),
                        None,
                        self.options.codecs(),
                    )?
                }
            }
        } else {
            if self.options.recovery.contains_key(id) {
                return Err(invalid(format!(
                    "{id}: recovery choice requires journal policy"
                )));
            }
            match source {
                assembly::ImageSource::File(path) => {
                    if let Some(base) = self.reader.bases.get(path) {
                        self.store.import_payload(
                            SectorReader {
                                source: base,
                                position: 0,
                            },
                            kind,
                            base.byte_len(),
                            pin.as_ref(),
                            self.options.codecs(),
                        )?
                    } else {
                        self.store.import_payload(
                            fs::File::open(path)?,
                            kind,
                            fs::metadata(path)?.len(),
                            pin.as_ref(),
                            self.options.codecs(),
                        )?
                    }
                }
                assembly::ImageSource::Inline(bytes) => self.store.import_payload(
                    bytes.as_slice(),
                    kind,
                    bytes.len() as u64,
                    pin.as_ref(),
                    self.options.codecs(),
                )?,
                _ => return Err(invalid("unsupported media source")),
            }
        };
        self.generations.push(record);
        Ok(blob)
    }
}
fn write_mode(
    mode: Option<v1::WriteMode>,
    journal: Option<&str>,
    keep: Option<std::num::NonZeroU32>,
    field: &str,
    origins: &Origins,
) -> Result<assembly::WriteMode> {
    match mode.unwrap_or(v1::WriteMode::Volatile) {
        v1::WriteMode::Journal => Ok(assembly::WriteMode::Journal {
            path: origins.image_path(
                &format!("{field}.journal"),
                journal.ok_or_else(|| invalid("journal path required"))?,
            ),
            keep_generations: keep
                .map_or(assembly::WriteMode::DEFAULT_KEEP_GENERATIONS, |k| k.get()),
        }),
        v1::WriteMode::ReadOnly => {
            if journal.is_some() || keep.is_some() {
                return Err(invalid("journal settings require journal policy"));
            }
            Ok(assembly::WriteMode::ReadOnly)
        }
        v1::WriteMode::Volatile => {
            if journal.is_some() || keep.is_some() {
                return Err(invalid("journal settings require journal policy"));
            }
            Ok(assembly::WriteMode::Volatile)
        }
    }
}
fn streaming_document(
    document: &v1::MachineFile,
    origins: &Origins,
    scratch: &Scratch,
) -> Result<(v1::MachineFile, Vec<Vec<assembly::WriteMode>>)> {
    let mut stream = document.clone();
    let mut policies = Vec::new();
    let mut next = 0;
    for (c, card) in stream.card.iter_mut().flatten().enumerate() {
        let mut drives = card
            .drives
            .iter_mut()
            .flatten()
            .enumerate()
            .collect::<Vec<_>>();
        drives.sort_by_key(|(_, d)| {
            d.unit
                .unwrap_or(u8::from(d.channel == Some(v1::Channel::Slave)))
        });
        for (d, drive) in drives {
            let field = format!("card[{c}].drives[{d}]");
            let mut writes = Vec::new();
            if let Some(media) = &mut drive.media {
                for (i, m) in media.iter_mut().enumerate() {
                    writes.push(write_mode(
                        m.write,
                        m.journal.as_deref(),
                        m.keep_generations,
                        &format!("{field}.media[{i}]"),
                        origins,
                    )?);
                    if m.write != Some(v1::WriteMode::Journal) {
                        m.write = Some(v1::WriteMode::Journal);
                        m.journal = Some(
                            scratch
                                .path
                                .join(format!("validation-{next}.upfj"))
                                .to_string_lossy()
                                .into_owned(),
                        );
                        next += 1;
                    }
                }
            } else if drive.empty != Some(true) {
                writes.push(write_mode(
                    drive.write,
                    drive.journal.as_deref(),
                    drive.keep_generations,
                    &field,
                    origins,
                )?);
                if drive.write != Some(v1::WriteMode::Journal) {
                    drive.write = Some(v1::WriteMode::Journal);
                    drive.journal = Some(
                        scratch
                            .path
                            .join(format!("validation-{next}.upfj"))
                            .to_string_lossy()
                            .into_owned(),
                    );
                    next += 1;
                }
            }
            policies.push(writes);
        }
    }
    Ok((stream, policies))
}

/// Resolve existing file inputs, freeze independent recovery points and publish a
/// fully verified image. Source folders are read-only throughout the transaction.
///
/// A ROM resolved from a firmware package stays user-supplied in the machine
/// image unless the firmware catalogue lists it as open, the package marks it
/// open and the package carries its source archive; then the image embeds that
/// archive's layer (the same stored blob) and marks both open under the
/// catalogue licence. A ROM read from a plain file stays user-supplied.
pub fn import_machine(
    store: &Store,
    path: &Path,
    tag: &str,
    options: &ImportOptions,
) -> Result<ImportRecord> {
    store.check_new_tag(tag)?;
    let mut warnings = Vec::new();
    let roots = options
        .rom_dirs
        .iter()
        .map(|p| {
            assembly::firmware_discovery::FirmwareRoot::new(
                p,
                assembly::firmware_discovery::RootOrigin::CommandLine,
            )
        })
        .collect::<Vec<_>>();
    let prepared = machine_file::prepare_for_assembly_with_firmware_store(
        path,
        &roots,
        &mut warnings,
        Some(store),
    )?;
    let (document, origins) = prepared.resolved();
    // A ROM from a compressed firmware package exists only in memory.
    let present = |p: &Path| p.exists() || prepared.firmware_image(p).is_some();
    let missing = inputs(document, origins)?
        .into_iter()
        .filter(|p| !present(p))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let mut firmware = false;
        let mut hardware = encode::machine(document);
        firmware_images(&mut hardware, |field, rom| {
            if let Some(image) = rom.get("image").and_then(toml::Value::as_str) {
                firmware |= !present(&source_path(origins, &format!("{field}.image"), image)?);
            }
            Ok(())
        })?;
        return Err(Error::MissingInputs {
            paths: missing
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            firmware,
        });
    }
    let scratch = Scratch::new(store)?;
    let (stream, policies) = streaming_document(document, origins, &scratch)?;
    let mut reader = BaseReader {
        prepared: Some(&prepared),
        ..BaseReader::default()
    };
    let mut effective = machine_file::lower(&stream, origins, &mut reader)?;
    for (drive, writes) in effective.drives.iter_mut().zip(policies) {
        if let Some(list) = &mut drive.media {
            for (m, write) in list.entries.iter_mut().zip(writes) {
                m.write = write;
            }
        } else if let Some(write) = writes.into_iter().next() {
            drive.write = write;
        }
    }
    validate_bios(&effective)?;
    let mut hardware = encode::machine(document);
    crate::defaults::materialize(&mut hardware, &effective)?;
    let mut config = Config {
        schema: 1,
        platform: "ibm-pc-at".into(),
        machine: hardware,
        firmware: Vec::new(),
        media: Vec::new(),
        slots: Vec::new(),
        nvram: None,
    };
    let embedded = open_sources(store, prepared.firmware_sources())?;
    let mut layers = Vec::new();
    firmware_images(&mut config.machine, |field, rom| {
        let name = rom
            .get("image")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| invalid("unresolved firmware image"))?;
        let path = origins.image_path(&format!("{field}.image"), name);
        let pin: Option<Digest> = rom
            .get("sha256")
            .and_then(toml::Value::as_str)
            .map(|s| format!("sha256:{s}").parse())
            .transpose()?;
        let b = match prepared.firmware_image(&path) {
            Some(bytes) => store.import_payload(
                bytes,
                mt::FIRMWARE,
                bytes.len() as u64,
                pin.as_ref(),
                options.codecs(),
            )?,
            None => store.import_payload(
                fs::File::open(&path)?,
                mt::FIRMWARE,
                fs::metadata(&path)?.len(),
                pin.as_ref(),
                options.codecs(),
            )?,
        };
        let source = format!("assets/firmware-{}.bin", config.firmware.len());
        rom.insert("image".into(), source.clone().into());
        rom.insert("sha256".into(), b.decoded_digest()?.hex().into());
        let archive = embedded.get(&b.decoded_digest()?);
        config.firmware.push(Firmware {
            key: field.into(),
            source,
            blob: b.decoded_digest()?,
            source_archive: archive.map(|e| e.archive.decoded_digest()).transpose()?,
        });
        layers.push(b);
        if let Some(e) = archive {
            layers.push(e.archive.clone());
        }
        Ok(())
    })?;
    let mut freezer = Freezer {
        store,
        scratch: &scratch,
        reader: &reader,
        options,
        generations: Vec::new(),
    };
    let mut drive_index = 0;
    drive_tables(&mut config.machine, |controller, _, _| {
        let drive = effective
            .drives
            .get(drive_index)
            .ok_or_else(|| invalid("drive order differs from loader"))?;
        let mut catalogue = Vec::new();
        let entries = drive.media.as_ref().map_or_else(
            || vec![(&drive.image, drive.expected_sha256, &drive.write, None)],
            |list| {
                list.entries
                    .iter()
                    .map(|m| (&m.image, m.expected_sha256, &m.write, Some(m.name.clone())))
                    .collect()
            },
        );
        for (source, hash, write, name) in entries {
            let id = format!("medium-{}", config.media.len());
            let kind = if matches!(drive.kind, assembly::DriveKind::Floppy { .. }) {
                mt::FLOPPY
            } else {
                mt::DISK_BASE
            };
            let d = freezer.freeze(source, hash, write, kind, &id)?;
            config.media.push(Medium {
                id: id.clone(),
                source: format!("assets/{id}.img"),
                head: d.decoded_digest()?,
                name,
                policy: Policy::from_write(write),
                geometry: Geometry::of_medium(drive.kind, d.decoded_size()?)?,
            });
            catalogue.push(id);
            layers.push(d);
        }
        let inserted = if drive.media.as_ref().is_some_and(|l| l.empty) {
            None
        } else {
            catalogue.first().cloned()
        };
        config.slots.push(Slot {
            controller: controller.into(),
            unit: drive.unit,
            catalogue,
            catalogue_mode: drive.media.is_some(),
            inserted,
        });
        drive_index += 1;
        Ok(())
    })?;
    for device in &effective.devices {
        let assembly::DeviceKind::Rtc(rtc) = &device.kind else {
            continue;
        };
        let (d, mode) = match &rtc.nvram {
            assembly::NvramSource::Image(bytes) => (
                freezer.freeze(
                    &assembly::ImageSource::Inline(bytes.to_vec()),
                    None,
                    &assembly::WriteMode::Volatile,
                    CMOS,
                    "cmos",
                )?,
                NvramMode::Image,
            ),
            assembly::NvramSource::Journal {
                image,
                expected_sha256,
                journal,
                keep_generations,
            } => (
                freezer.freeze(
                    &assembly::ImageSource::File(image.clone()),
                    Some(*expected_sha256),
                    &assembly::WriteMode::Journal {
                        path: journal.clone(),
                        keep_generations: *keep_generations,
                    },
                    CMOS,
                    "cmos",
                )?,
                NvramMode::Journal,
            ),
            _ => continue,
        };
        let source = "assets/cmos.bin".to_owned();
        let rtc = config
            .machine
            .get_mut("board")
            .and_then(toml::Value::as_table_mut)
            .and_then(|b| b.get_mut("rtc"))
            .and_then(toml::Value::as_table_mut)
            .ok_or_else(|| invalid("missing RTC hardware"))?;
        rtc.insert(
            "nvram".into(),
            toml::Table::from_iter([("image".into(), source.clone().into())]).into(),
        );
        config.nvram = Some(Nvram {
            mode,
            source,
            head: d.decoded_digest()?,
        });
        layers.push(d);
    }
    for id in options.recovery.keys() {
        if !freezer.generations.iter().any(|g| &g.medium == id) {
            return Err(invalid(format!("unknown recovery medium: {id}")));
        }
    }
    let bytes = toml::to_string(&config).map_err(|e| invalid(e.to_string()))?;
    // The configuration and metadata are generated by this program from the
    // user's own settings, never vendor bytes, so they are open. ROMs, disks,
    // floppies and CMOS come from the user's files and stay user-supplied.
    let configuration = generated(put(store, mt::CONFIG, bytes.as_bytes())?);
    layers.push(put(store, mt::METADATA, crate::EMPTY_METADATA)?);
    let mut seen = std::collections::BTreeSet::new();
    layers.retain(|d| seen.insert(d.digest.clone()));
    for d in &mut layers {
        if d.media_type == mt::METADATA {
            *d = generated(d.clone());
        } else {
            d.annotations
                .insert("unemu.redistribution".into(), "user-supplied".into());
        }
    }
    // An open copyleft ROM travels with its source, so both stay open.
    for d in &mut layers {
        let id = d.decoded_digest()?;
        if d.decoded_media_type() == mt::FIRMWARE {
            if let Some(Embedded { archive, rom, .. }) = embedded.get(&id) {
                image_store::license::mark(d, Some(rom));
                d.annotations.insert(
                    image_store::license::SOURCE.into(),
                    archive.decoded_digest()?.to_string(),
                );
            }
        } else if let Some(e) = embedded.values().find(|e| e.archive.digest == d.digest) {
            image_store::license::mark(d, Some(&e.source));
        }
    }
    let mut manifest = image_store::Manifest::new_with_types(
        "ibm-pc-at",
        configuration,
        layers,
        PcProvider.payload_types(),
    )?;
    if !prepared.firmware_sources().is_empty() {
        let sources = prepared
            .firmware_sources()
            .iter()
            .map(|d| format!("\"{d}\""))
            .collect::<Vec<_>>()
            .join(",");
        manifest
            .annotations
            .insert("unemu.firmware.sources".into(), format!("[{sources}]"));
    }
    let manifest = store.import_manifest_with_types(&manifest, PcProvider.payload_types())?;
    let record = ImportRecord {
        schema: 1,
        manifest,
        generations: freezer.generations,
        warnings,
    };
    record.save(store, &scratch)?;
    store.update_tag(tag, &record.manifest, None, &PcProvider)?;
    Ok(record)
}

/// A package's source-archive layer an open ROM travels with.
struct Embedded {
    archive: Descriptor,
    rom: image_store::license::Expression,
    source: image_store::license::Expression,
}
/// Decoded ROM digests mapped to the source archive they travel with: ROMs of
/// the contributing packages that the catalogue lists as open (and whose
/// bytes it does not list as proprietary), that the package marks open, and
/// whose linked archive the package marks open too.
fn open_sources(store: &Store, packages: &[Digest]) -> Result<BTreeMap<Digest, Embedded>> {
    use image_store::license;
    let mut result = BTreeMap::new();
    if packages.is_empty() {
        return Ok(result);
    }
    let catalogue = crate::firmware::catalogue();
    let validator = image_store::firmware::Validator(&crate::firmware::PcFirmwareRules(catalogue));
    let mut after = None;
    loop {
        let page = store.list_tags(after.as_deref(), 1000)?;
        for tag in &page.tags {
            if !packages.contains(&tag.manifest.digest) {
                continue;
            }
            let image = store.resolve_descriptor(&tag.manifest, &validator)?;
            let config = image_store::firmware::Config::decode(&image.config)?;
            for rom in &config.roms {
                let Some(archive) = config.source_of(&rom.id) else {
                    continue;
                };
                let Some(assembly::FirmwareDistribution::Open {
                    license: listed, ..
                }) = rom
                    .catalogue_id
                    .as_deref()
                    .and_then(|id| catalogue.iter().find(|e| e.id == id))
                    .map(|e| e.distribution)
                else {
                    continue;
                };
                let listed = license::Expression::parse(listed)?;
                let layer = |digest: &Digest| {
                    image
                        .manifest
                        .layers
                        .iter()
                        .find(|d| d.names(digest))
                        .cloned()
                        .ok_or_else(|| invalid("firmware package layer missing"))
                };
                let archive = layer(&archive.blob)?;
                let Some(source) = license::open_license(&archive) else {
                    continue;
                };
                let locked = crate::firmware::proprietary(catalogue, &rom.sha256, rom.size)
                    .is_some()
                    || rom.parts.iter().any(|p| {
                        crate::firmware::proprietary(catalogue, &p.sha256, p.size).is_some()
                    });
                if locked {
                    continue;
                }
                for digest in rom.blob.iter().chain(rom.parts.iter().map(|p| &p.blob)) {
                    if license::open_license(&layer(digest)?).as_ref() == Some(&listed) {
                        result.insert(
                            digest.clone(),
                            Embedded {
                                archive: archive.clone(),
                                rom: listed.clone(),
                                source: source.clone(),
                            },
                        );
                    }
                }
            }
        }
        match page.next_after {
            Some(next) => after = Some(next),
            None => return Ok(result),
        }
    }
}
