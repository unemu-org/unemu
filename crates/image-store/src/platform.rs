// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine platform ids and their read-only aliases; machine-images §1
//! (Platform neutrality) and §2.
//!
//! A platform id names a machine family and routes an artifact to the
//! emulator that provides it. When a family's id is renamed, the old id stays
//! an alias for good: published artifacts and the stores of earlier releases
//! keep their bytes, so every reader accepts both ids as one platform, while
//! every writer writes the new id. Platform ids are compared through [`same`]
//! only; an exact string comparison of two ids is a bug.
//!
//! The table lives in this shared crate, not with a provider, so that every
//! comparison agrees, also where no provider is at hand: the summary cache
//! and catalogue edits compare a manifest with its config and metadata, and
//! the registry client chooses a provider by platform. It holds identifiers
//! only, never emulator behaviour.

/// Retired platform ids, each with the id that replaced it. Readers accept
/// both; writers never write the retired one.
pub const ALIASES: &[(&str, &str)] = &[
    // The IBM PC compatible family: the PC and XT, the AT, the PS/2 models
    // and the later 386, 486 and Pentium machines. Its first id named the AT
    // alone.
    ("ibm-pc-at", "ibm-pc"),
];

/// The id a platform is written with: the replacement of a retired id, any
/// other id unchanged.
pub fn canonical(id: &str) -> &str {
    ALIASES
        .iter()
        .find(|(retired, _)| *retired == id)
        .map_or(id, |(_, current)| current)
}

/// Whether two platform ids name the same platform.
pub fn same(a: &str, b: &str) -> bool {
    canonical(a) == canonical(b)
}

fn retired(id: &str) -> bool {
    ALIASES.iter().any(|(old, _)| *old == id)
}

/// The metadata layer a writer carries over from the artifact it derives
/// from: `metadata` itself when its envelope names the platform by its
/// current id, otherwise a stored copy that does, with every other field
/// kept. Metadata of a newer schema is read-only and carried over as it is,
/// and so is metadata this release cannot decode, which the writer's own
/// validation judges. `layers` are the source manifest's, which hold the
/// artwork the metadata names.
pub fn current_metadata(
    store: &crate::Store,
    metadata: &crate::Descriptor,
    layers: &[crate::Descriptor],
) -> crate::Result<crate::Descriptor> {
    use crate::BlobSource;
    let bytes = store.read_verified(metadata, crate::MAX_SMALL_BLOB)?;
    let Ok(mut decoded) = crate::metadata::Metadata::decode(&bytes, layers) else {
        return Ok(metadata.clone());
    };
    if !retired(&decoded.platform) || decoded.is_read_only() {
        return Ok(metadata.clone());
    }
    decoded.platform = canonical(&decoded.platform).to_owned();
    let blob = store.import(decoded.encode(layers)?.as_slice(), None)?;
    let mut copy = metadata.clone();
    copy.digest = blob.digest;
    copy.size = blob.size;
    Ok(copy)
}

/// Test support: store a copy of the artifact `image` that names the platform
/// `id`, and return the copy's descriptor. The manifest annotation, the
/// config `platform` line and the metadata envelope name `id`, and the
/// `unemu.min-app-version` marker is left out; everything else keeps its
/// bytes. With a retired id this is the artifact as a release before the
/// rename wrote it, provided it needs nothing newer than the id it names now;
/// with an unknown id, one that no provider reads. `types` are the payload
/// types of `image`'s provider.
#[cfg(any(test, feature = "test-support"))]
pub fn with_platform_id(
    store: &crate::Store,
    image: &crate::Descriptor,
    types: &[crate::PayloadType],
    id: &str,
) -> crate::Result<crate::Descriptor> {
    use crate::{invalid, BlobSource, MAX_SMALL_BLOB};
    let restored = |descriptor: &crate::Descriptor, bytes: &[u8]| {
        let blob = store.import(bytes, None)?;
        let mut copy = descriptor.clone();
        copy.digest = blob.digest;
        copy.size = blob.size;
        crate::Result::Ok(copy)
    };
    let mut manifest =
        crate::Manifest::decode_with_types(&store.read_verified(image, MAX_SMALL_BLOB)?, types)?;
    let config = String::from_utf8(store.read_verified(&manifest.config, MAX_SMALL_BLOB)?)
        .map_err(|e| invalid(e.to_string()))?;
    let line = format!("platform = \"{}\"\n", manifest.platform()?);
    if config.matches(&line).count() != 1 {
        return Err(invalid("the config does not name its platform once"));
    }
    let config = config.replace(&line, &format!("platform = \"{id}\"\n"));
    manifest.config = restored(&manifest.config, config.as_bytes())?;
    let layers = manifest.layers.clone();
    for layer in &mut manifest.layers {
        if layer.media_type == crate::media_type::METADATA {
            let mut metadata = crate::metadata::Metadata::decode(
                &store.read_verified(layer, MAX_SMALL_BLOB)?,
                &layers,
            )?;
            metadata.platform = id.into();
            *layer = restored(layer, &metadata.encode(&layers)?)?;
        }
    }
    manifest
        .annotations
        .insert("unemu.platform".into(), id.into());
    manifest.annotations.remove(crate::compat::MIN_APP_VERSION);
    store.import_manifest_with_types(&manifest, types)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retired_id_is_the_same_platform_as_its_replacement_only() {
        assert_eq!(canonical("ibm-pc-at"), "ibm-pc");
        assert_eq!(canonical("ibm-pc"), "ibm-pc");
        assert_eq!(canonical("c64"), "c64");
        assert!(same("ibm-pc-at", "ibm-pc"));
        assert!(same("ibm-pc", "ibm-pc-at"));
        assert!(same("ibm-pc-at", "ibm-pc-at"));
        assert!(same("c64", "c64"));
        for other in ["ibm-pc-xt", "ibm-pc-at2", "ibm", "c64", "IBM-PC", ""] {
            assert!(!same(other, "ibm-pc"), "{other}");
            assert!(!same(other, "ibm-pc-at"), "{other}");
        }
    }

    /// A retired id never replaces another, and an id is retired once, so
    /// one lookup gives the current id.
    #[test]
    fn the_alias_table_is_flat() {
        for (retired, current) in ALIASES {
            assert_ne!(retired, current);
            assert!(crate::artifact::valid_platform(retired));
            assert!(crate::artifact::valid_platform(current));
            assert!(ALIASES.iter().all(|(r, _)| r != current), "{current}");
            assert_eq!(ALIASES.iter().filter(|(r, _)| r == retired).count(), 1);
        }
    }
}
