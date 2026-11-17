// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local-only catalogue overlays and personal data; machine-images §§2.1, 3–3.1.
//! Read/modify/write transactions reread under the index lock. Invalid or newer
//! library files fail closed and remain untouched rather than losing personal data.

use crate::{
    filesystem as host, invalid, media_type, metadata::Metadata, store::read_bounded,
    ConfigValidator, Descriptor, Digest, Error, Result, Store, MAX_SMALL_BLOB,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
};

/// Sparse field edits for an existing stable title id. JSON strings in TOML allow
/// explicit null (clear an optional field) and opaque provider launch objects.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TitleEdit {
    fields: BTreeMap<String, String>,
}

impl TitleEdit {
    pub fn set(&mut self, field: &str, value: Value) -> Result<()> {
        if !matches!(
            field,
            "title"
                | "sort_title"
                | "series"
                | "genres"
                | "developers"
                | "publishers"
                | "platform"
                | "release_year"
                | "players"
                | "languages"
                | "regions"
                | "description"
                | "version"
                | "edition"
                | "source_media"
                | "tags"
                | "cover"
                | "screenshots"
                | "launch"
        ) {
            return Err(invalid("field cannot be a catalogue edit"));
        }
        let mut probe = serde_json::json!({"id":"probe", "title":"Probe"});
        if !value.is_null() {
            probe[field] = value.clone();
        } else if field == "title" {
            return Err(invalid("title cannot be cleared"));
        }
        let title: crate::metadata::Title =
            serde_json::from_value(probe).map_err(|e| invalid(e.to_string()))?;
        title.validate()?;
        self.fields.insert(
            field.into(),
            serde_json::to_string(&value).map_err(|e| invalid(e.to_string()))?,
        );
        Ok(())
    }

    fn apply(&self, title: &mut crate::metadata::Title) -> Result<()> {
        let mut value = serde_json::to_value(&*title).map_err(|e| invalid(e.to_string()))?;
        for (field, json) in &self.fields {
            let edit: Value = serde_json::from_str(json).map_err(|e| invalid(e.to_string()))?;
            let mut checked = Self::default();
            checked.set(field, edit.clone())?;
            if edit.is_null() {
                if let Some(object) = value.as_object_mut() {
                    object.remove(field);
                }
            } else {
                value[field] = edit;
            }
        }
        *title = serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
        title.validate()
    }
}

/// Edits destined for an explicit new image publication, never automatic pushes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogueEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub titles: BTreeMap<String, TitleEdit>,
    #[serde(default)]
    pub artwork: Vec<Descriptor>,
}

impl CatalogueEdit {
    pub fn apply(&self, metadata: &Metadata) -> Result<Metadata> {
        if metadata.is_read_only() {
            return Err(invalid("newer metadata is read-only"));
        }
        let mut result = metadata.clone();
        if let Some(labels) = &self.labels {
            result.labels = labels.clone();
        }
        for (id, edit) in &self.titles {
            if let Some(title) = result.titles.iter_mut().find(|t| &t.id == id) {
                edit.apply(title)?;
            } else {
                result.warn(format!(
                    "overlay title id absent from source metadata: {id}"
                ));
            }
        }
        Ok(result)
    }

    fn has_fields(&self) -> bool {
        self.labels.is_some() || self.titles.values().any(|edit| !edit.fields.is_empty())
    }

    pub fn is_empty(&self) -> bool {
        !self.has_fields() && self.artwork.is_empty()
    }

    fn validate(&self) -> Result<()> {
        for (id, edit) in &self.titles {
            if id.trim().is_empty() {
                return Err(invalid("overlay title id must be nonempty"));
            }
            for (field, json) in &edit.fields {
                TitleEdit::default().set(
                    field,
                    serde_json::from_str(json).map_err(|e| invalid(e.to_string()))?,
                )?;
            }
        }
        for descriptor in &self.artwork {
            if !matches!(
                descriptor.media_type.as_str(),
                media_type::PNG | media_type::JPEG
            ) {
                return Err(invalid("overlay artwork must be PNG or JPEG"));
            }
            if descriptor.urls.is_some() || descriptor.data.is_some() {
                return Err(invalid(
                    "overlay artwork cannot contain urls or embedded data",
                ));
            }
            descriptor.validate()?;
        }
        Ok(())
    }
}

/// Personal data is structurally separate from publishable catalogue edits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Personal {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_rating: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// One source-image entry; title maps use stable ids from its metadata blob.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryEntry {
    #[serde(default)]
    pub catalogue: CatalogueEdit,
    #[serde(default)]
    pub personal: BTreeMap<String, Personal>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    #[serde(default)]
    pub overlay_revision: u64,
    #[serde(default)]
    pub personal_revision: u64,
    #[serde(default)]
    pub local_revision: u64,
}

impl LibraryEntry {
    /// Content identity remains valid across restored files and reused counters.
    pub fn content_digest(&self) -> Result<Digest> {
        let bytes = serde_json::to_vec(&(
            &self.catalogue,
            &self.personal,
            &self.labels,
            self.favorite,
            &self.last_run,
        ))
        .map_err(|e| invalid(e.to_string()))?;
        Ok(Digest::of(&bytes))
    }

    pub fn is_empty(&self) -> bool {
        self.catalogue.is_empty()
            && self.personal.is_empty()
            && self.labels.is_empty()
            && !self.favorite
            && self.last_run.is_none()
    }

    fn normalize(&mut self) {
        self.catalogue
            .titles
            .retain(|_, edit| !edit.fields.is_empty());
        self.personal
            .retain(|_, personal| personal != &Personal::default());
    }

    fn revise(&mut self, old: &Self) -> Result<()> {
        fn revision(old: u64, changed: bool) -> Result<u64> {
            old.checked_add(u64::from(changed))
                .ok_or_else(|| invalid("library revision exhausted"))
        }
        self.overlay_revision = revision(old.overlay_revision, old.catalogue != self.catalogue)?;
        self.personal_revision = revision(old.personal_revision, old.personal != self.personal)?;
        self.local_revision = revision(
            old.local_revision,
            old.labels != self.labels
                || old.favorite != self.favorite
                || old.last_run != self.last_run,
        )?;
        Ok(())
    }
}

/// Versioned library file. A missing file is empty; corrupt files return errors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Library {
    schema: u32,
    #[serde(default)]
    pub images: BTreeMap<Digest, LibraryEntry>,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            schema: 1,
            images: BTreeMap::new(),
        }
    }
}

impl Library {
    /// Overlay sources and artwork are roots for a future mark/sweep collector.
    pub fn gc_roots(&self) -> Result<BTreeSet<Digest>> {
        let mut roots = BTreeSet::new();
        for (image, entry) in &self.images {
            if entry.catalogue.is_empty() {
                continue;
            }
            roots.insert(image.clone());
            roots.extend(entry.catalogue.artwork.iter().map(|d| d.digest.clone()));
            for edit in entry.catalogue.titles.values() {
                for (field, json) in &edit.fields {
                    let value: Value =
                        serde_json::from_str(json).map_err(|e| invalid(e.to_string()))?;
                    if field == "cover" {
                        if let Some(s) = value.as_str() {
                            roots.insert(s.parse()?);
                        }
                    } else if field == "screenshots" && !value.is_null() {
                        let digests: Vec<Digest> =
                            serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
                        roots.extend(digests);
                    }
                }
            }
        }
        Ok(roots)
    }

    fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            return Err(invalid("unsupported local library schema"));
        }
        for entry in self.images.values() {
            entry.catalogue.validate()?;
            if entry.personal.keys().any(|id| id.trim().is_empty()) {
                return Err(invalid("personal title id must be nonempty"));
            }
            if entry
                .personal
                .values()
                .any(|p| p.user_rating.is_some_and(|r| r > 5))
            {
                return Err(invalid("user rating must be 0..=5"));
            }
        }
        Ok(())
    }
}

impl Store {
    pub fn read_library(&self) -> Result<Library> {
        let _lease = self.lease()?;
        self.read_library_inner()
    }

    pub(crate) fn read_library_inner(&self) -> Result<Library> {
        let bytes = match read_bounded(&self.root().join("local/library.toml"), 64 * MAX_SMALL_BLOB)
        {
            Ok(bytes) => bytes,
            Err(crate::Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Library::default())
            }
            Err(error) => return Err(error),
        };
        let library: Library =
            toml::from_str(std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?)
                .map_err(|e| invalid(e.to_string()))?;
        library.validate()?;
        Ok(library)
    }

    /// Run user code without the index lock, then compare its input with current
    /// data under the lock. Re-entrant store calls are safe; concurrent edits return
    /// LibraryConflict and the caller may retry its closure against fresh data.
    pub fn edit_library(
        &self,
        image: &Digest,
        edit: impl FnOnce(&mut LibraryEntry) -> Result<()>,
    ) -> Result<()> {
        let _lease = self.lease()?;
        let old = self
            .read_library_inner()?
            .images
            .get(image)
            .cloned()
            .unwrap_or_default();
        let mut candidate = old.clone();
        edit(&mut candidate)?;
        candidate.normalize();
        candidate.catalogue.validate()?;
        candidate.catalogue.artwork = self.clean_artwork(&candidate.catalogue.artwork)?;
        let _lock = host::exclusive_lock(&self.root().join("locks/index.lock"))?;
        let mut library = self.read_library_inner()?;
        let latest = library.images.get(image).cloned().unwrap_or_default();
        if latest.content_digest()? != old.content_digest()? {
            return Err(Error::LibraryConflict);
        }
        if candidate.content_digest()? == old.content_digest()? {
            return Ok(());
        }
        candidate.revise(&latest)?;
        if candidate.is_empty() {
            library.images.remove(image);
        } else {
            library.images.insert(image.clone(), candidate);
        }
        self.write_library_locked(&mut library)
    }

    fn write_library_locked(&self, library: &mut Library) -> Result<()> {
        for entry in library.images.values_mut() {
            entry.normalize();
        }
        library.images.retain(|_, entry| !entry.is_empty());
        library.validate()?;
        let bytes = toml::to_string(library).map_err(|e| invalid(e.to_string()))?;
        if bytes.len() as u64 > 64 * MAX_SMALL_BLOB {
            return Err(invalid("library exceeds 64 MiB"));
        }
        fs::create_dir_all(self.root().join("local"))?;
        self.atomic_write(&self.root().join("local/library.toml"), bytes.as_bytes())?;
        host::sync_dir(self.root())
    }

    fn clean_artwork(&self, artwork: &[Descriptor]) -> Result<Vec<Descriptor>> {
        artwork
            .iter()
            .map(|descriptor| {
                let edit = CatalogueEdit {
                    artwork: vec![descriptor.clone()],
                    ..CatalogueEdit::default()
                };
                edit.validate()?;
                self.verify(&descriptor.digest, descriptor.size)?;
                Ok(Descriptor::new(
                    &descriptor.media_type,
                    descriptor.digest.clone(),
                    descriptor.size,
                ))
            })
            .collect()
    }

    /// Verify the source closure once, then only new metadata/artwork. Immutable
    /// source payloads stay covered by that verification and the retained lease.
    /// On success consume the catalogue edits and move local data by stable id.
    /// An I/O error after tag publication can leave the tag visible; original local
    /// data remains intact if its atomic replacement fails.
    pub fn publish_overlay(
        &self,
        source: &Descriptor,
        new_tag: &str,
        validator: &dyn ConfigValidator,
    ) -> Result<Descriptor> {
        let _lease = self.lease()?;
        let entry = self
            .read_library()?
            .images
            .get(&source.digest)
            .cloned()
            .unwrap_or_default();
        let resolved = self.resolve_image_inner(source, validator, false)?;
        let source_metadata = Metadata::decode(&resolved.metadata, &resolved.manifest.layers)?;
        if source_metadata.is_read_only() {
            return Err(invalid("newer metadata is read-only"));
        }
        if source_metadata.platform != resolved.manifest.platform()? {
            return Err(invalid("metadata platform mismatch"));
        }
        let mut manifest = resolved.manifest;
        let metadata = if entry.catalogue.has_fields() {
            entry.catalogue.apply(&source_metadata)?
        } else {
            source_metadata
        };
        if !metadata.warnings().is_empty() {
            return Err(invalid(metadata.warnings().join("; ")));
        }
        let image = if !entry.catalogue.has_fields() {
            if let Some(missing) = resolved.missing_artwork.first() {
                return Err(Error::MissingBlob {
                    digest: missing.digest.clone(),
                    media_type: missing.media_type.clone(),
                });
            }
            source.clone()
        } else {
            let mut available = self.clean_artwork(&entry.catalogue.artwork)?;
            available.extend(manifest.layers.clone());
            let bytes = metadata.encode(&available)?;
            let blob = self.import(bytes.as_slice(), None)?;
            let old = manifest
                .layers
                .iter()
                .find(|d| d.media_type == media_type::METADATA)
                .ok_or_else(|| invalid("missing metadata"))?;
            let descriptor = if blob.digest == old.digest {
                old.clone()
            } else {
                Descriptor::new(media_type::METADATA, blob.digest, blob.size)
            };
            manifest.layers.retain(|d| {
                !matches!(
                    d.media_type.as_str(),
                    media_type::METADATA | media_type::PNG | media_type::JPEG
                )
            });
            manifest.layers.push(descriptor);
            let mut seen = BTreeSet::new();
            for digest in metadata.artwork() {
                if seen.insert(digest.clone()) {
                    if resolved.missing_artwork.iter().any(|d| &d.digest == digest)
                        && !entry.catalogue.artwork.iter().any(|d| &d.digest == digest)
                    {
                        return Err(invalid("required artwork blob missing"));
                    }
                    manifest.layers.push(
                        available
                            .iter()
                            .find(|d| &d.digest == digest)
                            .ok_or_else(|| invalid("overlay artwork descriptor missing"))?
                            .clone(),
                    );
                }
            }
            manifest.annotations.insert(
                "org.opencontainers.image.base.digest".into(),
                source.digest.to_string(),
            );
            self.import_manifest_with_types(&manifest, validator.payload_types())?
        };
        let _lock = host::exclusive_lock(&self.root().join("locks/index.lock"))?;
        let mut library = self.read_library_inner()?;
        let current = library
            .images
            .get(&source.digest)
            .cloned()
            .unwrap_or_default();
        if current.catalogue != entry.catalogue {
            return Err(Error::LibraryConflict);
        }
        if image.digest != source.digest {
            let mut from = current.clone();
            from.catalogue = CatalogueEdit {
                artwork: from
                    .catalogue
                    .artwork
                    .into_iter()
                    .filter(|d| !metadata.artwork().any(|digest| &d.digest == digest))
                    .collect(),
                ..CatalogueEdit::default()
            };
            let to = library.images.entry(image.digest.clone()).or_default();
            let previous_to = to.clone();
            for (id, personal) in std::mem::take(&mut from.personal) {
                if metadata.titles.iter().any(|t| t.id == id) {
                    if to.personal.get(&id).is_some_and(|p| p != &personal) {
                        return Err(Error::LibraryConflict);
                    }
                    to.personal.insert(id, personal);
                } else {
                    from.personal.insert(id, personal);
                }
            }
            to.labels.extend(std::mem::take(&mut from.labels));
            to.labels.sort();
            to.labels.dedup();
            to.favorite |= std::mem::take(&mut from.favorite);
            to.revise(&previous_to)?;
            from.revise(&current)?;
            library.images.insert(source.digest.clone(), from);
        }
        library.validate()?;
        self.update_tag_locked(new_tag, &image, None, None)?;
        if image.digest != source.digest {
            self.write_library_locked(&mut library)?;
        }
        Ok(image)
    }
}
