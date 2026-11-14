// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC package rules shared by store resolution and the PC image provider.
//! IBM Personal Computer AT Technical Reference, Section 1, System Board ROM:
//! even and odd byte lanes retain socket identities; machine-images §5.3.

use assembly::{FirmwareEntry, FirmwareKind, Lane};
use image_store::{
    firmware::{Config, FirmwareRules, Rom},
    BlobSource, Descriptor, Digest, Error, Result,
};
use serde::{Deserialize, Serialize};

/// PC board identities, including reserved models which remain storage-only.
pub const BOARDS: &[&str] = &[
    "ibm-5170",
    "ibm-5150",
    "ibm-5160",
    "compaq-deskpro",
    "tandy-1000",
    "generic-386-isa",
    "i440fx-piix3",
    "i440bx-piix4",
];
/// Hardware families are provider vocabulary, independently of executable support.
pub const MACHINE_CLASSES: &[&str] = &["at", "xt"];
/// Executable boards whose machine class the provider can name.
const EXECUTABLE_BOARDS: &[assembly::BoardModel] = &[assembly::BoardModel::Ibm5170];
fn model_class(model: assembly::BoardModel) -> Option<&'static str> {
    match model.class() {
        Some(assembly::MachineClass::At) => Some("at"),
        None => None,
    }
}
fn board_class(board: &str) -> Option<&'static str> {
    EXECUTABLE_BOARDS
        .iter()
        .find(|model| model.id() == board)
        .and_then(|model| model_class(*model))
}

/// Provider-owned PC fit vocabulary, including reserved catalogue boards.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fit {
    #[serde(default)]
    pub boards: Vec<String>,
    #[serde(default)]
    pub machine_classes: Vec<String>,
}
impl Fit {
    pub fn decode(table: &toml::Table) -> Result<Self> {
        toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e: toml::de::Error| invalid(e.to_string()))
    }
    pub fn table(&self) -> Result<toml::Table> {
        toml::Table::try_from(self).map_err(|e| invalid(e.to_string()))
    }
}
fn invalid(s: impl Into<String>) -> Error {
    Error::Invalid(s.into())
}
pub fn role(kind: FirmwareKind) -> &'static str {
    match kind {
        FirmwareKind::SystemBios => "system-bios",
        FirmwareKind::OptionRom => "option-rom",
        FirmwareKind::VideoBios => "video-bios",
    }
}
pub fn lane(s: &str) -> Result<Lane> {
    match s {
        "even" => Ok(Lane::Even),
        "odd" => Ok(Lane::Odd),
        _ => Err(invalid("unknown PC chip lane")),
    }
}
/// Catalogue-backed rules used for package ingestion and final resolution.
pub struct PcFirmwareRules<'a>(pub &'a [FirmwareEntry]);
impl FirmwareRules for PcFirmwareRules<'_> {
    fn platform(&self) -> &str {
        "ibm-pc-at"
    }
    fn validate_name(&self, name: &str) -> Result<()> {
        if !crate::pc_names::valid_dos_name(name) {
            return Err(invalid(
                "expected canonical uppercase DOS 8.3 name (no reserved device names)",
            ));
        }
        Ok(())
    }
    fn validate(
        &self,
        config: &Config,
        blobs: &dyn BlobSource,
        descriptors: &[Descriptor],
    ) -> Result<()> {
        let fit = Fit::decode(&config.fits)?;
        let mut unique = std::collections::BTreeSet::new();
        for board in &fit.boards {
            if !unique.insert(board)
                || !(BOARDS.contains(&board.as_str())
                    || self.0.iter().any(|e| e.boards.contains(&board.as_str())))
            {
                return Err(invalid("unknown or duplicate PC fit board"));
            }
        }
        unique.clear();
        for class in &fit.machine_classes {
            if !MACHINE_CLASSES.contains(&class.as_str()) || !unique.insert(class) {
                return Err(invalid("unknown or duplicate PC machine class"));
            }
        }
        for rom in &config.roms {
            if !matches!(
                rom.role.as_str(),
                "system-bios" | "option-rom" | "video-bios" | "keyboard-controller" | "other"
            ) {
                return Err(invalid("unknown PC firmware role"));
            }
            if !rom.parts.is_empty() && rom.role != "system-bios" {
                return Err(invalid("only system-bios may use chip parts"));
            }
            if rom.size > 1024 * 1024 {
                return Err(invalid("PC ROM exceeds 1 MiB"));
            }
            let bytes = reconstructed(rom, blobs, descriptors)?;
            if bytes.len() as u64 != rom.size || Digest::of(&bytes).hex() != rom.sha256 {
                return Err(invalid("assembled firmware identity mismatch"));
            }
            if let Some(id) = &rom.catalogue_id {
                let entry = self
                    .0
                    .iter()
                    .find(|e| e.id == id)
                    .ok_or_else(|| invalid("unknown firmware catalogue id"))?;
                if role(entry.kind) != rom.role
                    || entry.sha256 != rom.sha256
                    || entry.size != rom.size
                    || fit
                        .boards
                        .iter()
                        .any(|b| !entry.boards.contains(&b.as_str()))
                    || fit.machine_classes.iter().any(|class| {
                        !entry
                            .boards
                            .iter()
                            .any(|board| board_class(board) == Some(class.as_str()))
                    })
                {
                    return Err(invalid("firmware catalogue identity, role or fit mismatch"));
                }
                if !rom.parts.is_empty()
                    && !entry.parts.iter().any(|layout| {
                        bijection(&rom.parts, layout, |part, known| {
                            known.sha256 == part.sha256
                                && known.size == part.size
                                && lane(&part.lane).ok() == Some(known.lane)
                                && known.offset == part.offset
                                && part.position.as_deref().is_none_or(|s| s == known.label)
                                && part
                                    .part_number
                                    .as_deref()
                                    .is_none_or(|s| s == known.part_number)
                        })
                        .is_some()
                    })
                {
                    return Err(invalid(
                        "firmware chips do not match a complete catalogue layout",
                    ));
                }
            }
        }
        Ok(())
    }
}
/// Reconstruct via the same lane/offset rules as machine-file parts.
pub fn reconstructed(
    rom: &Rom,
    blobs: &dyn BlobSource,
    descriptors: &[Descriptor],
) -> Result<Vec<u8>> {
    let read = |digest: &Digest| {
        let descriptor = descriptors
            .iter()
            .find(|d| &d.digest == digest)
            .ok_or_else(|| invalid("missing ROM descriptor"))?;
        blobs.read_verified(descriptor, 1024 * 1024)
    };
    if let Some(blob) = &rom.blob {
        return read(blob);
    }
    let total = rom.parts.iter().try_fold(0u64, |n, p| {
        n.checked_add(p.size)
            .ok_or_else(|| invalid("chip sizes overflow"))
    })?;
    if total != rom.size || total > 1024 * 1024 {
        return Err(invalid("chip sizes differ from bounded logical ROM size"));
    }
    let parts = rom
        .parts
        .iter()
        .map(|p| Ok((lane(&p.lane)?, p.offset, read(&p.blob)?)))
        .collect::<Result<Vec<_>>>()?;
    assembly::interleave_parts(&parts).map_err(|e| invalid(e.to_string()))
}

/// Resolve matching pins in stable package-digest order without visiting ROM roots.
/// A selected package is fully verified; corruption is an error, never a miss.
pub(crate) fn resolve_document(
    document: &mut crate::MachineFile,
    store: &image_store::Store,
    warnings: &mut Vec<String>,
) -> Result<Vec<Digest>> {
    use image_store::{
        firmware::Validator,
        summary::{ArtifactKind, SummaryCache},
    };
    let model = match document.board.as_ref().and_then(|b| b.model) {
        Some(crate::schema::v1::BoardModel::Ibm5170) => assembly::BoardModel::Ibm5170,
        None => return Ok(Vec::new()),
    };
    let board = model.id();
    let cache = (|| {
        let mut cache = SummaryCache::open(store)?;
        while !cache.pending().is_empty() {
            cache.rebuild(store, 64, &[], &[])?;
        }
        image_store::Result::Ok(cache)
    })();
    let cache = match cache {
        Ok(cache) => cache,
        Err(error) => {
            warnings.push(format!("firmware store index unavailable: {error}"));
            return Ok(Vec::new());
        }
    };
    for tag in cache.tags() {
        if let Some(failure) = cache.failure(&tag.manifest.digest) {
            warnings.push(format!(
                "firmware store summary unavailable: {}",
                failure.reason
            ));
        }
    }
    let mut tags = cache
        .tags()
        .iter()
        .filter(|tag| {
            cache
                .summary(&tag.manifest.digest)
                .is_some_and(|s| s.kind == ArtifactKind::Firmware && s.platform == "ibm-pc-at")
        })
        .cloned()
        .collect::<Vec<_>>();
    tags.sort_by(|a, b| a.manifest.digest.cmp(&b.manifest.digest));
    tags.dedup_by(|a, b| a.manifest.digest == b.manifest.digest);
    let catalogue = model.firmware_catalogue();
    let mut sources = Vec::new();
    let mut verified = std::collections::BTreeSet::new();
    if let Some(firmware) = &mut document.firmware {
        for (is_bios, rom) in firmware.bios.iter_mut().map(|r| (true, r)).chain(
            firmware
                .option_roms
                .iter_mut()
                .flatten()
                .map(|r| (false, r)),
        ) {
            if rom.image.is_some() && rom.parts.is_some()
                || rom.parts.as_ref().is_some_and(Vec::is_empty)
            {
                continue;
            }
            if rom.sha256.is_none()
                && rom
                    .parts
                    .as_ref()
                    .is_some_and(|parts| parts.iter().any(|p| p.sha256.is_none()))
            {
                continue;
            }
            let pins = rom
                .sha256
                .iter()
                .chain(rom.parts.iter().flatten().filter_map(|p| p.sha256.as_ref()))
                .map(|s| s.as_str())
                .collect::<Vec<_>>();
            if pins.is_empty() {
                continue;
            }
            for tag in &tags {
                let Some(facets) = cache
                    .summary(&tag.manifest.digest)
                    .and_then(|s| s.firmware.as_ref())
                else {
                    continue;
                };
                if !pins.iter().all(|p| facets.hashes.iter().any(|h| h == p)) {
                    continue;
                }
                let candidate = (|| {
                    let manifest = image_store::Manifest::decode(
                        &store.read_verified(&tag.manifest, image_store::MAX_SMALL_BLOB)?,
                    )?;
                    let config = Config::decode(
                        &store.read_verified(&manifest.config, image_store::MAX_SMALL_BLOB)?,
                    )?;
                    let fit = Fit::decode(&config.fits)?;
                    image_store::Result::Ok((config, fit))
                })();
                let (config, fit) = match candidate {
                    Ok(value) => value,
                    Err(error) => {
                        warnings.push(format!("firmware store candidate unavailable: {error}"));
                        continue;
                    }
                };
                if !fit.boards.is_empty() && !fit.boards.iter().any(|b| b == board)
                    || !fit.machine_classes.is_empty()
                        && !fit
                            .machine_classes
                            .iter()
                            .any(|class| Some(class.as_str()) == model_class(model))
                {
                    continue;
                }
                let found = config
                    .roms
                    .iter()
                    .find(|r| binding_matches(rom, r, is_bios));
                let Some(found) = found else {
                    continue;
                };
                if verified.insert(tag.manifest.digest.clone()) {
                    store.resolve_descriptor(
                        &tag.manifest,
                        &Validator(&PcFirmwareRules(catalogue)),
                    )?;
                }
                if let Some(blob) = &found.blob {
                    rom.image = Some(path_text(&store.blob_file_path(blob, found.size)?)?);
                } else if let Some(parts) = &mut rom.parts {
                    let indices = bijection(parts, &found.parts, part_matches)
                        .ok_or_else(|| invalid("package chip layout changed"))?;
                    for (part, index) in parts.iter_mut().zip(indices) {
                        let matching = &found.parts[index];
                        part.image =
                            path_text(&store.blob_file_path(&matching.blob, matching.size)?)?;
                    }
                }
                if !sources.contains(&tag.manifest.digest) {
                    sources.push(tag.manifest.digest.clone());
                }
                break;
            }
        }
    }
    Ok(sources)
}
fn path_text(path: &std::path::Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("store path is not Unicode"))
}
fn binding_matches(machine: &crate::schema::v1::Rom, package: &Rom, bios: bool) -> bool {
    if if bios {
        package.role != "system-bios"
    } else {
        !matches!(package.role.as_str(), "option-rom" | "video-bios")
    } {
        return false;
    }
    if machine
        .sha256
        .as_ref()
        .is_some_and(|p| p.as_str() != package.sha256)
    {
        return false;
    }
    match &machine.parts {
        Some(parts) => {
            bios && package.blob.is_none()
                && bijection(parts, &package.parts, part_matches).is_some()
        }
        None => package.blob.is_some(),
    }
}

/// One-to-one assignment of declared chips to package chips: each declared chip
/// takes a distinct package chip and none is left over. Returns, per declared
/// chip, the index of its package chip. Every caller's predicate requires the
/// same lane and offset, and a valid layout holds one chip per lane and offset,
/// so each declared chip has at most one candidate and first-fit is exact.
fn bijection<A, B>(
    declared: &[A],
    packaged: &[B],
    matches: impl Fn(&A, &B) -> bool,
) -> Option<Vec<usize>> {
    if declared.len() != packaged.len() {
        return None;
    }
    let mut used = vec![false; packaged.len()];
    let mut indices = Vec::with_capacity(declared.len());
    for part in declared {
        let index = packaged
            .iter()
            .enumerate()
            .position(|(i, candidate)| !used[i] && matches(part, candidate))?;
        used[index] = true;
        indices.push(index);
    }
    Some(indices)
}

fn part_matches(a: &crate::schema::v1::RomPart, b: &image_store::firmware::Part) -> bool {
    a.offset == b.offset
        && matches!(
            (a.lane, b.lane.as_str()),
            (crate::schema::v1::Lane::Even, "even") | (crate::schema::v1::Lane::Odd, "odd")
        )
        && a.sha256.as_ref().is_none_or(|s| s.as_str() == b.sha256)
}
