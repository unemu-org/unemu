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
