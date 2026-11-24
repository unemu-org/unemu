// SPDX-License-Identifier: AGPL-3.0-or-later
//! PC package interpretation for core-neutral template preparation.

use image_store::{firmware, media_set, Digest, ResolvedImage, Store};
use recipe::template::{UsePlan, VerifiedUse};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
};
use unemu_core_api::{
    BindingItem, BindingKind, BindingUse, Content, ContentReader, ContentSource, Error, ErrorKind,
};

/// Resolve using the package's own ROM declarations, checked against this
/// catalogue. A synthetic catalogue can exercise the same boundary in tests.
pub fn firmware_use(
    store: &Store,
    usage: &UsePlan,
    catalogue: &[assembly::FirmwareEntry],
) -> Result<VerifiedUse, String> {
    let rules = crate::firmware_package::PcFirmwareRules(catalogue);
    let image = resolve(store, usage, &firmware::Validator(&rules))?;
    let config = firmware::Config::decode(&image.config).map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    let mut assembled = BTreeMap::new();
    for rom in &config.roms {
        let bytes = crate::firmware_package::reconstructed(rom, store, &image.manifest.layers)
            .map_err(|e| e.to_string())?;
        let digest = Digest::of(&bytes);
        if digest.hex() != rom.sha256 || bytes.len() as u64 != rom.size {
            return Err("firmware content identity mismatch".into());
        }
        let hash = crate::provider::hash_array(&digest).map_err(|e| e.to_string())?;
        items.push(
            BindingItem::new(&rom.id, BindingKind::FirmwareRom, rom.size, hash)
                .with_roles(vec![rom.role.clone()]),
        );
        if rom.blob.is_none() {
            assembled.insert((hash, rom.size), bytes);
        }
    }
    finish(usage, image, items, assembled)
}

pub fn media_use(store: &Store, usage: &UsePlan) -> Result<VerifiedUse, String> {
    let image = resolve(
        store,
        usage,
        &media_set::Validator(&crate::media_set::PcMediaFormats),
    )?;
    let config = media_set::Config::decode(&image.config).map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for medium in config.media {
        let descriptor = image
            .manifest
            .layers
            .iter()
            .find(|d| d.names(&medium.blob))
            .ok_or("missing medium payload")?;
        items.push(BindingItem::new(
            medium.id,
            BindingKind::Medium,
            descriptor.decoded_size().map_err(|e| e.to_string())?,
            crate::provider::hash_array(&medium.blob).map_err(|e| e.to_string())?,
        ));
    }
    finish(usage, image, items, BTreeMap::new())
}

fn resolve(
    store: &Store,
    usage: &UsePlan,
    validator: &dyn image_store::ConfigValidator,
) -> Result<ResolvedImage, String> {
    let digest: Digest = usage
        .digest
        .parse()
        .map_err(|e: image_store::Error| e.to_string())?;
    let descriptor = recipe::template::descriptor(store, &digest)
        .map_err(|e| e.to_string())?
        .ok_or("missing package manifest")?;
    let bytes = store
        .read_compatible_manifest(&descriptor)
        .map_err(|e| e.to_string())?;
    let manifest = image_store::Manifest::decode_for_summary(&bytes).map_err(|e| e.to_string())?;
    if manifest.artifact_type() != usage.artifact_type()?
        || manifest.artifact_type() != validator.artifact_type()
        || !image_store::platform::same(
            manifest.platform().map_err(|e| e.to_string())?,
            crate::PLATFORM,
        )
    {
        return Err(format!("use {}: wrong artifact type or platform", usage.id));
    }
    store
        .resolve_descriptor(&descriptor, validator)
        .map_err(|e| format!("use {}: {e}", usage.id))
}

fn finish(
    usage: &UsePlan,
    image: ResolvedImage,
    mut items: Vec<BindingItem>,
    assembled: BTreeMap<([u8; 32], u64), Vec<u8>>,
) -> Result<VerifiedUse, String> {
    // Every declared item has a full receipt, even when the core later selects
    // only one member. Resolution already streams and hashes the closure.
    items.sort_by(|a, b| a.id.cmp(&b.id));
    let allowed = items
        .iter()
        .map(|i| {
            let hex: String = i.sha256.iter().map(|b| format!("{b:02x}")).collect();
            Ok((
                (i.sha256, i.size),
                format!("sha256:{hex}")
                    .parse()
                    .map_err(|e: image_store::Error| e.to_string())?,
            ))
        })
        .collect::<Result<_, String>>()?;
    Ok(VerifiedUse {
        facts: BindingUse::new(&usage.id, &usage.digest, items),
        content: Box::new(Source {
            image,
            allowed,
            assembled,
        }),
    })
}

struct Source {
    image: ResolvedImage,
    allowed: BTreeMap<([u8; 32], u64), Digest>,
    assembled: BTreeMap<([u8; 32], u64), Vec<u8>>,
}
fn content_error(error: impl ToString) -> Error {
    Error::new(ErrorKind::Content, error.to_string())
}
fn stored_error(error: image_store::Error) -> Error {
    let kind = match error {
        image_store::Error::Io(_) => ErrorKind::Storage,
        _ => ErrorKind::Content,
    };
    Error::new(kind, error.to_string())
}
impl ContentSource for Source {
    fn open(&self, sha256: &[u8; 32], size: u64) -> Result<Content, Error> {
        let digest = self
            .allowed
            .get(&(*sha256, size))
            .ok_or_else(|| content_error("content is not a declared package item"))?;
        if let Some(bytes) = self.assembled.get(&(*sha256, size)) {
            return Ok(Content::Bytes(bytes.clone()));
        }
        let (file, _) = self.image.verified_file(digest).map_err(stored_error)?;
        Ok(Content::Reader(Box::new(Reader {
            file: RefCell::new(file),
            image: self.image.clone(),
            digest: digest.clone(),
            size,
        })))
    }
}
struct Reader {
    file: RefCell<std::fs::File>,
    image: ResolvedImage,
    digest: Digest,
    size: u64,
}
impl ContentReader for Reader {
    fn size(&self) -> u64 {
        self.size
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        if offset
            .checked_add(buf.len() as u64)
            .is_none_or(|end| end > self.size)
        {
            return Err(Error::new(
                ErrorKind::Usage,
                "content read is outside the package item",
            ));
        }
        self.image
            .verified_file(&self.digest)
            .map_err(stored_error)?;
        let mut file = self.file.borrow_mut();
        file.seek(SeekFrom::Start(offset))
            .and_then(|_| file.read_exact(buf))
            .map_err(|e| Error::new(ErrorKind::Storage, e.to_string()))
    }
}

/// Package a composed document without rebuilding or re-encoding its config.
pub fn publish(
    store: &Store,
    recipe: &recipe::model::Recipe,
    prepared: &unemu_core_api::PreparedConfig,
    slots: &[unemu_core_api::BindingSlot],
    facts: &recipe::template::Facts,
) -> crate::Result<recipe::build::Published> {
    use image_store::{license, media_type as mt, ConfigValidator, Descriptor, Manifest};
    use unemu_core_api::ContentReader;
    let generated = |descriptor| -> crate::Result<Descriptor> {
        let mut descriptor = crate::generated(descriptor);
        if let Some(expression) = recipe.license.as_ref().and_then(|l| l.expression.as_ref()) {
            license::mark(
                &mut descriptor,
                Some(&license::Expression::parse(&expression.value)?),
            );
        }
        Ok(descriptor)
    };
    let mut layers = BTreeMap::<Digest, Descriptor>::new();
    let mut add = |descriptor: Descriptor| -> crate::Result<()> {
        if let Some(previous) = layers.insert(descriptor.digest.clone(), descriptor.clone()) {
            if previous != descriptor {
                return Err(crate::invalid(
                    "conflicting descriptors for template content",
                ));
            }
        }
        Ok(())
    };
    let mut provenance = BTreeMap::new();
    let mut firmware_sources = std::collections::BTreeSet::new();
    for usage in &recipe.uses {
        let usage = UsePlan {
            id: usage.id.value.clone(),
            image: usage
                .image
                .as_ref()
                .map(|v| v.value.clone())
                .unwrap_or_default(),
            digest: usage
                .digest
                .as_ref()
                .map(|v| v.value.clone())
                .unwrap_or_default(),
            present: true,
        };
        let rules = crate::firmware_package::PcFirmwareRules(crate::firmware::catalogue());
        let image = if usage
            .artifact_type()
            .map_err(|e| crate::invalid(e.to_string()))?
            == mt::FIRMWARE
        {
            resolve(store, &usage, &firmware::Validator(&rules))
        } else {
            resolve(
                store,
                &usage,
                &media_set::Validator(&crate::media_set::PcMediaFormats),
            )
        }
        .map_err(|e| crate::invalid(e.to_string()))?;
        provenance.insert(
            usage.id.clone(),
            (usage.image.clone(), usage.digest.clone()),
        );
        let package = if image.manifest.artifact_type() == mt::FIRMWARE {
            firmware_sources.insert(usage.digest.clone());
            Some(firmware::Config::decode(&image.config)?)
        } else {
            None
        };
        for slot in slots
            .iter()
            .filter(|s| s.use_id.as_deref() == Some(&usage.id))
        {
            let item = facts
                .uses
                .iter()
                .find(|u| u.id == usage.id)
                .and_then(|u| {
                    u.items
                        .iter()
                        .find(|i| Some(&i.id) == slot.item_id.as_ref())
                })
                .ok_or_else(|| crate::invalid("missing selected package item"))?;
            let digest = pc_config::image::Digest::from_sha256(item.sha256);
            let digest = crate::StoreDigest::store(&digest)?;
            if let Some(descriptor) = image.manifest.layers.iter().find(|d| d.names(&digest)) {
                add(descriptor.clone())?;
            } else {
                let content = facts
                    .open(&item.sha256, item.size)
                    .map_err(|e| crate::invalid(e.to_string()))?;
                let mut bytes = vec![0; item.size as usize];
                content
                    .read_at(0, &mut bytes)
                    .map_err(|e| crate::invalid(e.to_string()))?;
                let blob = store.import(bytes.as_slice(), Some(&digest))?;
                let rom = package
                    .as_ref()
                    .and_then(|p| p.roms.iter().find(|r| r.id == item.id))
                    .ok_or_else(|| crate::invalid("missing assembled ROM declaration"))?;
                let mut descriptor = Descriptor::new(mt::FIRMWARE, blob.digest, blob.size);
                let first = rom
                    .parts
                    .first()
                    .and_then(|part| image.manifest.layers.iter().find(|d| d.names(&part.blob)))
                    .ok_or_else(|| crate::invalid("missing ROM chip rights"))?;
                descriptor.annotations = first.annotations.clone();
                // Chip-specific identity annotations do not describe the assembled ROM.
                descriptor
                    .annotations
                    .retain(|k, _| !k.starts_with("unemu.decoded."));
                add(descriptor)?;
            }
            if let Some(package) = &package {
                for source in package.sources.iter().filter(|s| s.roms.contains(&item.id)) {
                    let descriptor = image
                        .manifest
                        .layers
                        .iter()
                        .find(|d| d.names(&source.blob))
                        .ok_or_else(|| crate::invalid("missing corresponding source archive"))?;
                    add(descriptor.clone())?;
                }
                for extra in &package.extras {
                    let descriptor = image
                        .manifest
                        .layers
                        .iter()
                        .find(|d| d.names(&extra.blob))
                        .ok_or_else(|| crate::invalid("missing firmware rights text"))?;
                    add(descriptor.clone())?;
                }
                if let Some(readme) = &package.readme {
                    let descriptor = image
                        .manifest
                        .layers
                        .iter()
                        .find(|d| d.names(&readme.blob))
                        .ok_or_else(|| crate::invalid("missing firmware provenance text"))?;
                    add(descriptor.clone())?;
                }
            }
            // Imported rights links stay package-local, including media terms.
            let mut pending = vec![digest];
            let mut seen = std::collections::BTreeSet::new();
            while let Some(digest) = pending.pop() {
                if !seen.insert(digest.clone()) {
                    continue;
                }
                if let Some(descriptor) = image.manifest.layers.iter().find(|d| d.names(&digest)) {
                    for key in [license::SOURCE, license::TERMS] {
                        if let Some(link) = descriptor.annotations.get(key) {
                            let digest: Digest = link.parse()?;
                            let linked = image
                                .manifest
                                .layers
                                .iter()
                                .find(|d| d.names(&digest))
                                .ok_or_else(|| crate::invalid("missing package rights link"))?;
                            add(linked.clone())?;
                            pending.push(digest);
                        }
                    }
                }
            }
        }
    }
    let mut metadata = image_store::metadata::Metadata::new(crate::PLATFORM);
    if let Some(template) = &recipe.template {
        metadata.labels = template.labels.iter().map(|v| v.value.clone()).collect();
    }
    let bytes = metadata.encode(&[])?;
    let blob = store.import(bytes.as_slice(), None)?;
    add(generated(Descriptor::new(
        mt::METADATA,
        blob.digest,
        blob.size,
    ))?)?;
    for content in &prepared.validated.content {
        let digest =
            crate::StoreDigest::store(&pc_config::image::Digest::from_sha256(content.sha256))?;
        if !layers
            .values()
            .any(|d| d.names(&digest) && d.decoded_size().ok() == Some(content.size))
        {
            return Err(crate::invalid(
                "prepared content is absent from the template closure",
            ));
        }
    }
    let config = &prepared.config;
    let blob = store.import(config.bytes.as_slice(), None)?;
    let descriptor = generated(Descriptor::new(config.media_type, blob.digest, blob.size))?;
    let mut manifest = Manifest::new_with_types(
        crate::PLATFORM,
        descriptor,
        layers.into_values().collect(),
        crate::PAYLOAD_TYPES,
    )?;
    if let Some(template) = &recipe.template {
        for (key, value) in [
            ("org.opencontainers.image.title", &template.title),
            (
                "org.opencontainers.image.description",
                &template.description,
            ),
        ] {
            if let Some(value) = value {
                manifest.annotations.insert(key.into(), value.value.clone());
            }
        }
    }
    manifest.annotations.insert(
        "unemu.firmware.sources".into(),
        format!(
            "[{}]",
            firmware_sources
                .into_iter()
                .map(|d| format!("\"{d}\""))
                .collect::<Vec<_>>()
                .join(",")
        ),
    );
    manifest.annotations.insert(
        "unemu.template.uses".into(),
        toml::to_string(&provenance).map_err(|e| crate::invalid(e.to_string()))?,
    );
    let reference = format!(
        "{}:{}",
        recipe
            .identity
            .name
            .as_ref()
            .map(|v| v.value.as_str())
            .unwrap_or_default(),
        recipe
            .identity
            .version
            .as_ref()
            .map(|v| v.value.as_str())
            .unwrap_or_default()
    );
    let descriptor =
        store.import_manifest_with_types(&manifest, crate::PcProvider.payload_types())?;
    let created = match store.tag_descriptor(&reference) {
        Ok(existing) if existing.digest == descriptor.digest => {
            store.resolve_descriptor(&existing, &crate::PcProvider)?;
            false
        }
        Ok(_) => {
            return Err(crate::invalid(
                "template version tag already names different content",
            ))
        }
        Err(image_store::Error::TagNotFound(_)) => {
            store.update_tag(&reference, &descriptor, None, &crate::PcProvider)?;
            true
        }
        Err(error) => return Err(error.into()),
    };
    Ok(recipe::build::Published {
        descriptor,
        created,
    })
}
