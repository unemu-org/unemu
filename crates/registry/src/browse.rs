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
//! cover, disk or ROM. The uncached functions keep records in memory only;
//! cache-backed variants hand verified bytes to the caller's bounded cache.
//!
//! The manifest is read leniently, as a browser must: unknown fields and
//! artifact types are kept as they are, so a newer release's image or another
//! tool's artifact still gets a row. Strict decoding stays with pull.
//!
//! Every function takes a `cancelled` check and asks it before each request
//! after the first, answering [`Error::Cancelled`] once it holds.

use crate::{client::FetchedManifest, Client, Error, Result};
use image_store::{media_type as mt, metadata::Metadata, metadata::Title, Descriptor, Digest};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// Verified browse content and tag resolutions, scoped to one registry host.
/// Implementations must bound entries and verify their digest on every read.
pub trait Cache {
    fn get(&self, digest: &Digest) -> Option<Vec<u8>>;
    fn put(&self, digest: &Digest, bytes: &[u8]);
    fn tag(&self, repository: &str, tag: &str) -> Option<Digest>;
    /// Called only for a fresh registry resolution, never for a cache hit.
    fn remember(&self, repository: &str, tag: &str, digest: &Digest, immutable: bool);
}

/// Resolve once per cache session, reusing manifest bytes by digest.
pub fn cached_manifest(
    client: &Client,
    repository: &str,
    reference: &str,
    cache: &dyn Cache,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<FetchedManifest>> {
    let Some(digest) = cached_digest(client, repository, reference, cache, cancelled)? else {
        return Ok(None);
    };
    if let Some(bytes) = cache.get(&digest) {
        return Ok(Some(FetchedManifest {
            digest,
            bytes,
            media_type: Some(mt::MANIFEST.into()),
        }));
    }
    check(cancelled)?;
    let fetched = client.manifest(repository, &digest.to_string())?;
    if let Some(fetched) = &fetched {
        cache.put(&fetched.digest, &fetched.bytes);
    }
    Ok(fetched)
}

/// Resolve a tag, retaining any verified manifest bytes for later record reads.
/// Potential immutable tags need their manifest type before a mapping can persist.
pub fn cached_digest(
    client: &Client,
    repository: &str,
    reference: &str,
    cache: &dyn Cache,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Digest>> {
    if let Some(digest) = reference
        .parse::<Digest>()
        .ok()
        .or_else(|| cache.tag(repository, reference))
    {
        return Ok(Some(digest));
    }
    check(cancelled)?;
    let (digest, mut fetched) =
        client.manifest_resolution_cancellable(repository, reference, cancelled)?;
    let Some(digest) = digest else {
        return Ok(None);
    };
    cache.remember(repository, reference, &digest, false);
    let bytes = cache.get(&digest);
    if fetched.is_none()
        && bytes.is_none()
        && (image_store::tag::is_revision(reference)
            || image_store::tag::precedence(reference).is_some())
    {
        // Persist while this tag resolution is fresh; later session reads must
        // not overwrite a mapping another process has already refreshed.
        check(cancelled)?;
        fetched = client.manifest(repository, &digest.to_string())?;
    }
    if let Some(fetched) = fetched {
        if fetched
            .media_type
            .as_deref()
            .is_none_or(|kind| kind == mt::MANIFEST)
        {
            cache.put(&digest, &fetched.bytes);
            remember_manifest(cache, repository, reference, &digest, &fetched.bytes);
        }
    } else if let Some(bytes) = bytes {
        remember_manifest(cache, repository, reference, &digest, &bytes);
    }
    Ok(Some(digest))
}

fn immutable_tag(artifact_type: &str, tag: &str) -> bool {
    if image_store::tag::governs(artifact_type) {
        image_store::tag::is_revision(tag)
    } else {
        artifact_type == mt::MACHINE && image_store::tag::precedence(tag).is_some()
    }
}

fn remember_manifest(
    cache: &dyn Cache,
    repository: &str,
    tag: &str,
    digest: &Digest,
    bytes: &[u8],
) {
    if let Ok(envelope) = serde_json::from_slice::<Envelope>(bytes) {
        let artifact_type = envelope
            .artifact_type
            .as_deref()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| implied_type(&envelope.config.media_type));
        cache.remember(repository, tag, digest, immutable_tag(artifact_type, tag));
    }
}

fn small_blob(
    client: &Client,
    repository: &str,
    descriptor: &Descriptor,
    cache: Option<&dyn Cache>,
) -> Result<Vec<u8>> {
    if descriptor.size <= image_store::MAX_SMALL_BLOB {
        if let Some(bytes) = cache.and_then(|cache| cache.get(&descriptor.digest)) {
            if bytes.len() as u64 != descriptor.size {
                return Err(Error::Protocol(format!(
                    "registry blob {} has {} bytes, expected {}",
                    descriptor.digest,
                    bytes.len(),
                    descriptor.size
                )));
            }
            return Ok(bytes);
        }
    }
    let bytes = client.small_blob(repository, descriptor)?;
    if let Some(cache) = cache {
        cache.put(&descriptor.digest, &bytes);
    }
    Ok(bytes)
}

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
    /// The manifest's config descriptor, with its annotations; fetched only
    /// as [`TagRecord::config`] says.
    pub config_descriptor: Descriptor,
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

/// The revision tags of `release` that `repository` has
/// ([`image_store::tag`]), highest first, read with [`list_tags`]; none for a
/// repository the registry does not have (`404`).
pub fn release_revisions(
    client: &Client,
    repository: &str,
    release: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<String>> {
    let tags = match list_tags(client, repository, 100, cancelled) {
        Err(Error::Status { status: 404, .. }) => Vec::new(),
        other => other?,
    };
    let mut revisions: Vec<(u64, String)> = tags
        .into_iter()
        .filter_map(|name| match image_store::tag::parse(&name)? {
            image_store::tag::Kind::Revision {
                release: Some(of),
                number,
            } if of == release => Some((number, name.clone())),
            _ => None,
        })
        .collect();
    revisions.sort_by_key(|(number, _)| std::cmp::Reverse(*number));
    Ok(revisions.into_iter().map(|(_, name)| name).collect())
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
    read_tag_inner(client, repository, tag, cancelled, None, None)
}

pub fn read_tag_with_compatibility(
    client: &Client,
    repository: &str,
    tag: &str,
    cancelled: &dyn Fn() -> bool,
    compatibility: &image_store::compat::Compatibility,
) -> Result<TagRecord> {
    read_tag_inner(
        client,
        repository,
        tag,
        cancelled,
        Some(compatibility),
        None,
    )
}

/// Read a tag with persistent browse content supplied by the caller.
pub fn read_tag_cached(
    client: &Client,
    repository: &str,
    tag: &str,
    cancelled: &dyn Fn() -> bool,
    compatibility: &image_store::compat::Compatibility,
    cache: &dyn Cache,
) -> Result<TagRecord> {
    read_tag_inner(
        client,
        repository,
        tag,
        cancelled,
        Some(compatibility),
        Some(cache),
    )
}

fn read_tag_inner(
    client: &Client,
    repository: &str,
    tag: &str,
    cancelled: &dyn Fn() -> bool,
    compatibility: Option<&image_store::compat::Compatibility>,
    cache: Option<&dyn Cache>,
) -> Result<TagRecord> {
    let fetched = match cache {
        Some(cache) => cached_manifest(client, repository, tag, cache, cancelled)?,
        None => client.manifest(repository, tag)?,
    }
    .ok_or_else(|| {
        Error::NotFound(format!(
            "{repository}:{tag} (no OCI image manifest under this tag)"
        ))
    })?;
    let unread = compatibility.is_some_and(|runtime| {
        let levels = runtime.manifest_levels(&fetched.bytes);
        levels.core.is_none()
            || image_store::compat::Requirements::read(&fetched.bytes)
                .unmet_reason(levels)
                .is_some()
    });
    let envelope: Envelope = serde_json::from_slice(&fetched.bytes).map_err(|e| {
        compatibility
            .and_then(|runtime| {
                runtime
                    .check(&fetched.bytes, &format!("{repository}:{tag}"))
                    .err()
            })
            .map(Error::Store)
            .unwrap_or_else(|| Error::Protocol(format!("manifest of {repository}:{tag}: {e}")))
    })?;
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
        mt::MEDIA_SET_CONFIG | mt::FIRMWARE_CONFIG if !unread => {
            check(cancelled)?;
            Some(small_blob(client, repository, &config, cache)?)
        }
        _ => None,
    };
    let artifact_type = envelope
        .artifact_type
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| implied_type(&config.media_type).to_owned());
    Ok(TagRecord {
        digest: fetched.digest,
        artifact_type,
        annotations: envelope.annotations,
        config_descriptor: config,
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
    read_title_inner(client, repository, record, cancelled, None)
}

/// Read only missing metadata bytes from the registry.
pub fn read_title_cached(
    client: &Client,
    repository: &str,
    record: &TagRecord,
    cancelled: &dyn Fn() -> bool,
    cache: &dyn Cache,
) -> Result<Option<Title>> {
    read_title_inner(client, repository, record, cancelled, Some(cache))
}

fn read_title_inner(
    client: &Client,
    repository: &str,
    record: &TagRecord,
    cancelled: &dyn Fn() -> bool,
    cache: Option<&dyn Cache>,
) -> Result<Option<Title>> {
    let mut metadata = record
        .layers
        .iter()
        .filter(|layer| layer.media_type == mt::METADATA);
    let (Some(layer), None) = (metadata.next(), metadata.next()) else {
        return Ok(None);
    };
    check(cancelled)?;
    let bytes = small_blob(client, repository, layer, cache)?;
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
        assert_eq!(
            record.config_descriptor,
            Descriptor::new(mt::FIRMWARE_CONFIG, Digest::of(config), config.len() as u64),
            "the config descriptor is kept for Contents…"
        );
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

#[cfg(test)]
mod cached_request_tests {
    use super::*;
    use crate::http::{Body, Method, Response, Transport};
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    #[derive(Default)]
    struct Records {
        content: Mutex<BTreeMap<Digest, Vec<u8>>>,
        tags: Mutex<BTreeMap<(String, String), Digest>>,
        remembered: Mutex<usize>,
        immutable: Mutex<BTreeSet<(String, String)>>,
    }
    impl Cache for Records {
        fn get(&self, digest: &Digest) -> Option<Vec<u8>> {
            self.content.lock().unwrap().get(digest).cloned()
        }
        fn put(&self, digest: &Digest, bytes: &[u8]) {
            self.content
                .lock()
                .unwrap()
                .insert(digest.clone(), bytes.to_vec());
        }
        fn tag(&self, repository: &str, tag: &str) -> Option<Digest> {
            self.tags
                .lock()
                .unwrap()
                .get(&(repository.into(), tag.into()))
                .cloned()
        }
        fn remember(&self, repository: &str, tag: &str, digest: &Digest, immutable: bool) {
            if immutable {
                self.immutable
                    .lock()
                    .unwrap()
                    .insert((repository.into(), tag.into()));
            }
            *self.remembered.lock().unwrap() += 1;
            self.tags
                .lock()
                .unwrap()
                .insert((repository.into(), tag.into()), digest.clone());
        }
    }

    #[derive(Default)]
    struct Registry {
        bodies: BTreeMap<String, Vec<u8>>,
        log: Vec<String>,
        cancel_after_head: Option<Arc<std::sync::atomic::AtomicBool>>,
        head_digest: bool,
    }
    struct MemoryTransport(Arc<Mutex<Registry>>);
    impl Transport for MemoryTransport {
        fn send(
            &self,
            method: Method,
            url: &str,
            _: &[(&str, String)],
            _: &Body<'_>,
        ) -> io::Result<Response> {
            let path = url
                .strip_prefix("https://registry.example")
                .unwrap()
                .split('?')
                .next()
                .unwrap();
            let mut registry = self.0.lock().unwrap();
            registry.log.push(format!("{} {path}", method.as_str()));
            if method == Method::Head {
                if let Some(cancel) = &registry.cancel_after_head {
                    cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let bytes = registry
                .bodies
                .get(path)
                .unwrap_or_else(|| panic!("unexpected request: {path}"))
                .clone();
            if method == Method::Head && registry.head_digest {
                Ok(Response::manifest_head(url, &Digest::of(&bytes)))
            } else {
                Ok(Response::manifest_bytes(url, bytes))
            }
        }
    }
    fn publish(registry: &mut Registry, label: &str) -> (Digest, Digest) {
        let config = format!("schema = 1\nname = \"{label}\"\n").into_bytes();
        let metadata =
            br#"{"schema":1,"platform":"synthetic","titles":[{"id":"one","title":"One"}]}"#;
        let config_digest = Digest::of(&config);
        let metadata_digest = Digest::of(metadata);
        let manifest = format!(r#"{{"schemaVersion":2,"annotations":{{"unemu.platform":"synthetic"}},"mediaType":"{}","artifactType":"{}","config":{{"mediaType":"{}","digest":"{}","size":{}}},"layers":[{{"mediaType":"{}","digest":"{}","size":{}}}]}}"#,
            mt::MANIFEST, mt::MEDIA_SET, mt::MEDIA_SET_CONFIG, config_digest, config.len(), mt::METADATA, metadata_digest, metadata.len()).into_bytes();
        let digest = Digest::of(&manifest);
        registry.bodies.insert(
            "/v2/repo/tags/list".into(),
            br#"{"name":"repo","tags":["1.4-r1"]}"#.to_vec(),
        );
        registry
            .bodies
            .insert("/v2/repo/manifests/1.4-r1".into(), manifest.clone());
        registry
            .bodies
            .insert(format!("/v2/repo/manifests/{digest}"), manifest);
        registry
            .bodies
            .insert(format!("/v2/repo/blobs/{config_digest}"), config);
        registry.bodies.insert(
            format!("/v2/repo/blobs/{metadata_digest}"),
            metadata.to_vec(),
        );
        (digest, config_digest)
    }

    #[test]
    fn digest_probe_persists_a_version_before_its_later_manifest_read() {
        let registry = Arc::new(Mutex::new(Registry::default()));
        let (digest, _) = publish(&mut registry.lock().unwrap(), "first");
        registry.lock().unwrap().head_digest = true;
        let client = Client::for_test_transport(
            "registry.example",
            Box::new(MemoryTransport(registry.clone())),
        );
        let cache = Records::default();
        assert_eq!(
            cached_digest(&client, "repo", "1.4-r1", &cache, &|| false).unwrap(),
            Some(digest.clone())
        );
        assert_eq!(
            registry.lock().unwrap().log,
            [
                "HEAD /v2/repo/manifests/1.4-r1".to_owned(),
                format!("GET /v2/repo/manifests/{digest}")
            ]
        );
        assert!(cache
            .immutable
            .lock()
            .unwrap()
            .contains(&("repo".into(), "1.4-r1".into())));
        let remembered = *cache.remembered.lock().unwrap();
        registry.lock().unwrap().log.clear();
        cached_manifest(&client, "repo", "1.4-r1", &cache, &|| false).unwrap();
        assert!(registry.lock().unwrap().log.is_empty());
        assert_eq!(*cache.remembered.lock().unwrap(), remembered);
    }

    #[test]
    fn stop_after_headerless_head_prevents_fallback_get() {
        let registry = Arc::new(Mutex::new(Registry::default()));
        publish(&mut registry.lock().unwrap(), "first");
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        registry.lock().unwrap().cancel_after_head = Some(cancel.clone());
        let client = Client::for_test_transport(
            "registry.example",
            Box::new(MemoryTransport(registry.clone())),
        );
        let cache = Records::default();
        let result = cached_manifest(&client, "repo", "1.4-r1", &cache, &|| {
            cancel.load(std::sync::atomic::Ordering::SeqCst)
        });
        assert!(matches!(result, Err(Error::Cancelled)));
        assert_eq!(
            registry.lock().unwrap().log,
            ["HEAD /v2/repo/manifests/1.4-r1"]
        );
        assert_eq!(*cache.remembered.lock().unwrap(), 0);
    }

    #[test]
    fn cached_resolutions_never_rewrite_mappings_even_if_the_manifest_was_evicted() {
        let registry = Arc::new(Mutex::new(Registry::default()));
        let (digest, _) = publish(&mut registry.lock().unwrap(), "first");
        let client = Client::for_test_transport(
            "registry.example",
            Box::new(MemoryTransport(registry.clone())),
        );
        let cache = Records::default();
        cached_manifest(&client, "repo", "1.4-r1", &cache, &|| false).unwrap();
        let remembered = *cache.remembered.lock().unwrap();
        registry.lock().unwrap().log.clear();
        cached_manifest(&client, "repo", "1.4-r1", &cache, &|| false).unwrap();
        assert_eq!(*cache.remembered.lock().unwrap(), remembered);
        assert!(registry.lock().unwrap().log.is_empty());
        cache.content.lock().unwrap().remove(&digest);
        cached_manifest(&client, "repo", "1.4-r1", &cache, &|| false).unwrap();
        assert_eq!(*cache.remembered.lock().unwrap(), remembered);
        assert_eq!(
            registry.lock().unwrap().log,
            [format!("GET /v2/repo/manifests/{digest}")]
        );
    }

    #[test]
    fn immutable_mappings_follow_each_artifact_types_tag_grammar() {
        for (kind, name, immutable) in [
            (mt::MACHINE, "1.2.3", true),
            (mt::MACHINE, "1.2.3-rc.1", true),
            (mt::MACHINE, "latest", false),
            (mt::MACHINE, "1.4", false),
            (mt::MACHINE, "r1", false),
            (mt::MEDIA_SET, "1.4-r2", true),
            (mt::MEDIA_SET, "1.4", false),
            (mt::MEDIA_SET, "1.2.3", false),
            (mt::FIRMWARE, "r1", true),
            (mt::FIRMWARE, "1", false),
            ("application/vnd.example", "1.2.3", false),
        ] {
            assert_eq!(immutable_tag(kind, name), immutable, "{kind}:{name}");
        }
    }

    #[test]
    fn cached_browse_requests_cover_selection_refresh_and_moved_tags_without_sockets() {
        let registry = Arc::new(Mutex::new(Registry::default()));
        let (_, config) = publish(&mut registry.lock().unwrap(), "first");
        let client = Client::for_test_transport(
            "registry.example",
            Box::new(MemoryTransport(registry.clone())),
        );
        let runtime = image_store::compat::Compatibility::new(
            image_store::compat::EXTRACTION_BASELINE,
            image_store::compat::NEWEST_ROW,
        )
        .with_core(
            "synthetic",
            image_store::compat::EXTRACTION_BASELINE,
            image_store::compat::BASELINE,
        );
        let cache = Records::default();
        let read =
            || read_tag_cached(&client, "repo", "1.4-r1", &|| false, &runtime, &cache).unwrap();
        let record = read();
        assert!(record.config.is_some());
        registry.lock().unwrap().log.clear();
        let record = read();
        read_title_cached(&client, "repo", &record, &|| false, &cache).unwrap();
        let metadata = record.layers[0].digest.clone();
        let selection = registry.lock().unwrap().log.clone();
        registry.lock().unwrap().log.clear();
        cache.tags.lock().unwrap().clear();
        list_tags(&client, "repo", 100, &|| false).unwrap();
        let record = read();
        read_title_cached(&client, "repo", &record, &|| false, &cache).unwrap();
        let refresh = registry.lock().unwrap().log.clone();
        let (digest, new_config) = publish(&mut registry.lock().unwrap(), "second");
        assert_ne!(new_config, config);
        registry.lock().unwrap().log.clear();
        cache.tags.lock().unwrap().clear();
        list_tags(&client, "repo", 100, &|| false).unwrap();
        assert_eq!(read().digest, digest);
        let moved = registry.lock().unwrap().log.clone();
        registry.lock().unwrap().log.clear();
        list_tags(&client, "repo", 100, &|| false).unwrap();
        let record = read();
        read_title_cached(&client, "repo", &record, &|| false, &cache).unwrap();
        let reused = registry.lock().unwrap().log.clone();
        assert_eq!(
            (selection, refresh, moved, reused),
            (
                vec![format!("GET /v2/repo/blobs/{metadata}")],
                vec![
                    "GET /v2/repo/tags/list".into(),
                    "HEAD /v2/repo/manifests/1.4-r1".into(),
                    "GET /v2/repo/manifests/1.4-r1".into()
                ],
                vec![
                    "GET /v2/repo/tags/list".into(),
                    "HEAD /v2/repo/manifests/1.4-r1".into(),
                    "GET /v2/repo/manifests/1.4-r1".into(),
                    format!("GET /v2/repo/blobs/{new_config}")
                ],
                vec!["GET /v2/repo/tags/list".into()],
            )
        );
        registry.lock().unwrap().log.clear();
        let mut descriptor = record.config_descriptor;
        descriptor.size += 1;
        assert!(matches!(
            small_blob(&client, "repo", &descriptor, Some(&cache)),
            Err(Error::Protocol(_))
        ));
        assert!(registry.lock().unwrap().log.is_empty());
    }
}
