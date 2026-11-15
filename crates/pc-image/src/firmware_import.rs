// SPDX-License-Identifier: AGPL-3.0-or-later
//! Read-only ROM-folder matching and tag-last package import; machine-images §5.3.
//! Source names are local diagnostics only; exact hashes identify each chip.

use crate::firmware::{self, Fit, PcFirmwareRules};
use crate::{invalid, Result};
use image_store::{
    firmware::{Config, Part, Rom, Validator},
    media_set::{Extra, ExtraKind, MediaFormats},
    media_type as mt, BlobSource, Descriptor, Digest, Manifest, Store,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct Issue {
    pub source: PathBuf,
    pub reason: String,
}
#[derive(Debug)]
pub struct Candidate {
    pub catalogue_id: String,
    pub config: Config,
    pub sources: Vec<PathBuf>,
    pub distribution: assembly::FirmwareDistribution,
}
impl Candidate {
    pub fn tag(&self) -> String {
        format!("firmware/{}:1", self.catalogue_id)
    }
    /// Another catalogue entry pins the same logical ROM, so the folder alone
    /// cannot choose its identity: the user must select one entry.
    pub fn is_ambiguous(&self, catalogue: &[assembly::FirmwareEntry]) -> bool {
        self.config.roms.first().is_some_and(|rom| {
            catalogue.iter().any(|entry| {
                entry.id != self.catalogue_id
                    && entry.sha256 == rom.sha256
                    && entry.size == rom.size
            })
        })
    }
}
#[derive(Debug, Default)]
pub struct Scan {
    pub candidates: Vec<Candidate>,
    pub issues: Vec<Issue>,
    pub size_mismatches: usize,
    pub hash_mismatches: usize,
}
#[derive(Clone, Debug)]
pub struct Attachment {
    pub path: PathBuf,
    pub dos_name: Option<String>,
}

/// Host enumeration limits prevent an accidental large tree from becoming a ROM scan.
#[derive(Clone, Copy, Debug)]
pub struct ScanLimits {
    pub depth: usize,
    pub entries: usize,
}
impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            depth: 32,
            entries: 10_000,
        }
    }
}
struct Walk {
    visited: BTreeSet<PathBuf>,
    files: Vec<PathBuf>,
    issues: Vec<Issue>,
    limits: ScanLimits,
    entries: usize,
    limited: bool,
}
impl Walk {
    fn limit(&mut self, path: &Path, reason: &str) {
        if !self.limited {
            let maximum = if reason == "entry count" {
                self.limits.entries
            } else {
                self.limits.depth
            };
            self.issues.push(Issue { source: path.into(), reason: format!("scan limit reached ({reason}, maximum {maximum}); narrow the folder or supply explicit files") });
        }
        self.limited = true;
    }
    fn admit(&mut self, path: &Path) -> bool {
        if self.entries >= self.limits.entries {
            self.limit(path, "entry count");
            return false;
        }
        self.entries += 1;
        true
    }
    fn visit(&mut self, path: &Path, boundary: &Path, depth: usize) {
        let result = (|| -> std::io::Result<()> {
            let canonical = path.canonicalize()?;
            if !canonical.starts_with(boundary) {
                return Err(std::io::Error::other("symlink leaves import root"));
            }
            if !self.visited.insert(canonical) {
                return Ok(());
            }
            let metadata = fs::metadata(path)?;
            if metadata.is_dir() {
                if depth > self.limits.depth {
                    self.limit(path, "recursion depth");
                    return Ok(());
                }
                let mut children = Vec::new();
                for entry in fs::read_dir(path)? {
                    if !self.admit(path) {
                        return Ok(());
                    }
                    match entry {
                        Ok(entry) => children.push(entry.path()),
                        Err(e) => self.issues.push(Issue {
                            source: path.into(),
                            reason: format!("unreadable directory entry: {}", e.kind()),
                        }),
                    }
                }
                children.sort();
                for child in children {
                    self.visit(&child, boundary, depth + 1);
                    if self.limited {
                        break;
                    }
                }
            } else if metadata.is_file() {
                self.files.push(path.into());
            } else {
                return Err(std::io::Error::other("expected regular file"));
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.issues.push(Issue {
                source: path.into(),
                reason: format!("unreadable input: {e}"),
            });
        }
    }
}
/// Stable file order and full hash/size matching; no source or store is modified.
pub fn scan(inputs: &[PathBuf], catalogue: &[assembly::FirmwareEntry]) -> Result<Scan> {
    scan_with_limits(inputs, catalogue, ScanLimits::default())
}
/// A limited enumeration publishes no candidates, so filesystem listing order
/// cannot choose an arbitrary partial subset of a large tree.
pub fn scan_with_limits(
    inputs: &[PathBuf],
    catalogue: &[assembly::FirmwareEntry],
    limits: ScanLimits,
) -> Result<Scan> {
    let mut walk = Walk {
        visited: BTreeSet::new(),
        files: Vec::new(),
        issues: Vec::new(),
        entries: 0,
        limited: false,
        limits,
    };
    let mut inputs = inputs.to_vec();
    inputs.sort();
    for path in inputs {
        if !walk.admit(&path) {
            break;
        }
        match path.canonicalize() {
            Ok(boundary) => walk.visit(&path, &boundary, 0),
            Err(e) => walk.issues.push(Issue {
                source: path,
                reason: format!("unreadable input: {}", e.kind()),
            }),
        }
        if walk.limited {
            break;
        }
    }
    let mut scan = Scan {
        issues: walk.issues,
        ..Scan::default()
    };
    if walk.limited {
        return Ok(scan);
    }
    let mut files = walk.files;
    files.sort();
    let sizes: BTreeSet<_> = catalogue
        .iter()
        .flat_map(|e| {
            std::iter::once(e.size).chain(e.parts.iter().flat_map(|p| p.iter().map(|p| p.size)))
        })
        .collect();
    let mut found = BTreeMap::<(String, u64), PathBuf>::new();
    for path in files {
        let result = (|| -> Result<()> {
            let stat = fs::metadata(&path)?;
            if !sizes.contains(&stat.len()) {
                scan.size_mismatches += 1;
                return Ok(());
            }
            let hash = assembly::firmware_discovery::hash_firmware_file(&path)
                .map_err(|e| invalid(format!("unreadable ROM: {e}")))?;
            let matches = catalogue.iter().any(|e| {
                (e.sha256 == hash && e.size == stat.len())
                    || e.parts
                        .iter()
                        .flat_map(|l| l.iter())
                        .any(|p| p.sha256 == hash && p.size == stat.len())
            });
            if !matches {
                scan.hash_mismatches += 1;
                return Ok(());
            }
            found.entry((hash, stat.len())).or_insert(path.clone());
            Ok(())
        })();
        if let Err(e) = result {
            scan.issues.push(Issue {
                source: path,
                reason: e.to_string(),
            });
        }
    }
    for entry in catalogue {
        let mut choices = Vec::new();
        if let Some(path) = found.get(&(entry.sha256.into(), entry.size)) {
            choices.push((
                Some(format!("sha256:{}", entry.sha256).parse()?),
                Vec::new(),
                vec![path.clone()],
            ));
        }
        for layout in entry.parts {
            let present = layout
                .iter()
                .filter(|p| found.contains_key(&(p.sha256.into(), p.size)))
                .count();
            if present == 0 {
                continue;
            }
            if present != layout.len() {
                let missing = layout
                    .iter()
                    .filter(|p| !found.contains_key(&(p.sha256.into(), p.size)))
                    .map(|p| p.label)
                    .collect::<Vec<_>>()
                    .join(", ");
                scan.issues.push(Issue {
                    source: PathBuf::from(entry.id),
                    reason: format!(
                        "{present} of {} chips present, missing {missing}",
                        layout.len()
                    ),
                });
                continue;
            }
            let mut parts = Vec::new();
            let mut sources = Vec::new();
            for p in *layout {
                let path = found
                    .get(&(p.sha256.into(), p.size))
                    .ok_or_else(|| invalid("missing matched chip"))?;
                sources.push(path.clone());
                parts.push(Part {
                    blob: format!("sha256:{}", p.sha256).parse()?,
                    sha256: p.sha256.into(),
                    size: p.size,
                    position: Some(p.label.into()),
                    part_number: Some(p.part_number.into()),
                    lane: p.lane.to_string(),
                    offset: p.offset,
                });
            }
            choices.push((None, parts, sources));
        }
        if choices.len() > 1 {
            scan.issues.push(Issue {
                source: entry.id.into(),
                reason: "ambiguous complete layouts; supply files for exactly one layout".into(),
            });
            continue;
        }
        if let Some((blob, parts, sources)) = choices.pop() {
            let rom = Rom {
                id: firmware::role(entry.kind).into(),
                role: firmware::role(entry.kind).into(),
                catalogue_id: Some(entry.id.into()),
                sha256: entry.sha256.into(),
                size: entry.size,
                blob,
                position: None,
                part_number: None,
                parts,
            };
            let fit = Fit {
                boards: entry.boards.iter().map(|s| (*s).into()).collect(),
                machine_classes: if entry.boards.contains(&"ibm-5170") {
                    vec!["at".into()]
                } else {
                    Vec::new()
                },
            };
            let config = Config {
                schema: 1,
                platform: "ibm-pc-at".into(),
                name: entry.id.into(),
                vendor: None,
                version: None,
                date: None,
                notes: None,
                fits: fit.table()?,
                roms: vec![rom],
                extras: Vec::new(),
            };
            let candidate = Candidate {
                catalogue_id: entry.id.into(),
                config,
                sources,
                distribution: entry.distribution,
            };
            let descriptors = candidate
                .config
                .payloads()
                .into_iter()
                .map(|(hash, size, kind)| Descriptor::new(kind, hash, size))
                .collect::<Vec<_>>();
            let source = Files(&candidate);
            if let Err(e) = image_store::firmware::FirmwareRules::validate(
                &PcFirmwareRules(catalogue),
                &candidate.config,
                &source,
                &descriptors,
            ) {
                scan.issues.push(Issue {
                    source: entry.id.into(),
                    reason: format!("invalid complete set: {e}"),
                });
            } else {
                scan.candidates.push(candidate);
            }
        }
    }
    Ok(scan)
}
struct Files<'a>(&'a Candidate);
impl BlobSource for Files<'_> {
    fn read_verified(&self, d: &Descriptor, limit: u64) -> image_store::Result<Vec<u8>> {
        let payloads = self.0.config.payloads();
        let index = payloads
            .iter()
            .position(|(hash, _, _)| hash == &d.digest)
            .ok_or_else(|| image_store::Error::Invalid("missing source".into()))?;
        let path = self
            .0
            .sources
            .get(index)
            .ok_or_else(|| image_store::Error::Invalid("missing source".into()))?;
        use std::io::Read;
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(limit + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit
            || bytes.len() as u64 != d.size
            || Digest::of(&bytes) != d.digest
        {
            return Err(image_store::Error::Invalid(
                "ROM changed during import preview".into(),
            ));
        }
        Ok(bytes)
    }
}
fn descriptor(kind: &str, digest: Digest, size: u64) -> Descriptor {
    let mut d = Descriptor::new(kind, digest, size);
    d.annotations
        .insert("unemu.redistribution".into(), "user-supplied".into());
    d
}
/// Compute the immutable package identity without writing any store objects.
pub fn preview_manifest(candidate: &Candidate, attachments: &[Attachment]) -> Result<Manifest> {
    Ok(plan(candidate, attachments)?.0)
}

type PackagePlan = (Manifest, Vec<u8>, Vec<(PathBuf, Descriptor)>);

fn plan(candidate: &Candidate, attachments: &[Attachment]) -> Result<PackagePlan> {
    let mut config = candidate.config.clone();
    let mut payloads = Vec::new();
    let bindings = config.payloads();
    if bindings.len() != candidate.sources.len() {
        return Err(invalid("firmware source count differs from bindings"));
    }
    for ((digest, size, kind), path) in bindings.into_iter().zip(&candidate.sources) {
        let mut d = descriptor(kind, digest, size);
        if let assembly::FirmwareDistribution::Open {
            license,
            provenance,
            ..
        } = candidate.distribution
        {
            d.annotations
                .insert("unemu.redistribution".into(), "open".into());
            d.annotations.insert("unemu.license".into(), license.into());
            d.annotations
                .insert("unemu.provenance".into(), provenance.into());
        }
        payloads.push((path.clone(), d));
    }
    for extra in attachments {
        if let Some(name) = &extra.dos_name {
            crate::media_set::PcMediaFormats.validate_name(name)?;
        }
        let stat = fs::metadata(&extra.path)?;
        if !stat.is_file() {
            return Err(invalid("extra must be a regular file"));
        }
        let filename = extra
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| invalid("extra filename must be UTF-8"))?
            .to_owned();
        let digest: Digest = format!(
            "sha256:{}",
            assembly::firmware_discovery::hash_firmware_file(&extra.path)?
        )
        .parse()?;
        let extension = extra
            .path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let kind = match extension.as_str() {
            "txt" | "pdf" | "md" | "doc" => ExtraKind::Document,
            "zip" | "7z" | "arj" | "rar" => ExtraKind::Archive,
            "exe" | "com" | "bat" => ExtraKind::Executable,
            _ => ExtraKind::Other,
        };
        config.extras.push(Extra {
            blob: digest.clone(),
            filename,
            size: stat.len(),
            kind,
            dos_name: extra.dos_name.clone(),
        });
        payloads.push((
            extra.path.clone(),
            descriptor(mt::EXTRA, digest, stat.len()),
        ));
    }
    let bytes = config.encode()?;
    let mut layers = payloads.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    layers.retain(|d| seen.insert(d.digest.clone()));
    let mut manifest = Manifest::new_firmware(
        &config.platform,
        descriptor(mt::FIRMWARE_CONFIG, Digest::of(&bytes), bytes.len() as u64),
        layers,
    )?;
    manifest
        .annotations
        .insert("org.opencontainers.image.title".into(), config.name);
    Ok((manifest, bytes, payloads))
}

/// Check only a selected, unambiguous candidate. An identical existing package
/// is verified and reused; a different identity still conflicts before copying.
pub fn already_imported(
    store: &Store,
    tag: &str,
    manifest: &Manifest,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<bool> {
    let digest = Digest::of(&manifest.encode()?);
    match store.tag_descriptor(tag) {
        Ok(existing) if existing.digest == digest => {
            store.resolve_descriptor(&existing, &Validator(&PcFirmwareRules(catalogue)))?;
            Ok(true)
        }
        Ok(existing) => Err(image_store::Error::Conflict {
            current: Some(existing.digest),
        }
        .into()),
        Err(image_store::Error::TagNotFound(_)) => {
            store.check_new_tag(tag)?;
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}
/// Pre-check the target before copying, then verify the closure and publish by CAS.
pub fn publish(
    store: &Store,
    candidate: &Candidate,
    attachments: &[Attachment],
    catalogue: &[assembly::FirmwareEntry],
) -> Result<Descriptor> {
    let tag = candidate.tag();
    let (manifest, config, payloads) = plan(candidate, attachments)?;
    let encoded = manifest.encode()?;
    let expected = Descriptor::new(mt::MANIFEST, Digest::of(&encoded), encoded.len() as u64);
    if already_imported(store, &tag, &manifest, catalogue)? {
        return Ok(expected);
    }
    for (path, descriptor) in payloads {
        let blob = store.import_file(path, Some(&descriptor.digest))?;
        if blob.size != descriptor.size {
            return Err(invalid("source length changed during import"));
        }
    }
    store.import(config.as_slice(), Some(&manifest.config.digest))?;
    let d = store.import_manifest(&manifest)?;
    match store.update_tag(&tag, &d, None, &Validator(&PcFirmwareRules(catalogue))) {
        Ok(()) => (),
        Err(image_store::Error::Conflict {
            current: Some(current),
        }) if current == d.digest => {
            store.resolve_descriptor(&d, &Validator(&PcFirmwareRules(catalogue)))?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(d)
}
