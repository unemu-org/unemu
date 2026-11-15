// SPDX-License-Identifier: AGPL-3.0-or-later
//! Push, pull, guard and listing against the in-process mock registry, plus an
//! opt-in live round trip. Synthetic images only; no emulator or guest data.

use crate::{
    client::{AnonymousAccess, Client},
    credentials::{Anonymous, CredentialSource, Credentials, EnvCredentials, HostCredentials},
    guard::{MarkSource, Redistribution, Refusal},
    pull, push,
    push::{BlobOutcome, PushOptions, TagOutcome, UploadSessions, MIN_CHUNK_SIZE},
    test_support::{BlobFault, MockRegistry, PatchFault, Policy, RangeSupport},
    Error, Reference, RegistryEntry,
};
use image_store::{
    media_type as mt, BlobSource, ConfigValidator, Descriptor, Digest, Manifest, Store,
    MAX_SMALL_BLOB,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) struct Temp(PathBuf);
impl Temp {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "image-registry-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    pub(crate) fn store(&self) -> Store {
        Store::open(&self.0).unwrap()
    }
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A neutral provider: the config lists its payload digests.
pub(crate) struct Synthetic;
impl ConfigValidator for Synthetic {
    fn platform(&self) -> &str {
        "synthetic"
    }
    fn validate(
        &self,
        table: &toml::Table,
        _: &[Descriptor],
        _: &dyn BlobSource,
    ) -> image_store::Result<Vec<Digest>> {
        table
            .get("blobs")
            .and_then(toml::Value::as_array)
            .ok_or_else(|| image_store::Error::Invalid("missing blobs".into()))?
            .iter()
            .map(|v| v.as_str().unwrap_or_default().parse())
            .collect()
    }
}
pub(crate) const PROVIDERS: [&dyn ConfigValidator; 1] = [&Synthetic];

fn marked(store: &Store, kind: &str, bytes: &[u8], mark: Option<&str>) -> Descriptor {
    let blob = store.import(bytes, None).unwrap();
    let mut descriptor = Descriptor::new(kind, blob.digest, blob.size);
    if let Some(mark) = mark {
        descriptor
            .annotations
            .insert("unemu.redistribution".into(), mark.into());
    }
    descriptor
}

/// Tag an image whose config, metadata and payloads all carry `mark`, except
/// payloads listed with their own mark.
pub(crate) fn image(
    store: &Store,
    tag: &str,
    payloads: &[(&[u8], Option<&str>)],
    mark: Option<&str>,
) -> Descriptor {
    let layers: Vec<_> = payloads
        .iter()
        .map(|(bytes, own)| marked(store, mt::FIRMWARE, bytes, *own))
        .collect();
    let blobs = layers
        .iter()
        .map(|d| format!("\"{}\"", d.digest))
        .collect::<Vec<_>>()
        .join(",");
    let config = marked(
        store,
        mt::CONFIG,
        format!("schema = 1\nplatform = \"synthetic\"\nblobs = [{blobs}]\n").as_bytes(),
        mark,
    );
    let metadata = marked(
        store,
        mt::METADATA,
        br#"{"schema":1,"platform":"synthetic","titles":[]}"#,
        mark,
    );
    let mut all = layers;
    all.push(metadata);
    let manifest = Manifest::new("synthetic", config, all).unwrap();
    let descriptor = store.import_manifest(&manifest).unwrap();
    store
        .update_tag(tag, &descriptor, None, &Synthetic)
        .unwrap();
    descriptor
}

fn open_image(store: &Store, tag: &str, label: &[u8]) -> Descriptor {
    image(store, tag, &[(label, Some("open"))], Some("open"))
}

fn client(entry: &RegistryEntry, credentials: impl CredentialSource + 'static) -> Client {
    Client::new(entry, Box::new(credentials)).unwrap()
}

pub(crate) fn target(registry: &MockRegistry, path: &str) -> Reference {
    Reference::parse(&format!("{}/{path}", registry.host()), "unused.example").unwrap()
}

fn push_with(
    store: &Store,
    local: &str,
    target: Reference,
    entry: &RegistryEntry,
    client: &Client,
    options: &PushOptions,
) -> crate::Result<push::PushReport> {
    let plan = push::plan(store, local, target, Some(entry))?;
    push::push(store, &plan, &PROVIDERS, client, options, &mut |_, _| {})
}

pub(crate) fn open_policy() -> Policy {
    Policy {
        anonymous_read: vec!["*".into()],
        anonymous_write: vec!["*".into()],
        ..Policy::default()
    }
}

fn uploads(log: &[String]) -> usize {
    log.iter()
        .filter(|l| {
            l.starts_with("POST ")
                || l.starts_with("PATCH ")
                || (l.contains("/blobs/uploads/") && l.starts_with("PUT "))
        })
        .count()
}

#[test]
fn push_uploads_only_missing_blobs_and_publishes_the_manifest_last() {
    let dir = Temp::new();
    let store = dir.store();
    let image = image(
        &store,
        "lab:1.0.0",
        &[
            (b"present payload", Some("open")),
            (b"new payload", Some("open")),
        ],
        Some("open"),
    );
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    registry.seed_blob("owner/lab", b"present payload");
    let client = client(&entry, Anonymous);
    let report = push_with(
        &store,
        "lab:1.0.0",
        target(&registry, "owner/lab:1.0.0"),
        &entry,
        &client,
        &PushOptions::default(),
    )
    .unwrap();
    assert_eq!(report.tag, TagOutcome::Created);
    let outcomes: Vec<_> = report.blobs.iter().map(|(_, o)| *o).collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == BlobOutcome::Present)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == BlobOutcome::Uploaded)
            .count(),
        3
    );
    let log = registry.log();
    let manifest_put = log
        .iter()
        .position(|l| l == "PUT /v2/owner/lab/manifests/1.0.0")
        .expect("manifest published");
    assert_eq!(manifest_put, log.len() - 1, "manifest PUT is last: {log:?}");
    let heads = log[..manifest_put]
        .iter()
        .filter(|l| l.starts_with("HEAD /v2/owner/lab/blobs/"))
        .count();
    assert_eq!(
        heads, 8,
        "every blob is checked before upload and before the manifest"
    );
    assert_eq!(
        uploads(&log),
        6,
        "three POST + PUT pairs, none for the present blob"
    );
    assert_eq!(
        registry.tag_digest("owner/lab", "1.0.0"),
        Some(image.digest.clone())
    );

    registry.clear_log();
    let again = push_with(
        &store,
        "lab:1.0.0",
        target(&registry, "owner/lab:1.0.0"),
        &entry,
        &client,
        &PushOptions::default(),
    )
    .unwrap();
    assert_eq!(again.tag, TagOutcome::Unchanged);
    assert_eq!(uploads(&registry.log()), 0);
}

fn large_image(store: &Store, tag: &str) -> (Descriptor, Vec<u8>) {
    let size = 2 * MIN_CHUNK_SIZE as usize + 12_345;
    let payload: Vec<u8> = (0..size).map(|i| (i * 7 % 251) as u8).collect();
    let descriptor = image(store, tag, &[(&payload, Some("open"))], Some("open"));
    (descriptor, payload)
}

fn chunked_options(max_retries: u32) -> PushOptions {
    PushOptions {
        chunk_size: MIN_CHUNK_SIZE,
        max_retries,
        ..PushOptions::default()
    }
}

#[test]
fn chunked_upload_resumes_from_the_registry_offset_after_a_dropped_connection() {
    let dir = Temp::new();
    let store = dir.store();
    let (image, payload) = large_image(&store, "big:1.0.0");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    registry.inject_patch_fault(PatchFault {
        nth: 2,
        keep: 1000,
        drop_session: false,
    });
    let client = client(&entry, Anonymous);
    push_with(
        &store,
        "big:1.0.0",
        target(&registry, "owner/big:1.0.0"),
        &entry,
        &client,
        &chunked_options(3),
    )
    .unwrap();
    let log = registry.log();
    let patches: Vec<_> = log.iter().filter(|l| l.starts_with("PATCH ")).collect();
    assert_eq!(patches.len(), 4, "{log:?}");
    assert!(log
        .iter()
        .any(|l| l.starts_with("GET /v2/owner/big/blobs/uploads/session-")));
    assert!(registry.has_blob("owner/big", &Digest::of(&payload)));
    assert_eq!(
        registry.tag_digest("owner/big", "1.0.0"),
        Some(image.digest)
    );
    assert_eq!(UploadSessions::for_store(&store).count(), 0);
}

#[test]
fn an_interrupted_upload_resumes_its_persisted_session_in_a_later_push() {
    let dir = Temp::new();
    let store = dir.store();
    let (_, payload) = large_image(&store, "big:1.0.0");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    registry.inject_patch_fault(PatchFault {
        nth: 2,
        keep: 0,
        drop_session: false,
    });
    let client = client(&entry, Anonymous);
    let first = push_with(
        &store,
        "big:1.0.0",
        target(&registry, "owner/big:1.0.0"),
        &entry,
        &client,
        &chunked_options(0),
    );
    assert!(matches!(first, Err(Error::Transport(_))), "{first:?}");
    assert_eq!(UploadSessions::for_store(&store).count(), 1);
    assert!(registry.sessions().contains(&(MIN_CHUNK_SIZE as usize)));
    registry.clear_log();
    push_with(
        &store,
        "big:1.0.0",
        target(&registry, "owner/big:1.0.0"),
        &entry,
        &client,
        &chunked_options(0),
    )
    .unwrap();
    let log = registry.log();
    let big = Digest::of(&payload);
    // The config went up in the first push; the large blob continues its old
    // session; only the metadata needs a new one.
    let posts = log.iter().filter(|l| l.starts_with("POST ")).count();
    assert_eq!(posts, 1, "{log:?}");
    assert!(log.contains(&"GET /v2/owner/big/blobs/uploads/session-2".to_owned()));
    let patches: Vec<_> = log.iter().filter(|l| l.starts_with("PATCH ")).collect();
    assert_eq!(patches.len(), 2);
    assert!(patches.iter().all(|l| l.ends_with("/session-2")));
    assert!(registry.has_blob("owner/big", &big));
    assert_eq!(UploadSessions::for_store(&store).count(), 0);
}

#[test]
fn a_session_the_registry_forgot_restarts_from_zero() {
    let dir = Temp::new();
    let store = dir.store();
    let (_, payload) = large_image(&store, "big:1.0.0");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    registry.inject_patch_fault(PatchFault {
        nth: 2,
        keep: 500,
        drop_session: true,
    });
    let client = client(&entry, Anonymous);
    push_with(
        &store,
        "big:1.0.0",
        target(&registry, "owner/big:1.0.0"),
        &entry,
        &client,
        &chunked_options(3),
    )
    .unwrap();
    let log = registry.log();
    assert_eq!(log.iter().filter(|l| l.starts_with("PATCH ")).count(), 5);
    assert!(registry.has_blob("owner/big", &Digest::of(&payload)));
}

#[test]
fn basic_credentials_answer_one_challenge_and_are_reused() {
    let dir = Temp::new();
    let store = dir.store();
    open_image(&store, "lab:1.0.0", b"payload");
    let registry = MockRegistry::start(Policy {
        user: Some(("ci".into(), "synthetic-secret".into())),
        ..Policy::default()
    });
    let entry = registry.entry(true);
    let target = || target(&registry, "private/lab:1.0.0");
    let anonymous = push_with(
        &store,
        "lab:1.0.0",
        target(),
        &entry,
        &client(&entry, Anonymous),
        &PushOptions::default(),
    );
    assert!(
        matches!(anonymous, Err(Error::AuthRequired { .. })),
        "{anonymous:?}"
    );
    let wrong = push_with(
        &store,
        "lab:1.0.0",
        target(),
        &entry,
        &client(&entry, Credentials::new("ci", "wrong")),
        &PushOptions::default(),
    );
    assert!(
        matches!(wrong, Err(Error::Unauthorized { .. })),
        "{wrong:?}"
    );
    let before = registry.challenges();
    push_with(
        &store,
        "lab:1.0.0",
        target(),
        &entry,
        &client(&entry, Credentials::new("ci", "synthetic-secret")),
        &PushOptions::default(),
    )
    .unwrap();
    assert_eq!(registry.challenges() - before, 1);
}

#[test]
fn version_tags_are_immutable_and_other_tags_move_only_when_confirmed() {
    let dir = Temp::new();
    let store = dir.store();
    let first = open_image(&store, "lab:1.0.0", b"first");
    let second = open_image(&store, "lab:2.0.0", b"second");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    let options = PushOptions::default();
    for tag in ["1.0.0", "latest"] {
        push_with(
            &store,
            "lab:1.0.0",
            target(&registry, &format!("o/lab:{tag}")),
            &entry,
            &client,
            &options,
        )
        .unwrap();
    }
    let moving = PushOptions {
        allow_move: true,
        ..PushOptions::default()
    };
    let immutable = push_with(
        &store,
        "lab:2.0.0",
        target(&registry, "o/lab:1.0.0"),
        &entry,
        &client,
        &moving,
    );
    assert!(
        matches!(&immutable, Err(Error::ImmutableTag { current, new, .. }) if *current == first.digest && *new == second.digest),
        "{immutable:?}"
    );
    let unconfirmed = push_with(
        &store,
        "lab:2.0.0",
        target(&registry, "o/lab:latest"),
        &entry,
        &client,
        &options,
    );
    let message = unconfirmed.unwrap_err().to_string();
    assert!(
        message.contains(&first.digest.to_string()) && message.contains(&second.digest.to_string()),
        "{message}"
    );
    assert_eq!(
        registry.tag_digest("o/lab", "latest"),
        Some(first.digest.clone())
    );
    let moved = push_with(
        &store,
        "lab:2.0.0",
        target(&registry, "o/lab:latest"),
        &entry,
        &client,
        &moving,
    )
    .unwrap();
    assert_eq!(
        moved.tag,
        TagOutcome::Moved {
            from: first.digest.clone()
        }
    );
    assert_eq!(registry.tag_digest("o/lab", "latest"), Some(second.digest));
    assert_eq!(registry.tag_digest("o/lab", "1.0.0"), Some(first.digest));
}

#[test]
fn pull_verifies_every_blob_and_tags_only_a_complete_closure() {
    let source = Temp::new();
    let store = source.store();
    let image = image(
        &store,
        "lab:1.0.0",
        &[(b"rom", Some("open")), (b"disk", Some("open"))],
        Some("open"),
    );
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    let reference = target(&registry, "owner/lab:1.0.0");
    push_with(
        &store,
        "lab:1.0.0",
        reference.clone(),
        &entry,
        &client,
        &PushOptions::default(),
    )
    .unwrap();

    let fresh = Temp::new();
    let pulled = fresh.store();
    let report = pull::pull(
        &pulled,
        &client,
        &reference,
        "owner/lab:1.0.0",
        &PROVIDERS,
        &|_| {},
    )
    .unwrap();
    assert_eq!(report.outcome, pull::PullOutcome::Pulled);
    assert_eq!(report.fetched.len(), 4);
    assert_eq!(
        pulled.tag_descriptor("owner/lab:1.0.0").unwrap().digest,
        image.digest
    );
    pulled.verify_publication(&image, &Synthetic).unwrap();
    let again = pull::pull(
        &pulled,
        &client,
        &reference,
        "owner/lab:1.0.0",
        &PROVIDERS,
        &|_| {},
    )
    .unwrap();
    assert_eq!(again.outcome, pull::PullOutcome::Unchanged);
    let by_digest = Reference {
        tag: None,
        digest: Some(image.digest.clone()),
        ..reference.clone()
    };
    let other = pull::pull(&pulled, &client, &by_digest, "copy:1", &PROVIDERS, &|_| {}).unwrap();
    assert!(other.fetched.is_empty(), "every blob is already local");

    let corrupt = Temp::new();
    let damaged = corrupt.store();
    let rom = Digest::of(b"rom");
    registry.corrupt_blob(&rom);
    let refused = pull::pull(
        &damaged,
        &client,
        &reference,
        "owner/lab:1.0.0",
        &PROVIDERS,
        &|_| {},
    );
    assert!(
        matches!(refused, Err(Error::DigestMismatch { ref expected, .. }) if *expected == rom),
        "{refused:?}"
    );
    assert!(matches!(
        damaged.tag_descriptor("owner/lab:1.0.0"),
        Err(image_store::Error::TagNotFound(_))
    ));
    assert!(matches!(
        damaged.open_blob(&rom, 3),
        Err(image_store::Error::MissingBlob { .. })
    ));
}

#[test]
fn pull_refuses_a_conflicting_local_tag_and_a_manifest_that_does_not_match_its_digest() {
    let dir = Temp::new();
    let store = dir.store();
    open_image(&store, "lab:1.0.0", b"local");
    let remote = Temp::new();
    let remote_store = remote.store();
    open_image(&remote_store, "lab:1.0.0", b"remote");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    let reference = target(&registry, "o/lab:1.0.0");
    push_with(
        &remote_store,
        "lab:1.0.0",
        reference.clone(),
        &entry,
        &client,
        &PushOptions::default(),
    )
    .unwrap();
    let conflict = pull::pull(
        &store,
        &client,
        &reference,
        "lab:1.0.0",
        &PROVIDERS,
        &|_| {},
    );
    assert!(
        matches!(conflict, Err(Error::LocalTagConflict { .. })),
        "{conflict:?}"
    );

    let claimed = Digest::of(b"the pinned manifest");
    registry.forge_manifest("o/forged", "1", &claimed, b"{\"other\":\"bytes\"}");
    let pinned = Reference {
        tag: None,
        digest: Some(claimed.clone()),
        ..target(&registry, "o/forged:1")
    };
    for reference in [target(&registry, "o/forged:1"), pinned] {
        let mismatch = pull::pull(&store, &client, &reference, "forged:1", &PROVIDERS, &|_| {});
        assert!(
            matches!(mismatch, Err(Error::DigestMismatch { ref expected, .. }) if *expected == claimed),
            "{mismatch:?}"
        );
    }
}

#[test]
fn listing_follows_link_pages_for_tags_and_catalog() {
    let registry = MockRegistry::start(open_policy());
    for tag in ["a", "b", "c", "d", "e"] {
        registry.seed_manifest("o/lab", tag, format!("{{\"t\":\"{tag}\"}}").as_bytes());
    }
    registry.seed_repositories(&["o/one", "o/two", "p/three"]);
    let client = client(&registry.entry(false), Anonymous);
    assert_eq!(
        pull::list_tags(&client, "o/lab", 2).unwrap(),
        ["a", "b", "c", "d", "e"]
    );
    let pages = registry
        .log()
        .iter()
        .filter(|l| l.starts_with("GET /v2/o/lab/tags/list"))
        .count();
    assert_eq!(pages, 3);
    assert_eq!(
        pull::list_repositories(&client, 2).unwrap(),
        ["o/lab", "o/one", "o/two", "p/three"]
    );
}

fn assert_no_upload(registry: &MockRegistry) {
    let log = registry.log();
    assert_eq!(uploads(&log), 0, "{log:?}");
    assert!(!log
        .iter()
        .any(|l| l.contains("/manifests/") && l.starts_with("PUT ")));
}

#[test]
fn the_push_guard_refuses_before_any_upload() {
    let dir = Temp::new();
    let store = dir.store();
    image(&store, "rom:1.0.0", &[(b"vendor rom", None)], None);
    image(
        &store,
        "mixed:1.0.0",
        &[
            (b"open rom", Some("open")),
            (b"vendor rom", Some("user-supplied")),
        ],
        Some("open"),
    );
    let registry = MockRegistry::start(Policy {
        anonymous_read: vec!["unemu".into()],
        anonymous_write: vec!["*".into()],
        ..Policy::default()
    });
    let private = registry.entry(true);
    let public = registry.entry(false);
    let anonymous = client(&private, Anonymous);
    let options = PushOptions::default();
    for local in ["rom:1.0.0", "mixed:1.0.0"] {
        let refused = push_with(
            &store,
            local,
            target(&registry, "unemu/rom:1.0.0"),
            &private,
            &anonymous,
            &options,
        );
        assert!(
            matches!(
                refused,
                Err(Error::Refused(Refusal::OfficialNamespaceNeedsOpen { .. }))
            ),
            "{refused:?}"
        );
    }
    let refused = push_with(
        &store,
        "rom:1.0.0",
        target(&registry, "private/rom:1.0.0"),
        &public,
        &anonymous,
        &options,
    );
    assert!(
        matches!(
            refused,
            Err(Error::Refused(Refusal::RegistryNotPrivate { .. }))
        ),
        "{refused:?}"
    );
    let unknown = push::plan(
        &store,
        "rom:1.0.0",
        target(&registry, "private/rom:1.0.0"),
        None,
    )
    .unwrap();
    assert!(matches!(
        unknown.verdict(),
        Err(Refusal::UnknownRegistry { .. })
    ));
    assert!(
        registry.log().is_empty(),
        "configuration refusals need no network"
    );

    let readable = MockRegistry::start(Policy {
        anonymous_read: vec!["private".into()],
        anonymous_write: vec!["*".into()],
        ..Policy::default()
    });
    let entry = readable.entry(true);
    let refused = push_with(
        &store,
        "rom:1.0.0",
        target(&readable, "private/rom:1.0.0"),
        &entry,
        &client(&entry, Anonymous),
        &options,
    );
    assert!(
        matches!(
            refused,
            Err(Error::Refused(Refusal::AnonymousReadable { .. }))
        ),
        "{refused:?}"
    );
    assert_no_upload(&readable);

    let listed = MockRegistry::start(Policy {
        anonymous_catalog: true,
        ..Policy::default()
    });
    listed.seed_repositories(&["private/visible"]);
    let entry = listed.entry(true);
    let catalog = client(&entry, Anonymous).anonymous_access("private/rom");
    assert_eq!(catalog.unwrap(), crate::client::AnonymousAccess::Readable);
}

#[test]
fn allowed_pushes_send_restricted_content_to_a_private_registry_and_open_content_to_the_official_namespace(
) {
    let dir = Temp::new();
    let store = dir.store();
    // Only the ROM is restricted: the metadata bytes are the same in both
    // images, and one local user-supplied mark would restrict them in both.
    let rom = image(&store, "rom:1.0.0", &[(b"vendor rom", None)], Some("open"));
    let open = open_image(&store, "open:1.0.0", b"open rom");
    let registry = MockRegistry::start(Policy {
        anonymous_read: vec!["unemu".into()],
        anonymous_write: vec!["unemu".into()],
        user: Some(("maintainer".into(), "synthetic-secret".into())),
        ..Policy::default()
    });
    let private = registry.entry(true);
    let maintainer = client(&private, Credentials::new("maintainer", "synthetic-secret"));
    let report = push_with(
        &store,
        "rom:1.0.0",
        target(&registry, "private/rom:1.0.0"),
        &private,
        &maintainer,
        &PushOptions::default(),
    )
    .unwrap();
    assert_eq!(report.tag, TagOutcome::Created);
    assert_eq!(
        registry.tag_digest("private/rom", "1.0.0"),
        Some(rom.digest)
    );
    let public = registry.entry(false);
    let report = push_with(
        &store,
        "open:1.0.0",
        target(&registry, "unemu/open:1.0.0"),
        &public,
        &client(&public, Anonymous),
        &PushOptions::default(),
    )
    .unwrap();
    assert_eq!(report.tag, TagOutcome::Created);
    assert_eq!(
        registry.tag_digest("unemu/open", "1.0.0"),
        Some(open.digest)
    );
}

#[test]
fn missing_blobs_mount_from_another_repository_when_offered() {
    let dir = Temp::new();
    let store = dir.store();
    open_image(&store, "lab:1.0.0", b"shared payload");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    push_with(
        &store,
        "lab:1.0.0",
        target(&registry, "a/lab:1.0.0"),
        &entry,
        &client,
        &PushOptions::default(),
    )
    .unwrap();
    registry.clear_log();
    let options = PushOptions {
        mount_from: Some("a/lab".into()),
        ..PushOptions::default()
    };
    let report = push_with(
        &store,
        "lab:1.0.0",
        target(&registry, "b/lab:1.0.0"),
        &entry,
        &client,
        &options,
    )
    .unwrap();
    assert!(
        report.blobs.iter().all(|(_, o)| *o == BlobOutcome::Mounted),
        "{report:?}"
    );
    assert!(!registry
        .log()
        .iter()
        .any(|l| l.starts_with("PUT /v2/b/lab/blobs/uploads/")));
}

fn user_policy() -> Policy {
    Policy {
        user: Some(("maintainer".into(), "synthetic-secret".into())),
        ..Policy::default()
    }
}

fn maintainer() -> Credentials {
    Credentials::new("maintainer", "synthetic-secret")
}

/// A vendor ROM imported here is user-supplied. An image pulled from another
/// registry that labels the same bytes open must not carry them out.
#[test]
fn a_blob_another_local_image_records_as_user_supplied_stays_restricted() {
    let dir = Temp::new();
    let store = dir.store();
    image(&store, "rom:1.0.0", &[(b"vendor rom", None)], Some("open"));

    let elsewhere = Temp::new();
    let third_party = elsewhere.store();
    image(
        &third_party,
        "game:1.0.0",
        &[(b"vendor rom", Some("open")), (b"game data", Some("open"))],
        Some("open"),
    );
    let other = MockRegistry::start(open_policy());
    let other_entry = other.entry(false);
    let other_client = client(&other_entry, Anonymous);
    let source = target(&other, "vendor/game:1.0.0");
    push_with(
        &third_party,
        "game:1.0.0",
        source.clone(),
        &other_entry,
        &other_client,
        &PushOptions::default(),
    )
    .unwrap();
    pull::pull(
        &store,
        &other_client,
        &source,
        "game:1.0.0",
        &PROVIDERS,
        &|_| {},
    )
    .unwrap();

    let rom = Digest::of(b"vendor rom");
    let public = MockRegistry::start(open_policy());
    let public_entry = public.entry(false);
    let plan = push::plan(
        &store,
        "game:1.0.0",
        target(&public, "owner/game:1.0.0"),
        Some(&public_entry),
    )
    .unwrap();
    let entry = plan
        .closure()
        .iter()
        .find(|c| c.descriptor.digest == rom)
        .unwrap();
    assert_eq!(
        (entry.mark, entry.effective),
        (Redistribution::Open, Redistribution::UserSupplied)
    );
    assert_eq!(entry.source, MarkSource::LocalTag("rom:1.0.0".into()));
    assert!(
        entry
            .to_string()
            .ends_with("\tuser-supplied\tlocal tag rom:1.0.0"),
        "{entry}"
    );
    assert!(plan
        .closure()
        .iter()
        .filter(|c| c.descriptor.digest != rom)
        .all(|c| c.effective == Redistribution::Open && c.source == MarkSource::ThisImage));
    let refused = push::push(
        &store,
        &plan,
        &PROVIDERS,
        &client(&public_entry, Anonymous),
        &PushOptions::default(),
        &mut |_, _| {},
    );
    assert!(
        matches!(&refused, Err(Error::Refused(Refusal::RegistryNotPrivate { blobs, .. }))
            if blobs.iter().map(|d| &d.digest).eq([&rom])),
        "{refused:?}"
    );
    let official = push_with(
        &store,
        "game:1.0.0",
        target(&public, "unemu/game:1.0.0"),
        &public_entry,
        &client(&public_entry, Anonymous),
        &PushOptions::default(),
    );
    assert!(
        matches!(
            official,
            Err(Error::Refused(Refusal::OfficialNamespaceNeedsOpen { .. }))
        ),
        "{official:?}"
    );
    assert!(public.log().is_empty(), "{:?}", public.log());

    // Allowed: the same image into a private repository on a private
    // registry, and an image whose digests every local manifest marks open.
    let private = MockRegistry::start(user_policy());
    let private_entry = private.entry(true);
    push_with(
        &store,
        "game:1.0.0",
        target(&private, "private/game:1.0.0"),
        &private_entry,
        &client(&private_entry, maintainer()),
        &PushOptions::default(),
    )
    .unwrap();
    assert!(private.has_blob("private/game", &rom));
    open_image(&store, "tool:1.0.0", b"open tool");
    let report = push_with(
        &store,
        "tool:1.0.0",
        target(&public, "owner/tool:1.0.0"),
        &public_entry,
        &client(&public_entry, Anonymous),
        &PushOptions::default(),
    )
    .unwrap();
    assert_eq!(report.tag, TagOutcome::Created);
}

#[test]
fn a_reference_with_both_a_tag_and_a_digest_is_refused() {
    let dir = Temp::new();
    let store = dir.store();
    let image = open_image(&store, "lab:1.0.0", b"payload");
    let registry = MockRegistry::start(open_policy());
    let reference = Reference::parse(
        &format!("{}/o/lab:1.0.0@{}", registry.host(), image.digest),
        "unused.example",
    )
    .unwrap();
    let refused = push::plan(&store, "lab:1.0.0", reference, Some(&registry.entry(false)));
    assert!(
        matches!(&refused, Err(Error::Reference(message)) if message.contains("both a tag and a digest")),
        "{refused:?}"
    );
}

#[test]
fn environment_credentials_never_reach_another_configured_host() {
    let bound = MockRegistry::start(user_policy());
    let other = MockRegistry::start(user_policy());
    for registry in [&bound, &other] {
        registry.seed_manifest("private/lab", "1", b"{}");
    }
    let env = EnvCredentials::new(&bound.host(), "maintainer", "synthetic-secret");
    let source = |env| HostCredentials {
        configured_user: None,
        env,
        store: std::sync::Arc::new(crate::credential_store::SessionStore::default()),
    };
    let refused = pull::list_tags(
        &client(&other.entry(true), source(env.clone())),
        "private/lab",
        10,
    );
    assert!(
        matches!(refused, Err(Error::AuthRequired { .. })),
        "{refused:?}"
    );
    assert!(
        other.authorized_log().is_empty(),
        "{:?}",
        other.authorized_log()
    );
    let listed = pull::list_tags(&client(&bound.entry(true), source(env)), "private/lab", 10);
    assert_eq!(listed.unwrap(), ["1"]);
    assert!(!bound.authorized_log().is_empty());
}

#[test]
fn a_bearer_challenge_leaves_anonymous_access_unverified_and_refuses_restricted_pushes() {
    let dir = Temp::new();
    let store = dir.store();
    image(&store, "rom:1.0.0", &[(b"vendor rom", None)], None);
    let registry = MockRegistry::start(Policy {
        bearer_challenge: true,
        ..user_policy()
    });
    let entry = registry.entry(true);
    assert_eq!(
        client(&entry, Anonymous)
            .anonymous_access("private/rom")
            .unwrap(),
        AnonymousAccess::Inconclusive
    );
    let refused = push_with(
        &store,
        "rom:1.0.0",
        target(&registry, "private/rom:1.0.0"),
        &entry,
        &client(&entry, maintainer()),
        &PushOptions::default(),
    );
    let message = refused.as_ref().map(|_| ()).unwrap_err().to_string();
    assert!(
        matches!(
            refused,
            Err(Error::Refused(Refusal::AnonymousAccessUnverified { .. }))
        ),
        "{message}"
    );
    assert!(
        message.contains("Bearer authentication is not supported yet"),
        "{message}"
    );
    assert_no_upload(&registry);

    let basic = MockRegistry::start(user_policy());
    assert_eq!(
        client(&basic.entry(true), Anonymous)
            .anonymous_access("private/rom")
            .unwrap(),
        AnonymousAccess::Denied
    );
}

fn payload_image(store: &Store, tag: &str, size: usize) -> (Descriptor, Vec<u8>) {
    let payload: Vec<u8> = (0..size).map(|i| (i * 31 % 253) as u8).collect();
    let descriptor = image(store, tag, &[(&payload, Some("open"))], Some("open"));
    (descriptor, payload)
}

fn published(size: usize) -> (Temp, MockRegistry, Client, Reference, Descriptor, Digest) {
    let source = Temp::new();
    let store = source.store();
    let (image, payload) = payload_image(&store, "big:1.0.0", size);
    let registry = MockRegistry::start(open_policy());
    let client = client(&registry.entry(false), Anonymous);
    let reference = target(&registry, "owner/big:1.0.0");
    push_with(
        &store,
        "big:1.0.0",
        reference.clone(),
        &registry.entry(false),
        &client,
        &PushOptions::default(),
    )
    .unwrap();
    (
        source,
        registry,
        client,
        reference,
        image,
        Digest::of(&payload),
    )
}

#[test]
fn a_download_cut_mid_blob_resumes_by_range_and_still_verifies() {
    let size = 300_000;
    let (_source, registry, client, reference, image, payload) = published(size);
    for keep in [100_000, 50_000] {
        registry.inject_blob_fault(BlobFault {
            digest: payload.clone(),
            keep,
        });
    }
    let fresh = Temp::new();
    let store = fresh.store();
    pull::pull(
        &store,
        &client,
        &reference,
        "big:1.0.0",
        &PROVIDERS,
        &|_| {},
    )
    .unwrap();
    assert_eq!(
        store.tag_descriptor("big:1.0.0").unwrap().digest,
        image.digest
    );
    store.verify(&payload, size as u64).unwrap();
    assert_eq!(
        registry.range_requests(),
        ["bytes=100000-", "bytes=150000-"]
    );

    for _ in 0..=pull::DOWNLOAD_RETRIES {
        registry.inject_blob_fault(BlobFault {
            digest: payload.clone(),
            keep: 10_000,
        });
    }
    let exhausted = Temp::new();
    let store = exhausted.store();
    let failed = pull::pull(
        &store,
        &client,
        &reference,
        "big:1.0.0",
        &PROVIDERS,
        &|_| {},
    );
    assert!(matches!(failed, Err(Error::Transport(_))), "{failed:?}");
    assert!(matches!(
        store.open_blob(&payload, size as u64),
        Err(image_store::Error::MissingBlob { .. })
    ));
    assert!(store.tag_descriptor("big:1.0.0").is_err());
    assert_eq!(registry.range_requests().len(), 2 + 3);
}

#[test]
fn a_whole_blob_answer_restarts_the_download_and_a_wrong_range_is_refused() {
    let size = 200_000;
    let (_source, registry, client, reference, image, payload) = published(size);
    registry.set_range_support(RangeSupport::Ignore);
    registry.inject_blob_fault(BlobFault {
        digest: payload.clone(),
        keep: 70_000,
    });
    let fresh = Temp::new();
    let store = fresh.store();
    pull::pull(
        &store,
        &client,
        &reference,
        "big:1.0.0",
        &PROVIDERS,
        &|_| {},
    )
    .unwrap();
    assert_eq!(
        store.tag_descriptor("big:1.0.0").unwrap().digest,
        image.digest
    );
    store.verify(&payload, size as u64).unwrap();
    assert_eq!(registry.range_requests(), ["bytes=70000-"]);

    registry.set_range_support(RangeSupport::Misreport);
    registry.inject_blob_fault(BlobFault {
        digest: payload.clone(),
        keep: 70_000,
    });
    let other = Temp::new();
    let store = other.store();
    let refused = pull::pull(
        &store,
        &client,
        &reference,
        "big:1.0.0",
        &PROVIDERS,
        &|_| {},
    );
    assert!(matches!(refused, Err(Error::Protocol(_))), "{refused:?}");
    assert!(store.tag_descriptor("big:1.0.0").is_err());
}

#[test]
fn credentials_never_follow_a_redirect_to_another_port() {
    let dir = Temp::new();
    let store = dir.store();
    let image = open_image(&store, "lab:1.0.0", b"redirected payload");
    let origin = MockRegistry::start(user_policy());
    let entry = origin.entry(true);
    let authorized = client(&entry, maintainer());
    let reference = target(&origin, "private/lab:1.0.0");
    push_with(
        &store,
        "lab:1.0.0",
        reference.clone(),
        &entry,
        &authorized,
        &PushOptions::default(),
    )
    .unwrap();
    let storage = MockRegistry::start(Policy {
        anonymous_read: vec!["*".into()],
        ..Policy::default()
    });
    let manifest =
        Manifest::decode_for_summary(&store.read_verified(&image, MAX_SMALL_BLOB).unwrap())
            .unwrap();
    for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
        storage.seed_blob(
            "private/lab",
            &store.read_verified(descriptor, MAX_SMALL_BLOB).unwrap(),
        );
    }
    origin.redirect_blobs_to(format!("http://{}", storage.host()), "");
    let fresh = Temp::new();
    pull::pull(
        &fresh.store(),
        &authorized,
        &reference,
        "lab:1.0.0",
        &PROVIDERS,
        &|_| {},
    )
    .unwrap();
    assert!(storage
        .log()
        .iter()
        .any(|l| l.starts_with("GET /v2/private/lab/blobs/")));
    assert!(
        storage.authorized_log().is_empty(),
        "{:?}",
        storage.authorized_log()
    );
    assert!(origin
        .authorized_log()
        .iter()
        .any(|l| l.starts_with("GET /v2/private/lab/blobs/")));
}

#[test]
fn errors_name_a_refused_redirect_without_its_query() {
    let dir = Temp::new();
    let store = dir.store();
    open_image(&store, "lab:1.0.0", b"payload");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    let reference = target(&registry, "o/lab:1.0.0");
    push_with(
        &store,
        "lab:1.0.0",
        reference.clone(),
        &entry,
        &client,
        &PushOptions::default(),
    )
    .unwrap();
    registry.redirect_blobs_to(
        "http://storage.example/bucket",
        "?X-Amz-Signature=synthetic-signature",
    );
    let fresh = Temp::new();
    let failed = pull::pull(
        &fresh.store(),
        &client,
        &reference,
        "lab:1.0.0",
        &PROVIDERS,
        &|_| {},
    );
    let message = failed.unwrap_err().to_string();
    assert!(
        message.contains("http://storage.example/bucket/v2/o/lab/blobs/"),
        "{message}"
    );
    assert!(
        !message.contains("X-Amz") && !message.contains("synthetic-signature"),
        "{message}"
    );
}

#[test]
fn relative_upload_locations_resolve_against_the_request() {
    let dir = Temp::new();
    let store = dir.store();
    let (image, payload) = large_image(&store, "big:1.0.0");
    let registry = MockRegistry::start(open_policy());
    registry.use_relative_locations();
    let entry = registry.entry(false);
    push_with(
        &store,
        "big:1.0.0",
        target(&registry, "owner/big:1.0.0"),
        &entry,
        &client(&entry, Anonymous),
        &chunked_options(3),
    )
    .unwrap();
    assert!(registry.has_blob("owner/big", &Digest::of(&payload)));
    assert_eq!(
        registry.tag_digest("owner/big", "1.0.0"),
        Some(image.digest)
    );
    let patches = registry
        .log()
        .into_iter()
        .filter(|l| l.starts_with("PATCH "))
        .count();
    assert_eq!(patches, 3);
}

#[test]
fn a_session_holding_one_byte_resumes_at_offset_one() {
    let dir = Temp::new();
    let store = dir.store();
    let (_, payload) = large_image(&store, "big:1.0.0");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    registry.inject_patch_fault(PatchFault {
        nth: 1,
        keep: 1,
        drop_session: false,
    });
    push_with(
        &store,
        "big:1.0.0",
        target(&registry, "owner/big:1.0.0"),
        &entry,
        &client(&entry, Anonymous),
        &chunked_options(3),
    )
    .unwrap();
    let log = registry.log();
    let posts = log
        .iter()
        .filter(|l| l.starts_with("POST ") && l.contains("/owner/big/"))
        .count();
    assert_eq!(posts, 3, "one session per blob, none restarted: {log:?}");
    assert!(registry.has_blob("owner/big", &Digest::of(&payload)));
}

/// Some registries report an empty session as `0-0`, the same as one byte.
/// The chunk at offset 1 is then refused, and the upload starts over.
#[test]
fn a_rejected_resume_offset_restarts_the_upload_in_a_new_session() {
    let dir = Temp::new();
    let store = dir.store();
    let (_, payload) = large_image(&store, "big:1.0.0");
    let registry = MockRegistry::start(open_policy());
    registry.report_empty_sessions_as_one_byte();
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    registry.inject_patch_fault(PatchFault {
        nth: 1,
        keep: 0,
        drop_session: false,
    });
    let push = |retries| {
        push_with(
            &store,
            "big:1.0.0",
            target(&registry, "owner/big:1.0.0"),
            &entry,
            &client,
            &chunked_options(retries),
        )
    };
    assert!(matches!(push(0), Err(Error::Transport(_))));
    assert_eq!(UploadSessions::for_store(&store).count(), 1);
    push(1).unwrap();
    assert!(registry.has_blob("owner/big", &Digest::of(&payload)));
    assert_eq!(UploadSessions::for_store(&store).count(), 0);
}

#[test]
fn a_persisted_session_is_dropped_when_the_blob_is_already_present() {
    let dir = Temp::new();
    let store = dir.store();
    let (_, payload) = large_image(&store, "big:1.0.0");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    registry.inject_patch_fault(PatchFault {
        nth: 2,
        keep: 0,
        drop_session: false,
    });
    let push = || {
        push_with(
            &store,
            "big:1.0.0",
            target(&registry, "owner/big:1.0.0"),
            &entry,
            &client,
            &chunked_options(0),
        )
    };
    assert!(push().is_err());
    assert_eq!(UploadSessions::for_store(&store).count(), 1);
    let big = registry.seed_blob("owner/big", &payload);
    let report = push().unwrap();
    assert!(report
        .blobs
        .iter()
        .any(|(d, o)| d.digest == big && *o == BlobOutcome::Present));
    assert_eq!(UploadSessions::for_store(&store).count(), 0);
}

/// Opt-in round trip against a real registry: push a synthetic open image to
/// `unemu/test-<random>` with credentials from the environment, pull it
/// anonymously into a fresh store, compare digests, then delete it.
#[test]
#[ignore = "live registry: set UNEMU_REGISTRY_LIVE=1, UNEMU_REGISTRY_USER and UNEMU_REGISTRY_PASSWORD"]
fn live_round_trip_through_the_default_registry() {
    if std::env::var("UNEMU_REGISTRY_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: UNEMU_REGISTRY_LIVE is not 1");
        return;
    }
    let host = std::env::var(crate::config::DEFAULT_REGISTRY_ENV)
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| crate::DEFAULT_REGISTRY.into());
    let credentials = crate::credentials::HostCredentials {
        configured_user: None,
        env: crate::credentials::EnvCredentials::from_env(),
        store: std::sync::Arc::new(crate::credential_store::SessionStore::default()),
    }
    .credentials(&host)
    .unwrap()
    .expect(
        "UNEMU_REGISTRY_USER and UNEMU_REGISTRY_PASSWORD are required, with \
         UNEMU_REGISTRY_HOST when UNEMU_REGISTRY names another host",
    );
    let suffix = {
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u32(std::process::id());
        format!("{:012x}", hasher.finish() & 0xffff_ffff_ffff)
    };
    let repository = format!("unemu/test-{suffix}");
    let source = Temp::new();
    let store = source.store();
    let size = MIN_CHUNK_SIZE as usize + 4096;
    let payload: Vec<u8> = (0..size).map(|i| (i * 13 % 241) as u8).collect();
    let image = image(
        &store,
        "live:1.0.0",
        &[
            (&payload, Some("open")),
            (b"small open payload", Some("open")),
        ],
        Some("open"),
    );
    let entry = RegistryEntry::builtin(&host);
    let writer = client(&entry, credentials);
    let reference = Reference::parse(&format!("{host}/{repository}:1.0.0"), &host).unwrap();
    let plan = push::plan(&store, "live:1.0.0", reference.clone(), Some(&entry)).unwrap();
    let pushed = push::push(
        &store,
        &plan,
        &PROVIDERS,
        &writer,
        &chunked_options(3),
        &mut |_, _| {},
    );
    let pulled = pushed.as_ref().ok().map(|_| {
        let fresh = Temp::new();
        let target = fresh.store();
        let reader = client(&entry, Anonymous);
        let report = pull::pull(
            &target,
            &reader,
            &reference,
            "live:1.0.0",
            &PROVIDERS,
            &|_| {},
        );
        (report.map(|r| r.image), fresh)
    });
    let _ = writer.delete_manifest(&repository, &image.digest);
    for descriptor in std::iter::once(&plan.manifest().config).chain(&plan.manifest().layers) {
        let _ = writer.delete_blob(&repository, &descriptor.digest);
    }
    pushed.unwrap();
    let (pulled, _fresh) = pulled.unwrap();
    assert_eq!(pulled.unwrap(), image);
}

/// A synthetic platform whose firmware and disk payloads may be `+zstd`.
struct Compressed;
const COMPRESSED_TYPES: &[image_store::PayloadType] = &[
    image_store::PayloadType {
        media_type: "application/vnd.unemu.firmware.v1+zstd",
        max_size: 16 * 1024 * 1024,
    },
    image_store::PayloadType {
        media_type: "application/vnd.unemu.disk.base.v1+zstd",
        max_size: 1 << 40,
    },
];
impl ConfigValidator for Compressed {
    fn platform(&self) -> &str {
        "synthetic-zstd"
    }
    fn payload_types(&self) -> &[image_store::PayloadType] {
        COMPRESSED_TYPES
    }
    fn validate(
        &self,
        table: &toml::Table,
        descriptors: &[Descriptor],
        blobs: &dyn BlobSource,
    ) -> image_store::Result<Vec<Digest>> {
        Synthetic.validate(table, descriptors, blobs)
    }
}
const COMPRESSED: [&dyn ConfigValidator; 1] = [&Compressed];

/// Compress `bytes` as `kind`, then mark it.
fn compressed(store: &Store, kind: &str, bytes: &[u8], mark: &str) -> Descriptor {
    let mut d = store
        .import_payload(bytes, kind, bytes.len() as u64, None, COMPRESSED_TYPES)
        .unwrap();
    assert!(d.is_compressed());
    d.annotations
        .insert("unemu.redistribution".into(), mark.into());
    d
}

/// Tag a compressed image: a user-supplied ROM and an open disk base.
fn compressed_image(store: &Store, tag: &str, rom: &[u8], disk: &[u8]) -> Descriptor {
    let layers = vec![
        compressed(store, mt::FIRMWARE, rom, "user-supplied"),
        compressed(store, mt::DISK_BASE, disk, "open"),
    ];
    let blobs = layers
        .iter()
        .map(|d| format!("\"{}\"", d.decoded_digest().unwrap()))
        .collect::<Vec<_>>()
        .join(",");
    let config = marked(
        store,
        mt::CONFIG,
        format!("schema = 1\nplatform = \"synthetic-zstd\"\nblobs = [{blobs}]\n").as_bytes(),
        Some("open"),
    );
    let metadata = marked(
        store,
        mt::METADATA,
        br#"{"schema":1,"platform":"synthetic-zstd","titles":[]}"#,
        Some("open"),
    );
    let mut all = layers;
    all.push(metadata);
    let manifest =
        Manifest::new_with_types("synthetic-zstd", config, all, COMPRESSED_TYPES).unwrap();
    let descriptor = store
        .import_manifest_with_types(&manifest, COMPRESSED_TYPES)
        .unwrap();
    store
        .update_tag(tag, &descriptor, None, &Compressed)
        .unwrap();
    descriptor
}

fn blob_bytes(store: &Store, d: &Descriptor) -> Vec<u8> {
    std::fs::read(store.blob_file_path(&d.digest, d.size).unwrap()).unwrap()
}

#[test]
fn compressed_images_travel_as_stored_blobs_and_pull_back_identical() {
    let dir = Temp::new();
    let store = dir.store();
    let rom: Vec<u8> = (0..32 * 1024_u32).map(|i| (i % 97) as u8).collect();
    let disk = vec![0_u8; 1024 * 1024];
    let image = compressed_image(&store, "game:1.0.0", &rom, &disk);
    let manifest = Manifest::decode_with_types(
        &store.read_verified(&image, MAX_SMALL_BLOB).unwrap(),
        COMPRESSED_TYPES,
    )
    .unwrap();
    let rom_layer = manifest.layers[0].clone();

    // The ROM is user-supplied, so the official namespace refuses the image.
    let public = MockRegistry::start(Policy {
        anonymous_read: vec!["unemu".into()],
        anonymous_write: vec!["*".into()],
        ..Policy::default()
    });
    let public_entry = public.entry(true);
    let plan = push::plan(
        &store,
        "game:1.0.0",
        target(&public, "unemu/game:1.0.0"),
        Some(&public_entry),
    )
    .unwrap();
    let refused = push::push(
        &store,
        &plan,
        &COMPRESSED,
        &client(&public_entry, Anonymous),
        &PushOptions::default(),
        &mut |_, _| {},
    );
    assert!(
        matches!(&refused, Err(Error::Refused(Refusal::OfficialNamespaceNeedsOpen { blobs, .. }))
            if blobs.iter().map(|d| &d.digest).eq([&rom_layer.digest])),
        "{refused:?}"
    );
    assert!(public.log().is_empty(), "{:?}", public.log());

    // A private registry receives exactly the stored frames.
    let private = MockRegistry::start(user_policy());
    let entry = private.entry(true);
    let client = client(&entry, maintainer());
    let reference = target(&private, "private/game:1.0.0");
    let plan = push::plan(&store, "game:1.0.0", reference.clone(), Some(&entry)).unwrap();
    push::push(
        &store,
        &plan,
        &COMPRESSED,
        &client,
        &PushOptions::default(),
        &mut |_, _| {},
    )
    .unwrap();
    for d in &manifest.layers {
        assert!(private.has_blob("private/game", &d.digest));
        if d.is_compressed() {
            assert!(!private.has_blob("private/game", &d.decoded_digest().unwrap()));
        }
    }
    // Verification before push streamed; nothing was decoded to disk.
    assert!(!store.root().join("cache/materialized").exists());

    let fresh = Temp::new();
    let pulled = fresh.store();
    let report = pull::pull(
        &pulled,
        &client,
        &reference,
        "game:1.0.0",
        &COMPRESSED,
        &|_| {},
    )
    .unwrap();
    assert_eq!(report.outcome, pull::PullOutcome::Pulled);
    assert_eq!(
        pulled.tag_descriptor("game:1.0.0").unwrap().digest,
        image.digest
    );
    for d in std::iter::once(&manifest.config).chain(&manifest.layers) {
        assert_eq!(blob_bytes(&pulled, d), blob_bytes(&store, d));
    }
    let resolved = pulled.resolve("game:1.0.0", &Compressed).unwrap();
    let mut decoded = Vec::new();
    std::io::Read::read_to_end(
        &mut resolved
            .content_reader(&rom_layer.decoded_digest().unwrap())
            .unwrap(),
        &mut decoded,
    )
    .unwrap();
    assert_eq!(decoded, rom);
    assert_eq!(
        resolved.manifest.layers[0].annotations["unemu.redistribution"],
        "user-supplied"
    );
    assert!(!pulled.root().join("cache/materialized").exists());

    let mut cache = image_store::summary::SummaryCache::open(&pulled).unwrap();
    cache.rebuild(&pulled, 64, &[], &[]).unwrap();
    let row = cache.summary(&image.digest).unwrap();
    let expected = image.size
        + manifest.config.size
        + rom.len() as u64
        + disk.len() as u64
        + manifest.layers[2].size;
    assert_eq!(row.decoded_size, expected);
    assert!(row.stored_size < row.decoded_size / 4, "{row:?}");
    assert!(row
        .storage_blobs
        .iter()
        .any(|d| d.digest == rom_layer.digest));
}

/// The same ROM stored raw and user-supplied in one image restricts a
/// compressed copy that another image labels open.
#[test]
fn a_compressed_copy_shares_the_class_of_its_raw_content() {
    let dir = Temp::new();
    let store = dir.store();
    let rom: Vec<u8> = (0..32 * 1024_u32).map(|i| (i % 89) as u8).collect();
    image(
        &store,
        "rom:1.0.0",
        &[(&rom, Some("user-supplied"))],
        Some("open"),
    );
    let mut layer = compressed(&store, mt::FIRMWARE, &rom, "open");
    let config = marked(
        &store,
        mt::CONFIG,
        format!(
            "schema = 1\nplatform = \"synthetic-zstd\"\nblobs = [\"{}\"]\n",
            layer.decoded_digest().unwrap()
        )
        .as_bytes(),
        Some("open"),
    );
    let metadata = marked(
        &store,
        mt::METADATA,
        br#"{"schema":1,"platform":"synthetic-zstd","titles":[]}"#,
        Some("open"),
    );
    layer
        .annotations
        .insert("unemu.redistribution".into(), "open".into());
    let manifest = Manifest::new_with_types(
        "synthetic-zstd",
        config,
        vec![layer.clone(), metadata],
        COMPRESSED_TYPES,
    )
    .unwrap();
    let descriptor = store
        .import_manifest_with_types(&manifest, COMPRESSED_TYPES)
        .unwrap();
    store
        .update_tag("labelled:1.0.0", &descriptor, None, &Compressed)
        .unwrap();
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let plan = push::plan(
        &store,
        "labelled:1.0.0",
        target(&registry, "owner/labelled:1.0.0"),
        Some(&entry),
    )
    .unwrap();
    let rom_entry = plan
        .closure()
        .iter()
        .find(|c| c.descriptor.digest == layer.digest)
        .unwrap();
    assert_eq!(
        (rom_entry.mark, rom_entry.effective),
        (Redistribution::Open, Redistribution::UserSupplied)
    );
    assert_eq!(rom_entry.source, MarkSource::LocalTag("rom:1.0.0".into()));
    assert!(matches!(
        plan.verdict(),
        Err(Refusal::RegistryNotPrivate { .. })
    ));
}
