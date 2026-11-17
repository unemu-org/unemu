// SPDX-License-Identifier: AGPL-3.0-or-later
//! What one registry tag offers, read without pulling it; machine-images §5.1
//! and the approved remotes mock-up (`backlog/docs/design/mockups/remotes.html`,
//! note "What is read without pulling layers").
//!
//! A tag is read from its OCI image manifest (OCI Distribution Specification
//! 1.1, "Pulling manifests") and, for an installation media set or a firmware
//! package, from its config record, whose formats, name and version the list
//! shows. A machine image's config holds nothing the list shows and is not
//! read. For the one tag a person selects, the small metadata record adds the
//! edition and version ([`read_title`]). No other layer is ever fetched, no
//! cover, disk or ROM, and nothing is written: records live in memory only.
//!
//! The manifest is read leniently, as a browser must: unknown fields and
//! artifact types are kept as they are, so a newer release's image or another
//! tool's artifact still gets a row. Strict decoding stays with pull.
//!
//! Every function takes a `cancelled` check and asks it before each request
//! after the first, answering [`Error::Cancelled`] once it holds.

use crate::{Client, Error, Result};
use image_store::{media_type as mt, metadata::Metadata, metadata::Title, Descriptor, Digest};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// Manifest annotation: the oldest release of the program providing the
/// image's platform that reads and runs it; machine-images §5.6. Browsing
/// reports it as the registry has it, without interpreting it.
pub use image_store::compat::MIN_APP_VERSION;

/// One tag as the remote list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagRecord {
    /// The manifest the tag names now; a pull by this digest fetches it.
    pub digest: Digest,
    /// The manifest's `artifactType`, or the type its config media type
    /// implies when the manifest has none.
    pub artifact_type: String,
    /// The manifest's annotations, as the registry has them.
    pub annotations: BTreeMap<String, String>,
    /// The manifest's layers, with their annotations, none of them fetched.
    pub layers: Vec<Descriptor>,
    /// The manifest and every distinct blob it names, as stored.
    pub size: u64,
    /// Some descriptor carries the `user-supplied` redistribution mark.
    pub user_supplied: bool,
    /// The config record of a media set or firmware package, verified
    /// against its digest; `None` for any other artifact.
    pub config: Option<Vec<u8>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    #[serde(default)]
    artifact_type: Option<String>,
    config: Entry,
    #[serde(default)]
    layers: Vec<Entry>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    media_type: String,
    digest: String,
    size: u64,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
}

impl Entry {
    fn descriptor(&self) -> Result<Descriptor> {
        let digest = self
            .digest
            .parse()
            .map_err(|_| Error::Protocol(format!("invalid digest {}", self.digest)))?;
        let mut descriptor = Descriptor::new(self.media_type.clone(), digest, self.size);
        descriptor.annotations = self.annotations.clone();
        Ok(descriptor)
    }
}

/// The artifact type a config media type implies.
fn implied_type(config: &str) -> &str {
    match config {
        mt::CONFIG => mt::MACHINE,
        mt::MEDIA_SET_CONFIG => mt::MEDIA_SET,
        mt::FIRMWARE_CONFIG => mt::FIRMWARE,
        other => other,
    }
}

fn check(cancelled: &dyn Fn() -> bool) -> Result<()> {
    if cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(())
}

/// Every tag of `repository`, `page_size` a page, following the registry's
/// `Link` paging (distribution-spec, "Listing Tags").
pub fn list_tags(
    client: &Client,
    repository: &str,
    page_size: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<String>> {
    const MAX_PAGES: usize = 10_000;
    let mut tags = Vec::new();
    let mut next: Option<String> = None;
    let mut visited = BTreeSet::new();
    for page in 0..MAX_PAGES {
        if page > 0 {
            check(cancelled)?;
        }
        let page = client.tags_page(repository, page_size, next.as_deref())?;
        tags.extend(page.names);
        match page.next {
            Some(url) if visited.insert(url.clone()) => next = Some(url),
            Some(_) => return Err(Error::Protocol("registry paging repeats a page".into())),
            None => return Ok(tags),
        }
    }
    Err(Error::Protocol("registry paging does not end".into()))
}

/// Read `repository:tag`: its manifest and, for a media set or firmware
/// package, its config record. A tag that is gone, or that names a manifest
/// type other than the OCI image manifest, is [`Error::NotFound`].
pub fn read_tag(
    client: &Client,
    repository: &str,
    tag: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<TagRecord> {
    let fetched = client.manifest(repository, tag)?.ok_or_else(|| {
        Error::NotFound(format!(
            "{repository}:{tag} (no OCI image manifest under this tag)"
        ))
    })?;
    let envelope: Envelope = serde_json::from_slice(&fetched.bytes)
        .map_err(|e| Error::Protocol(format!("manifest of {repository}:{tag}: {e}")))?;
    let config = envelope.config.descriptor()?;
    let layers = envelope
        .layers
        .iter()
        .map(Entry::descriptor)
        .collect::<Result<Vec<_>>>()?;
    let mut seen = BTreeSet::from([config.digest.clone()]);
    let mut size = (fetched.bytes.len() as u64).saturating_add(config.size);
    for layer in &layers {
        if seen.insert(layer.digest.clone()) {
            size = size.saturating_add(layer.size);
        }
    }
    let user_supplied = std::iter::once(&config).chain(&layers).any(|d| {
        d.annotations
            .get(image_store::license::REDISTRIBUTION)
            .is_some_and(|mark| mark == image_store::license::USER_SUPPLIED)
    });
    let config_bytes = match config.media_type.as_str() {
        mt::MEDIA_SET_CONFIG | mt::FIRMWARE_CONFIG => {
            check(cancelled)?;
            Some(client.small_blob(repository, &config)?)
        }
        _ => None,
    };
    Ok(TagRecord {
        digest: fetched.digest,
        artifact_type: envelope
            .artifact_type
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| implied_type(&config.media_type).to_owned()),
        annotations: envelope.annotations,
        layers,
        size,
        user_supplied,
        config: config_bytes,
    })
}

/// The first title of a read tag's metadata record, for the one tag a person
/// selected: the edition and version a media set or machine image keeps
/// there. Only the manifest's single uncompressed metadata layer is fetched,
/// at most 1 MiB and verified ([`Client::small_blob`]); covers and screenshots
/// it names are not. `None` when the manifest has no such layer, as a
/// firmware package has none.
pub fn read_title(
    client: &Client,
    repository: &str,
    record: &TagRecord,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Title>> {
    let mut metadata = record
        .layers
        .iter()
        .filter(|layer| layer.media_type == mt::METADATA);
    let (Some(layer), None) = (metadata.next(), metadata.next()) else {
        return Ok(None);
    };
    check(cancelled)?;
    let bytes = client.small_blob(repository, layer)?;
    let metadata = Metadata::decode_for_display(&bytes, &record.layers)
        .map_err(|e| Error::Protocol(format!("metadata of {repository}: {e}")))?;
    Ok(metadata.titles.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        client::Challenge,
        credentials::Anonymous,
        test_support::{MockRegistry, Policy},
    };
    use image_store::MAX_SMALL_BLOB;

    fn entry(media_type: &str, bytes: &[u8]) -> String {
        format!(
            r#"{{"mediaType":"{media_type}","digest":"{}","size":{}}}"#,
            Digest::of(bytes),
            bytes.len()
        )
    }

    /// A manifest of the given artifact type (none when empty) with one config
    /// and one layer, written as another tool might: with an unknown field.
    fn manifest(artifact_type: &str, config: (&str, &[u8]), layer: &[u8]) -> Vec<u8> {
        let artifact_type = if artifact_type.is_empty() {
            String::new()
        } else {
            format!(r#""artifactType":"{artifact_type}","#)
        };
        format!(
            r#"{{"schemaVersion":2,"mediaType":"{}",{artifact_type}"config":{},"layers":[{}],"subject":null,"annotations":{{"{MIN_APP_VERSION}":"99.0.0"}}}}"#,
            mt::MANIFEST,
            entry(config.0, config.1),
            entry(mt::FIRMWARE, layer),
        )
        .into_bytes()
    }

    #[test]
    fn a_tag_is_read_from_its_manifest_and_verified_config_record_only() {
        let registry = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            ..Policy::default()
        });
        let client = Client::new(&registry.entry(false), Box::new(Anonymous)).unwrap();
        let config = b"schema = 1\nname = \"BIOS\"\n";
        let layer = b"rom bytes";
        registry.seed_blob("firmware/bios", config);
        registry.seed_blob("firmware/bios", layer);
        let bytes = manifest(mt::FIRMWARE, (mt::FIRMWARE_CONFIG, config), layer);
        let digest = registry.seed_manifest("firmware/bios", "1", &bytes);
        let record = read_tag(&client, "firmware/bios", "1", &|| false).unwrap();
        assert_eq!(record.digest, digest);
        assert_eq!(record.artifact_type, mt::FIRMWARE);
        assert_eq!(record.config.as_deref(), Some(&config[..]));
        assert_eq!(record.annotations[MIN_APP_VERSION], "99.0.0");
        assert_eq!(
            record.size,
            (bytes.len() + config.len() + layer.len()) as u64
        );
        let blobs: Vec<_> = registry
            .log()
            .into_iter()
            .filter(|line| line.contains("/blobs/"))
            .collect();
        assert_eq!(
            blobs,
            [format!(
                "GET /v2/firmware/bios/blobs/{}",
                Digest::of(config)
            )],
            "the layer is never fetched"
        );

        // A config record that does not hash to its digest is refused.
        registry.corrupt_blob(&Digest::of(config));
        assert!(matches!(
            read_tag(&client, "firmware/bios", "1", &|| false),
            Err(Error::DigestMismatch { .. })
        ));

        // Without an artifact type the config media type names it; a
        // machine's config is not read.
        let machine_config = b"schema = 1\n";
        registry.seed_blob("games/x", machine_config);
        registry.seed_blob("games/x", layer);
        let bytes = manifest("", (mt::CONFIG, machine_config), layer);
        registry.seed_manifest("games/x", "1.0.0", &bytes);
        registry.clear_log();
        let record = read_tag(&client, "games/x", "1.0.0", &|| false).unwrap();
        assert_eq!(record.artifact_type, mt::MACHINE);
        assert_eq!(record.config, None);
        assert!(!registry.log().iter().any(|line| line.contains("/blobs/")));
        assert!(matches!(
            read_tag(&client, "games/x", "2.0.0", &|| false),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn a_small_blob_is_refused_before_a_request_when_large_and_when_longer_than_described() {
        let registry = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            ..Policy::default()
        });
        let client = Client::new(&registry.entry(false), Box::new(Anonymous)).unwrap();
        let bytes = b"ten bytes!";
        let digest = registry.seed_blob("games/x", bytes);
        let large = Descriptor::new(mt::METADATA, digest.clone(), MAX_SMALL_BLOB + 1);
        assert!(matches!(
            client.small_blob("games/x", &large),
            Err(Error::Protocol(_))
        ));
        assert!(registry.log().is_empty(), "no request for a large blob");
        let short = Descriptor::new(mt::METADATA, digest.clone(), 5);
        assert!(matches!(
            client.small_blob("games/x", &short),
            Err(Error::Protocol(_))
        ));
        let exact = Descriptor::new(mt::METADATA, digest, bytes.len() as u64);
        assert_eq!(client.small_blob("games/x", &exact).unwrap(), bytes);
    }

    #[test]
    fn a_title_comes_from_the_single_metadata_layer_only() {
        let registry = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            ..Policy::default()
        });
        let client = Client::new(&registry.entry(false), Box::new(Anonymous)).unwrap();
        let metadata = br#"{"schema":1,"platform":"synthetic","titles":[{"id":"keen4","title":"Keen 4","edition":"Shareware","version":"1.4"}]}"#;
        let disk = b"disk bytes";
        let config = b"schema = 1\n";
        for blob in [&metadata[..], disk, config] {
            registry.seed_blob("games/keen", blob);
        }
        let bytes = format!(
            r#"{{"schemaVersion":2,"mediaType":"{}","config":{},"layers":[{},{}]}}"#,
            mt::MANIFEST,
            entry(mt::CONFIG, config),
            entry(mt::FLOPPY, disk),
            entry(mt::METADATA, metadata),
        );
        registry.seed_manifest("games/keen", "1.4", bytes.as_bytes());
        let record = read_tag(&client, "games/keen", "1.4", &|| false).unwrap();
        registry.clear_log();
        assert!(matches!(
            read_title(&client, "games/keen", &record, &|| true),
            Err(Error::Cancelled)
        ));
        assert!(registry.log().is_empty());
        let title = read_title(&client, "games/keen", &record, &|| false)
            .unwrap()
            .unwrap();
        assert_eq!(title.edition.as_deref(), Some("Shareware"));
        assert_eq!(title.version.as_deref(), Some("1.4"));
        assert_eq!(
            registry.log(),
            [format!("GET /v2/games/keen/blobs/{}", Digest::of(metadata))]
        );
    }

    #[test]
    fn a_login_request_keeps_the_registry_challenge() {
        let registry = MockRegistry::start(Policy {
            user: Some(("ci".into(), "secret".into())),
            ..Policy::default()
        });
        let client = Client::new(&registry.entry(true), Box::new(Anonymous)).unwrap();
        assert_eq!(client.last_challenge(), None);
        assert!(matches!(
            read_tag(&client, "private/x", "1.0.0", &|| false),
            Err(Error::AuthRequired { .. })
        ));
        assert_eq!(
            client.last_challenge(),
            Some(Challenge::Basic {
                realm: Some("mock".into())
            })
        );
    }
}
