// SPDX-License-Identifier: AGPL-3.0-or-later
//! Push, pull, guard and listing against the in-process mock registry, plus an
//! opt-in live round trip. Synthetic images only; no emulator or guest data.

use crate::{
    client::{AnonymousAccess, Client, ACCEPT_ANY_MANIFEST},
    config::Namespace,
    credentials::{Anonymous, CredentialSource, Credentials, EnvCredentials, HostCredentials},
    guard::{MarkSource, Redistribution, Refusal},
    http::{Body, Method},
    pull, push,
    push::{BlobOutcome, PushOptions, PushProgress, TagOutcome, UploadSessions, MIN_CHUNK_SIZE},
    test_support::{BlobFault, MockRegistry, PatchFault, Policy, RangeSupport},
    Error, Reference, RegistryEntry, DEFAULT_REGISTRY,
};
use image_store::{
    media_set::{self, MediaFormats},
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

/// An open mark carries a permissive licence, as the guard requires of it.
fn set_mark(descriptor: &mut Descriptor, mark: &str) {
    descriptor
        .annotations
        .insert("unemu.redistribution".into(), mark.into());
    if mark == "open" {
        descriptor
            .annotations
            .insert("unemu.license".into(), "MIT".into());
    }
}

fn marked(store: &Store, kind: &str, bytes: &[u8], mark: Option<&str>) -> Descriptor {
    let blob = store.import(bytes, None).unwrap();
    let mut descriptor = Descriptor::new(kind, blob.digest, blob.size);
    if let Some(mark) = mark {
        set_mark(&mut descriptor, mark);
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

/// Neutral installation media: every medium is a floppy.
struct Floppies;
impl MediaFormats for Floppies {
    fn platform(&self) -> &str {
        "synthetic"
    }
    fn validate_name(&self, _: &str) -> image_store::Result<()> {
        Ok(())
    }
    fn validate_format(&self, _: &toml::Table, _: u64) -> image_store::Result<&'static str> {
        Ok(mt::FLOPPY)
    }
}
const MEDIA_SETS: media_set::Validator<'static> = media_set::Validator(&Floppies);

/// Tag an open installation media set of one floppy holding `label`.
fn open_media_set(store: &Store, tag: &str, label: &[u8]) -> Descriptor {
    let medium = marked(store, mt::FLOPPY, label, Some("open"));
    let config = media_set::Config {
        schema: 1,
        platform: "synthetic".into(),
        media: vec![media_set::Medium {
            id: "disk-1".into(),
            label: "Disk 1".into(),
            format: toml::Table::new(),
            blob: medium.digest.clone(),
        }],
        extras: Vec::new(),
    };
    let config = marked(
        store,
        mt::MEDIA_SET_CONFIG,
        &config.encode().unwrap(),
        Some("open"),
    );
    let metadata = marked(
        store,
        mt::METADATA,
        br#"{"schema":1,"platform":"synthetic","titles":[]}"#,
        Some("open"),
    );
    let manifest = Manifest::new_media_set("synthetic", config, vec![medium, metadata]).unwrap();
    let descriptor = store.import_manifest(&manifest).unwrap();
    store
        .update_tag(tag, &descriptor, None, &MEDIA_SETS)
        .unwrap();
    descriptor
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

/// A registry may answer `404` for a tag whose manifest type the request does
/// not accept, as distribution/registry does for an OCI image index. The push
/// preflight and its final recheck accept every manifest and index type, so a
/// tag another tool pointed at an index or a Docker manifest is never taken for
/// a new tag and replaced without confirmation.
#[test]
fn a_tag_naming_an_index_or_a_docker_manifest_moves_only_when_confirmed() {
    let dir = Temp::new();
    let store = dir.store();
    let image = open_image(&store, "lab:2.0.0", b"image");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    let moving = PushOptions {
        allow_move: true,
        ..PushOptions::default()
    };
    for (i, media_type) in [
        mt::INDEX,
        "application/vnd.docker.distribution.manifest.v2+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
    ]
    .into_iter()
    .enumerate()
    {
        let tag = format!("other-{i}");
        let body = format!(r#"{{"schemaVersion":2,"mediaType":"{media_type}"}}"#);
        let foreign = registry.seed_manifest_as("o/lab", &tag, media_type, body.as_bytes());
        let reference = || target(&registry, &format!("o/lab:{tag}"));
        let refused = push_with(
            &store,
            "lab:2.0.0",
            reference(),
            &entry,
            &client,
            &PushOptions::default(),
        );
        assert!(
            matches!(&refused, Err(Error::TagMoveNeedsConfirmation { current, new, .. })
                if *current == foreign && *new == image.digest),
            "{media_type}: {refused:?}"
        );
        assert_eq!(registry.tag_digest("o/lab", &tag), Some(foreign.clone()));
        // Only the Accept header decides whether the registry shows the tag.
        let path = format!("/v2/o/lab/manifests/{tag}");
        let head = |accept: &str| {
            let headers = [("Accept", accept.to_owned())];
            client
                .send(Method::Head, &path, &headers, &Body::Empty)
                .unwrap()
                .status
        };
        assert_eq!(head(mt::MANIFEST), 404, "{media_type}");
        assert_eq!(head(ACCEPT_ANY_MANIFEST), 200, "{media_type}");
        let moved = push_with(&store, "lab:2.0.0", reference(), &entry, &client, &moving).unwrap();
        assert_eq!(
            moved.tag,
            TagOutcome::Moved { from: foreign },
            "{media_type}"
        );
        assert_eq!(
            registry.tag_digest("o/lab", &tag),
            Some(image.digest.clone())
        );
    }
    // A version tag naming an index never moves, as any version tag.
    let foreign = registry.seed_manifest_as("o/lab", "1.0.0", mt::INDEX, br#"{"schemaVersion":2}"#);
    let refused = push_with(
        &store,
        "lab:2.0.0",
        target(&registry, "o/lab:1.0.0"),
        &entry,
        &client,
        &moving,
    );
    assert!(
        matches!(&refused, Err(Error::ImmutableTag { current, .. }) if *current == foreign),
        "{refused:?}"
    );
    assert_eq!(registry.tag_digest("o/lab", "1.0.0"), Some(foreign));
}

/// The tags of installation media sets and firmware packages on a
/// registry: a revision tag (`[<release>-]rN`) never moves, with or without
/// `--move`, whichever side the set is on; the release alias moves only
/// forward, to the newest revision of its release the registry already has,
/// and only when confirmed; no `latest`. Machine images keep the version-tag
/// rule, and their other tags move when confirmed.
#[test]
fn revision_tags_never_move_on_the_registry_and_aliases_move_only_forward() {
    let dir = Temp::new();
    let store = dir.store();
    let first = open_media_set(&store, "sets:1.4-r1", b"first set");
    let second = open_media_set(&store, "sets:1.4-r2", b"second set");
    let other = open_media_set(&store, "sets:1.5-r1", b"other release");
    let machine = open_image(&store, "lab:1.4", b"machine");
    let newer_machine = open_image(&store, "lab:1.5", b"other machine");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(true);
    let client = client(&entry, Anonymous);
    let providers: [&dyn ConfigValidator; 2] = [&Synthetic, &MEDIA_SETS];
    let send =
        |local: &str, path: &str, options: &PushOptions| -> crate::Result<push::PushReport> {
            let plan = push::plan(&store, local, target(&registry, path), Some(&entry))?;
            push::push(&store, &plan, &providers, &client, options, &mut |_, _| {})
        };
    let plain = PushOptions::default();
    let moving = PushOptions {
        allow_move: true,
        ..PushOptions::default()
    };
    // An alias waits for a revision of its release on the registry.
    let early = send("sets:1.4-r1", "private/sets:1.4", &plain);
    assert!(
        matches!(&early, Err(Error::AliasNotForward { .. })),
        "{early:?}"
    );
    send("sets:1.4-r1", "private/sets:1.4-r1", &plain).unwrap();
    send("sets:1.4-r1", "private/sets:1.4", &plain).unwrap();
    // Neither another set nor a machine image takes a revision tag; the
    // refusal offers the next revision.
    for (local, new) in [("sets:1.4-r2", &second), ("lab:1.4", &machine)] {
        for options in [&plain, &moving] {
            let refused = send(local, "private/sets:1.4-r1", options);
            assert!(
                matches!(&refused, Err(Error::ImmutableRevisionTag { current, new: pushed, .. })
                    if *current == first.digest && pushed == &new.digest),
                "{local}: {refused:?}"
            );
            let message = refused.unwrap_err().to_string();
            assert!(message.contains("such as 1.4-r2"), "{message}");
        }
    }
    // The alias moves only to the newest revision the registry has.
    let ahead = send("sets:1.4-r2", "private/sets:1.4", &moving);
    assert!(
        matches!(&ahead, Err(Error::AliasNotForward { .. })),
        "{ahead:?}"
    );
    send("sets:1.4-r2", "private/sets:1.4-r2", &plain).unwrap();
    let unconfirmed = send("sets:1.4-r2", "private/sets:1.4", &plain);
    assert!(
        matches!(unconfirmed, Err(Error::TagMoveNeedsConfirmation { .. })),
        "{unconfirmed:?}"
    );
    let moved = send("sets:1.4-r2", "private/sets:1.4", &moving).unwrap();
    assert_eq!(
        moved.tag,
        TagOutcome::Moved {
            from: first.digest.clone()
        }
    );
    // Never backward, never to another release.
    send("sets:1.5-r1", "private/sets:1.5-r1", &plain).unwrap();
    for local in ["sets:1.4-r1", "sets:1.5-r1"] {
        let refused = send(local, "private/sets:1.4", &moving);
        assert!(
            matches!(&refused, Err(Error::AliasNotForward { .. })),
            "{local}: {refused:?}"
        );
    }
    assert_eq!(
        registry.tag_digest("private/sets", "1.4"),
        Some(second.digest.clone())
    );
    assert_eq!(
        registry.tag_digest("private/sets", "1.4-r1"),
        Some(first.digest.clone())
    );
    assert_eq!(
        registry.tag_digest("private/sets", "1.5-r1"),
        Some(other.digest)
    );
    // No latest for a set, not even a new one.
    for options in [&plain, &moving] {
        let refused = send("sets:1.4-r2", "private/sets:latest", options);
        assert!(matches!(refused, Err(Error::NoLatest)), "{refused:?}");
    }
    assert_eq!(registry.tag_digest("private/sets", "latest"), None);
    // A machine image keeps its rules: a tag of the same shape moves when
    // confirmed, and `latest` too.
    for path in ["private/lab:1.4", "private/lab:latest"] {
        send("lab:1.4", path, &plain).unwrap();
        let unconfirmed = send("lab:1.5", path, &plain);
        assert!(
            matches!(unconfirmed, Err(Error::TagMoveNeedsConfirmation { .. })),
            "{unconfirmed:?}"
        );
        let moved = send("lab:1.5", path, &moving).unwrap();
        assert_eq!(
            moved.tag,
            TagOutcome::Moved {
                from: machine.digest.clone()
            }
        );
    }
    assert_eq!(
        registry.tag_digest("private/lab", "1.4"),
        Some(newer_machine.digest)
    );
}

/// A set pulled by the alias of its release is written under the revision
/// the registry's alias names, and the local alias follows it forward; a
/// pull of an older revision leaves the alias where it is.
#[test]
fn a_pull_writes_the_revision_and_moves_the_local_alias_forward() {
    let dir = Temp::new();
    let store = dir.store();
    let first = open_media_set(&store, "sets:1.4-r1", b"first set");
    let second = open_media_set(&store, "sets:1.4-r2", b"second set");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(true);
    let client = client(&entry, Anonymous);
    let providers: [&dyn ConfigValidator; 2] = [&Synthetic, &MEDIA_SETS];
    let moving = PushOptions {
        allow_move: true,
        ..PushOptions::default()
    };
    for (local, path) in [
        ("sets:1.4-r1", "private/sets:1.4-r1"),
        ("sets:1.4-r1", "private/sets:1.4"),
        ("sets:1.4-r2", "private/sets:1.4-r2"),
        ("sets:1.4-r2", "private/sets:1.4"),
    ] {
        let plan = push::plan(&store, local, target(&registry, path), Some(&entry)).unwrap();
        push::push(&store, &plan, &providers, &client, &moving, &mut |_, _| {}).unwrap();
    }
    let host = registry.host();
    let fresh = Temp::new();
    let pulled = fresh.store();
    let pull = |path: &str| {
        let reference = target(&registry, path);
        let local = reference.local_tag().unwrap();
        pull::pull(&pulled, &client, &reference, &local, &providers, &|_| {}).unwrap()
    };
    // An older revision first: the alias is new, so it names that one.
    let report = pull("private/sets:1.4-r1");
    assert_eq!(report.local_tag, format!("{host}/private/sets:1.4-r1"));
    let alias = format!("{host}/private/sets:1.4");
    assert_eq!(pulled.tag_descriptor(&alias).unwrap().digest, first.digest);
    // By the alias: the newest revision, and the local alias follows, which
    // the report says.
    let report = pull("private/sets:1.4");
    assert_eq!(report.local_tag, format!("{host}/private/sets:1.4-r2"));
    assert_eq!(report.image.digest, second.digest);
    assert_eq!(report.alias.as_deref(), Some(alias.as_str()));
    let named = |tag: &str| pulled.tag_descriptor(tag).unwrap();
    assert_eq!(named(&alias).digest, second.digest);
    assert_eq!(
        named(&alias).annotations.get(image_store::PULL_REGISTRY),
        Some(&host)
    );
    assert_eq!(
        named(&format!("{host}/private/sets:1.4-r1")).digest,
        first.digest
    );
    // An older revision pulled again never moves the alias back.
    pulled
        .remove_tag(&format!("{host}/private/sets:1.4-r1"), &first.digest)
        .unwrap();
    let report = pull("private/sets:1.4-r1");
    assert_eq!(report.alias, None);
    assert_eq!(named(&alias).digest, second.digest);
    // A revision already in place still brings its alias along.
    pulled.remove_tag(&alias, &second.digest).unwrap();
    let report = pull("private/sets:1.4");
    assert_eq!(report.outcome, pull::PullOutcome::Unchanged);
    assert_eq!(report.alias.as_deref(), Some(alias.as_str()));
    assert_eq!(named(&alias).digest, second.digest);
}

/// Seed the manifest and blobs of the local `image` under `repository:tag`,
/// as a registry holds what an earlier release published.
fn seed_published(
    store: &Store,
    registry: &MockRegistry,
    repository: &str,
    tag: &str,
    image: &Descriptor,
) {
    let bytes = store.read_verified(image, MAX_SMALL_BLOB).unwrap();
    let manifest = Manifest::decode_for_summary(&bytes).unwrap();
    for blob in std::iter::once(&manifest.config).chain(&manifest.layers) {
        registry.seed_blob(
            repository,
            &store.read_verified(blob, MAX_SMALL_BLOB).unwrap(),
        );
    }
    registry.seed_manifest(repository, tag, &bytes);
}

/// The set and package rules replace the version rule: a release shaped like
/// a semantic version is an alias that moves forward. A tag outside the
/// scheme is refused. An alias a registry published before revisions is
/// pulled as it is, and a push names it to be deleted rather than moved.
#[test]
fn version_shaped_releases_move_and_aliases_from_before_revisions_stay() {
    let dir = Temp::new();
    let store = dir.store();
    let first = open_media_set(&store, "sets:1.0.0-r1", b"first set");
    let second = open_media_set(&store, "sets:1.0.0-r2", b"second set");
    let rebuilt = open_media_set(&store, "old:1.4-r1", b"rebuilt set");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(true);
    let client = client(&entry, Anonymous);
    let providers: [&dyn ConfigValidator; 2] = [&Synthetic, &MEDIA_SETS];
    let send =
        |local: &str, path: &str, options: &PushOptions| -> crate::Result<push::PushReport> {
            let plan = push::plan(&store, local, target(&registry, path), Some(&entry))?;
            push::push(&store, &plan, &providers, &client, options, &mut |_, _| {})
        };
    let moving = PushOptions {
        allow_move: true,
        ..PushOptions::default()
    };
    for (local, path) in [
        ("sets:1.0.0-r1", "private/sets:1.0.0-r1"),
        ("sets:1.0.0-r1", "private/sets:1.0.0"),
        ("sets:1.0.0-r2", "private/sets:1.0.0-r2"),
    ] {
        send(local, path, &PushOptions::default()).unwrap();
    }
    let moved = send("sets:1.0.0-r2", "private/sets:1.0.0", &moving).unwrap();
    assert_eq!(
        moved.tag,
        TagOutcome::Moved {
            from: first.digest.clone()
        }
    );
    assert_eq!(
        registry.tag_digest("private/sets", "1.0.0"),
        Some(second.digest.clone())
    );
    let outside = send("sets:1.0.0-r2", "private/sets:Beta", &moving);
    assert!(
        matches!(&outside, Err(Error::OutsideTagScheme { tag }) if tag == "Beta"),
        "{outside:?}"
    );
    // The registry holds `old:1.4` as an earlier release published it, with
    // no revision.
    seed_published(&store, &registry, "private/old", "1.4", &first);
    send("old:1.4-r1", "private/old:1.4-r1", &moving).unwrap();
    let refused = send("old:1.4-r1", "private/old:1.4", &moving).unwrap_err();
    assert!(
        matches!(&refused, Error::AliasNotForward { .. })
            && refused.to_string().contains("delete 1.4 on the registry"),
        "{refused}"
    );
    assert_ne!(rebuilt.digest, first.digest);
    // A pull of that alias finds no revision naming its image and writes it
    // as it is.
    let fresh = Temp::new();
    let pulled = fresh.store();
    let reference = target(&registry, "private/old:1.4");
    let local = reference.local_tag().unwrap();
    let report = pull::pull(&pulled, &client, &reference, &local, &providers, &|_| {}).unwrap();
    assert_eq!(report.local_tag, local);
    assert_eq!(pulled.tag_descriptor(&local).unwrap().digest, first.digest);
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

/// The registry a pull fetched a tag from, as the store records it on the
/// tag's index descriptor; the name is part of the store format.
fn pulled_from(store: &Store, tag: &str) -> Option<String> {
    store
        .tag_descriptor(tag)
        .unwrap()
        .annotations
        .get("unemu.pull.registry")
        .cloned()
}

#[test]
fn a_pull_from_the_family_default_registry_keeps_the_short_tag_and_records_the_registry() {
    let source = Temp::new();
    let published = source.store();
    let image = open_image(&published, "lab:1.0.0", b"official");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    push_with(
        &published,
        "lab:1.0.0",
        target(&registry, "unemu/lab:1.0.0"),
        &entry,
        &client(&entry, Anonymous),
        &PushOptions::default(),
    )
    .unwrap();

    let reference = Reference::parse("lab:1.0.0", DEFAULT_REGISTRY).unwrap();
    let local = reference.local_tag().unwrap();
    assert_eq!(local, "lab:1.0.0");
    let fresh = Temp::new();
    let store = fresh.store();
    open_image(&store, "made-here:1.0.0", b"made here");
    let default = Client::routed(DEFAULT_REGISTRY, &registry.host());
    let report = pull::pull(&store, &default, &reference, &local, &PROVIDERS, &|_| {}).unwrap();
    assert_eq!(report.outcome, pull::PullOutcome::Pulled);
    assert_eq!(store.tag_descriptor(&local).unwrap().digest, image.digest);
    assert_eq!(
        pulled_from(&store, &local).as_deref(),
        Some(DEFAULT_REGISTRY)
    );
    // A tag made here has no registry to record.
    assert_eq!(pulled_from(&store, "made-here:1.0.0"), None);
}

#[test]
fn pulls_from_other_registries_write_host_qualified_tags_that_never_collide() {
    let registries = [
        MockRegistry::start(open_policy()),
        MockRegistry::start(open_policy()),
    ];
    let mut images = Vec::new();
    for (registry, label) in registries.iter().zip([&b"first"[..], b"second"]) {
        let dir = Temp::new();
        let published = dir.store();
        images.push(open_image(&published, "x:1.1", label));
        let entry = registry.entry(false);
        push_with(
            &published,
            "x:1.1",
            target(registry, "private/media/x:1.1"),
            &entry,
            &client(&entry, Anonymous),
            &PushOptions::default(),
        )
        .unwrap();
    }
    assert_ne!(images[0].digest, images[1].digest);

    let dir = Temp::new();
    let store = dir.store();
    for (registry, image) in registries.iter().zip(&images) {
        let reference = target(registry, "private/media/x:1.1");
        let local = reference.local_tag().unwrap();
        assert_eq!(local, format!("{}/private/media/x:1.1", registry.host()));
        let client = client(&registry.entry(false), Anonymous);
        let report = pull::pull(&store, &client, &reference, &local, &PROVIDERS, &|_| {}).unwrap();
        assert_eq!(report.local_tag, local);
        assert_eq!(report.outcome, pull::PullOutcome::Pulled);
        assert_eq!(store.tag_descriptor(&local).unwrap().digest, image.digest);
        assert_eq!(pulled_from(&store, &local), Some(registry.host()));
    }
    let names: Vec<_> = store
        .list_tags(None, 10)
        .unwrap()
        .tags
        .into_iter()
        .map(|tag| tag.name)
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

/// Namespace policy belongs to the repository path: the host of a local tag
/// names the registry, never the namespace.
#[test]
fn the_push_guard_reads_the_namespace_after_the_host_of_a_local_tag() {
    let dir = Temp::new();
    let store = dir.store();
    let entry = RegistryEntry {
        host: "nas.example:5000".into(),
        private: false,
        user: None,
        plain_http: false,
        namespaces: vec![
            Namespace {
                name: "unemu".into(),
                private: false,
            },
            Namespace {
                name: "private".into(),
                private: true,
            },
        ],
    };
    for (local, allowed) in [
        ("nas.example:5000/unemu/rom:1.0.0", false),
        ("nas.example:5000/private/rom:1.0.0", true),
    ] {
        image(&store, local, &[(b"vendor rom", None)], Some("open"));
        // Push without a reference sends the local tag to its own host.
        let target = Reference::parse(local, DEFAULT_REGISTRY).unwrap();
        assert_eq!(target.host, "nas.example:5000");
        let plan = push::plan(&store, local, target, Some(&entry)).unwrap();
        if allowed {
            assert_eq!(plan.verdict(), &Ok(()), "{local}");
        } else {
            assert!(
                matches!(
                    plan.verdict(),
                    Err(Refusal::OfficialNamespaceNeedsOpen { .. })
                ),
                "{local}: {:?}",
                plan.verdict()
            );
        }
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

/// One host with a public `unemu/` and a private `private/`, as the
/// maintainer's registry serves them.
fn namespaced(registry: &MockRegistry) -> RegistryEntry {
    RegistryEntry {
        namespaces: vec![
            Namespace {
                name: "unemu".into(),
                private: false,
            },
            Namespace {
                name: "private".into(),
                private: true,
            },
        ],
        ..registry.entry(false)
    }
}

#[test]
fn per_namespace_privacy_sends_user_supplied_images_only_into_a_private_namespace_that_refuses_anonymous_reads(
) {
    let dir = Temp::new();
    let store = dir.store();
    let rom = image(&store, "rom:1.0.0", &[(b"vendor rom", None)], Some("open"));
    let open = open_image(&store, "open:1.0.0", b"open rom");
    let registry = MockRegistry::start(Policy {
        anonymous_read: vec!["unemu".into()],
        anonymous_write: vec!["unemu".into()],
        user: Some(("maintainer".into(), "synthetic-secret".into())),
        ..Policy::default()
    });
    let entry = namespaced(&registry);
    let maintainer = client(&entry, Credentials::new("maintainer", "synthetic-secret"));
    let options = PushOptions::default();
    for (local, path) in [
        ("rom:1.0.0", "unemu/rom:1.0.0"),
        ("rom:1.0.0", "owner/rom:1.0.0"),
        ("open:1.0.0", "owner/open:1.0.0"),
    ] {
        let refused = push_with(
            &store,
            local,
            target(&registry, path),
            &entry,
            &maintainer,
            &options,
        );
        assert!(
            matches!(
                refused,
                Err(Error::Refused(
                    Refusal::OfficialNamespaceNeedsOpen { .. } | Refusal::UnlistedNamespace { .. }
                ))
            ),
            "{path}: {refused:?}"
        );
    }
    assert!(
        registry.log().is_empty(),
        "configuration refusals need no network"
    );
    push_with(
        &store,
        "rom:1.0.0",
        target(&registry, "private/rom:1.0.0"),
        &entry,
        &maintainer,
        &options,
    )
    .unwrap();
    assert_eq!(
        registry.tag_digest("private/rom", "1.0.0"),
        Some(rom.digest)
    );
    push_with(
        &store,
        "open:1.0.0",
        target(&registry, "unemu/open:1.0.0"),
        &entry,
        &maintainer,
        &options,
    )
    .unwrap();
    assert_eq!(
        registry.tag_digest("unemu/open", "1.0.0"),
        Some(open.digest)
    );

    // A private namespace that answers anonymous reads receives nothing.
    let readable = MockRegistry::start(Policy {
        anonymous_read: vec!["private".into()],
        anonymous_write: vec!["*".into()],
        ..Policy::default()
    });
    let entry = namespaced(&readable);
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
    set_mark(&mut d, mark);
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
    set_mark(&mut layer, "open");
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

#[allow(clippy::too_many_arguments)]
fn push_cancellable_with(
    store: &Store,
    local: &str,
    target: Reference,
    entry: &RegistryEntry,
    client: &Client,
    options: &PushOptions,
    progress: &mut dyn FnMut(PushProgress<'_>),
    cancelled: &dyn Fn() -> bool,
) -> crate::Result<push::PushReport> {
    let plan = push::plan(store, local, target, Some(entry))?;
    push::push_cancellable(
        store, &plan, &PROVIDERS, client, options, progress, cancelled,
    )
}

#[test]
fn a_cancelled_push_stops_between_blobs_and_never_publishes_the_manifest() {
    let dir = Temp::new();
    let store = dir.store();
    let image = image(
        &store,
        "lab:1.0.0",
        &[
            (b"first payload", Some("open")),
            (b"second payload", Some("open")),
        ],
        Some("open"),
    );
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let cancelled = push_cancellable_with(
        &store,
        "lab:1.0.0",
        target(&registry, "owner/lab:1.0.0"),
        &entry,
        &client,
        &PushOptions::default(),
        &mut |event| {
            if matches!(event, PushProgress::Blob(..)) {
                done.fetch_add(1, Ordering::SeqCst);
            }
        },
        // Cancel as soon as the first blob is on the registry.
        &|| done.load(Ordering::SeqCst) >= 1,
    );
    assert!(matches!(cancelled, Err(Error::Cancelled)), "{cancelled:?}");
    assert_eq!(done.load(Ordering::SeqCst), 1);
    assert_eq!(registry.tag_digest("owner/lab", "1.0.0"), None);
    assert!(
        !registry
            .log()
            .iter()
            .any(|l| l.contains("/manifests/1.0.0") && l.starts_with("PUT ")),
        "{:?}",
        registry.log()
    );
    let plan = push::plan(
        &store,
        "lab:1.0.0",
        target(&registry, "owner/lab:1.0.0"),
        Some(&entry),
    )
    .unwrap();
    let on_registry = plan
        .closure()
        .iter()
        .filter(|c| registry.has_blob("owner/lab", &c.descriptor.digest))
        .count();
    assert_eq!(on_registry, 1, "only the first blob went up");

    // Cancelling before the manifest also stops a push whose blobs are all up.
    let late = push_cancellable_with(
        &store,
        "lab:1.0.0",
        target(&registry, "owner/lab:1.0.0"),
        &entry,
        &client,
        &PushOptions::default(),
        &mut |event| {
            if matches!(event, PushProgress::Blob(..)) {
                done.fetch_add(1, Ordering::SeqCst);
            }
        },
        // `done` counts the cancelled push's blob too.
        &|| done.load(Ordering::SeqCst) > plan.closure().len(),
    );
    assert!(matches!(late, Err(Error::Cancelled)), "{late:?}");
    assert_eq!(registry.tag_digest("owner/lab", "1.0.0"), None);

    let report = push_with(
        &store,
        "lab:1.0.0",
        target(&registry, "owner/lab:1.0.0"),
        &entry,
        &client,
        &PushOptions::default(),
    )
    .unwrap();
    assert!(report
        .blobs
        .iter()
        .all(|(_, outcome)| *outcome == BlobOutcome::Present));
    assert_eq!(
        registry.tag_digest("owner/lab", "1.0.0"),
        Some(image.digest)
    );
}

#[test]
fn a_push_cancelled_between_chunks_keeps_its_session_for_the_next_push() {
    let dir = Temp::new();
    let store = dir.store();
    let (image, payload) = large_image(&store, "big:1.0.0");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    let big = Digest::of(&payload);
    let mut chunks = Vec::new();
    let mut record = |event: PushProgress<'_>| {
        if let PushProgress::Chunk { blob, sent } = event {
            assert_eq!(blob.digest, big);
            chunks.push(sent);
        }
    };
    let first = push_cancellable_with(
        &store,
        "big:1.0.0",
        target(&registry, "owner/big:1.0.0"),
        &entry,
        &client,
        &chunked_options(0),
        &mut record,
        // Cancel once the registry holds the first chunk of the large blob.
        &|| {
            registry
                .sessions()
                .iter()
                .any(|&held| held >= MIN_CHUNK_SIZE as usize)
        },
    );
    assert!(matches!(first, Err(Error::Cancelled)), "{first:?}");
    assert_eq!(UploadSessions::for_store(&store).count(), 1);
    assert_eq!(registry.sessions(), vec![MIN_CHUNK_SIZE as usize]);
    assert_eq!(registry.tag_digest("owner/big", "1.0.0"), None);
    registry.clear_log();
    push_cancellable_with(
        &store,
        "big:1.0.0",
        target(&registry, "owner/big:1.0.0"),
        &entry,
        &client,
        &chunked_options(0),
        &mut record,
        &|| false,
    )
    .unwrap();
    // The first push's accepted chunk, the resumed offset, then each chunk.
    let size = payload.len() as u64;
    assert_eq!(
        chunks,
        [MIN_CHUNK_SIZE, MIN_CHUNK_SIZE, 2 * MIN_CHUNK_SIZE, size]
    );
    let log = registry.log();
    let patches = log.iter().filter(|l| l.starts_with("PATCH ")).count();
    assert_eq!(
        patches, 2,
        "the resumed upload sends only the rest: {log:?}"
    );
    assert!(registry.has_blob("owner/big", &big));
    assert_eq!(
        registry.tag_digest("owner/big", "1.0.0"),
        Some(image.digest)
    );
    assert_eq!(UploadSessions::for_store(&store).count(), 0);
}

#[test]
fn a_cancelled_pull_never_sets_the_local_tag_and_the_next_pull_fetches_the_rest() {
    let source = Temp::new();
    let store = source.store();
    let image = image(
        &store,
        "lab:1.0.0",
        &[
            (b"rom", Some("open")),
            (b"disk", Some("open")),
            (b"floppy", Some("open")),
        ],
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
    let fetched = std::sync::atomic::AtomicUsize::new(0);
    let cancelled = pull::pull_cancellable(
        &pulled,
        &client,
        &reference,
        "copy:1.0.0",
        &PROVIDERS,
        &|event| {
            if matches!(event, pull::PullProgress::Verified(_)) {
                fetched.fetch_add(1, Ordering::SeqCst);
            }
        },
        &|| fetched.load(Ordering::SeqCst) >= 1,
    );
    assert!(matches!(cancelled, Err(Error::Cancelled)), "{cancelled:?}");
    assert!(matches!(
        pulled.tag_descriptor("copy:1.0.0"),
        Err(image_store::Error::TagNotFound(_))
    ));
    let received = std::sync::Mutex::new(std::collections::BTreeMap::new());
    let verified = std::sync::Mutex::new(Vec::new());
    let report = pull::pull_cancellable(
        &pulled,
        &client,
        &reference,
        "copy:1.0.0",
        &PROVIDERS,
        &|event| match event {
            pull::PullProgress::Receiving { blob, have } => {
                let mut received = received.lock().unwrap();
                let last = received.entry(blob.digest.clone()).or_insert(0);
                assert!(have >= *last, "received bytes never go back");
                *last = have;
            }
            pull::PullProgress::Verified(blob) => verified.lock().unwrap().push(blob.clone()),
        },
        &|| false,
    )
    .unwrap();
    assert_eq!(report.outcome, pull::PullOutcome::Pulled);
    assert!(
        !report.present.is_empty(),
        "a blob verified before the cancel is not fetched again"
    );
    assert_eq!(report.present.len() + report.fetched.len(), 5);
    let (received, mut verified) = (
        received.into_inner().unwrap(),
        verified.into_inner().unwrap(),
    );
    verified.sort_by(|a, b| a.digest.cmp(&b.digest));
    let mut fetched = report.fetched.clone();
    fetched.sort_by(|a, b| a.digest.cmp(&b.digest));
    assert_eq!(verified, fetched);
    for blob in &report.fetched {
        assert_eq!(
            received.get(&blob.digest),
            Some(&blob.size),
            "{}",
            blob.digest
        );
    }
    assert_eq!(
        pulled.tag_descriptor("copy:1.0.0").unwrap().digest,
        image.digest
    );

    // A cancel before anything is fetched leaves no tag either.
    let empty = Temp::new();
    let untouched = empty.store();
    let refused = pull::pull_cancellable(
        &untouched,
        &client,
        &reference,
        "copy:1.0.0",
        &PROVIDERS,
        &|_| {},
        &|| true,
    );
    assert!(matches!(refused, Err(Error::Cancelled)), "{refused:?}");
    assert!(untouched.tag_descriptor("copy:1.0.0").is_err());
}

#[test]
fn a_confirmed_move_applies_only_from_the_digest_the_user_saw() {
    let dir = Temp::new();
    let store = dir.store();
    let first = open_image(&store, "lab:1.0.0", b"first");
    let second = open_image(&store, "lab:2.0.0", b"second");
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    push_with(
        &store,
        "lab:1.0.0",
        target(&registry, "o/lab:latest"),
        &entry,
        &client,
        &PushOptions::default(),
    )
    .unwrap();
    // Another client moves the tag after the user confirmed the first image.
    let elsewhere = registry.seed_manifest("o/lab", "latest", br#"{"schemaVersion":2}"#);
    registry.clear_log();
    let confirmed = |from: &Digest| PushOptions {
        allow_move: true,
        move_from: Some(from.clone()),
        ..PushOptions::default()
    };
    let stale = push_with(
        &store,
        "lab:2.0.0",
        target(&registry, "o/lab:latest"),
        &entry,
        &client,
        &confirmed(&first.digest),
    );
    assert!(
        matches!(&stale, Err(Error::TagMoveNeedsConfirmation { current, new, .. })
            if *current == elsewhere && *new == second.digest),
        "{stale:?}"
    );
    assert_eq!(uploads(&registry.log()), 0, "nothing is uploaded first");
    assert_eq!(
        registry.tag_digest("o/lab", "latest"),
        Some(elsewhere.clone())
    );
    let moved = push_with(
        &store,
        "lab:2.0.0",
        target(&registry, "o/lab:latest"),
        &entry,
        &client,
        &confirmed(&elsewhere),
    )
    .unwrap();
    assert_eq!(moved.tag, TagOutcome::Moved { from: elsewhere });
    assert_eq!(registry.tag_digest("o/lab", "latest"), Some(second.digest));
}

/// A provider that refuses config keys it does not know, as every real
/// decoder does: `unknown field`, after the closure is fetched.
struct Strict;
impl ConfigValidator for Strict {
    fn platform(&self) -> &str {
        "synthetic"
    }
    fn validate(
        &self,
        table: &toml::Table,
        descriptors: &[Descriptor],
        blobs: &dyn BlobSource,
    ) -> image_store::Result<Vec<Digest>> {
        if let Some(key) = table
            .keys()
            .find(|k| !matches!(k.as_str(), "schema" | "platform" | "blobs"))
        {
            return Err(image_store::Error::Invalid(format!(
                "unknown field `{key}`"
            )));
        }
        Synthetic.validate(table, descriptors, blobs)
    }
}

/// An image another release published: its config holds a key this build
/// does not know, and its manifest optionally declares the release it needs.
fn newer_image(
    registry: &MockRegistry,
    repository: &str,
    marker: Option<&str>,
) -> (Reference, Vec<Digest>) {
    let payload: &[u8] = b"payload of a newer release";
    let rom = registry.seed_blob(repository, payload);
    let config =
        format!("schema = 1\nplatform = \"synthetic\"\nblobs = [\"{rom}\"]\nfuture_option = 1\n");
    let config_digest = registry.seed_blob(repository, config.as_bytes());
    let metadata: &[u8] = br#"{"schema":1,"platform":"synthetic","titles":[]}"#;
    let metadata_digest = registry.seed_blob(repository, metadata);
    let mut manifest = Manifest::new(
        "synthetic",
        Descriptor::new(mt::CONFIG, config_digest.clone(), config.len() as u64),
        vec![
            Descriptor::new(mt::FIRMWARE, rom.clone(), payload.len() as u64),
            Descriptor::new(mt::METADATA, metadata_digest.clone(), metadata.len() as u64),
        ],
    )
    .unwrap();
    if let Some(marker) = marker {
        manifest
            .annotations
            .insert(image_store::compat::MIN_APP_VERSION.into(), marker.into());
    }
    registry.seed_manifest(repository, "1.0.0", &manifest.encode().unwrap());
    (
        target(registry, &format!("{repository}:1.0.0")),
        vec![rom, config_digest, metadata_digest],
    )
}

fn blob_requests(registry: &MockRegistry) -> Vec<String> {
    registry
        .log()
        .into_iter()
        .filter(|line| line.contains("/blobs/"))
        .collect()
}

#[test]
fn pull_refuses_an_image_that_needs_a_newer_release_before_fetching_any_blob() {
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    let level = image_store::compat::level();
    let providers: [&dyn ConfigValidator; 1] = [&Strict];

    let (reference, _) = newer_image(&registry, "o/newer", Some("99.0.0"));
    let dir = Temp::new();
    let store = dir.store();
    registry.clear_log();
    let refused = pull::pull(
        &store,
        &client,
        &reference,
        "newer:1.0.0",
        &providers,
        &|_| {},
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        format!("{reference} needs unemupc 99.0.0 or newer (this is {level})")
    );
    assert!(blob_requests(&registry).is_empty(), "{:?}", registry.log());
    assert!(registry.log().iter().all(|l| l.contains("/manifests/")));
    assert!(matches!(
        store.tag_descriptor("newer:1.0.0"),
        Err(image_store::Error::TagNotFound(_))
    ));

    // Without the marker the same image fails as before: after the whole
    // closure is fetched, with the decoder's message, and nothing is tagged.
    let (unmarked, blobs) = newer_image(&registry, "o/unmarked", None);
    registry.clear_log();
    let refused = pull::pull(
        &store,
        &client,
        &unmarked,
        "unmarked:1.0.0",
        &providers,
        &|_| {},
    );
    let message = refused.unwrap_err().to_string();
    assert!(
        message.contains("unknown field `future_option`"),
        "{message}"
    );
    assert_eq!(blob_requests(&registry).len(), blobs.len());
    assert!(matches!(
        store.tag_descriptor("unmarked:1.0.0"),
        Err(image_store::Error::TagNotFound(_))
    ));

    // A marker that does not parse counts as unmet.
    let (invalid, _) = newer_image(&registry, "o/invalid", Some("next"));
    registry.clear_log();
    let refused = pull::pull(
        &store,
        &client,
        &invalid,
        "invalid:1.0.0",
        &providers,
        &|_| {},
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        format!("{invalid} needs a newer unemupc (this is {level})")
    );
    assert!(blob_requests(&registry).is_empty());
}

#[test]
fn pull_takes_an_image_whose_marker_this_release_meets() {
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    let source = Temp::new();
    let store = source.store();
    let level = image_store::compat::level();
    let image = rated_image(&store, "met:1.0.0", b"met", Some(&level.to_string()));
    let reference = target(&registry, "o/met:1.0.0");
    push_with(
        &store,
        "met:1.0.0",
        reference.clone(),
        &entry,
        &client,
        &PushOptions::default(),
    )
    .unwrap();
    let fresh = Temp::new();
    let report = pull::pull(
        &fresh.store(),
        &client,
        &reference,
        "met:1.0.0",
        &PROVIDERS,
        &|_| {},
    )
    .unwrap();
    assert_eq!(report.image.digest, image.digest);
}

/// A synthetic provider whose tables rate every image at `self.0`.
struct Rated(image_store::compat::AppVersion);
impl ConfigValidator for Rated {
    fn platform(&self) -> &str {
        "synthetic"
    }
    fn validate(
        &self,
        table: &toml::Table,
        descriptors: &[Descriptor],
        blobs: &dyn BlobSource,
    ) -> image_store::Result<Vec<Digest>> {
        Synthetic.validate(table, descriptors, blobs)
    }
    fn required_release(
        &self,
        _: &Manifest,
        _: &[u8],
    ) -> image_store::Result<image_store::compat::AppVersion> {
        Ok(self.0)
    }
}

/// An open synthetic image whose manifest declares `marker`.
fn rated_image(store: &Store, tag: &str, label: &[u8], marker: Option<&str>) -> Descriptor {
    let mut layers = vec![marked(store, mt::FIRMWARE, label, Some("open"))];
    let config = marked(
        store,
        mt::CONFIG,
        format!(
            "schema = 1\nplatform = \"synthetic\"\nblobs = [\"{}\"]\n",
            layers[0].digest
        )
        .as_bytes(),
        Some("open"),
    );
    layers.push(marked(
        store,
        mt::METADATA,
        br#"{"schema":1,"platform":"synthetic","titles":[]}"#,
        Some("open"),
    ));
    let mut manifest = Manifest::new("synthetic", config, layers).unwrap();
    if let Some(marker) = marker {
        manifest
            .annotations
            .insert(image_store::compat::MIN_APP_VERSION.into(), marker.into());
    }
    let descriptor = store.import_manifest(&manifest).unwrap();
    store
        .update_tag(tag, &descriptor, None, &Synthetic)
        .unwrap();
    descriptor
}

#[test]
fn the_official_namespace_refuses_a_missing_or_lower_release_marker() {
    let dir = Temp::new();
    let store = dir.store();
    let required = image_store::compat::level();
    assert!(required > image_store::compat::BASELINE);
    rated_image(&store, "none:1.0.0", b"none", None);
    rated_image(&store, "lower:1.0.0", b"lower", Some("0.0.1"));
    let exact = rated_image(&store, "exact:1.0.0", b"exact", Some(&required.to_string()));
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let client = client(&entry, Anonymous);
    let rated = Rated(required);
    let push_to = |local: &str, path: &str| {
        let plan = push::plan(&store, local, target(&registry, path), Some(&entry))?;
        push::push(
            &store,
            &plan,
            &[&rated],
            &client,
            &PushOptions::default(),
            &mut |_, _| {},
        )
    };
    for (local, declared) in [("none:1.0.0", "none"), ("lower:1.0.0", "0.0.1")] {
        let refused = push_to(local, &format!("unemu/{local}")).unwrap_err();
        assert!(
            matches!(
                &refused,
                Error::Refused(Refusal::MinAppVersion { required: r, declared: d, .. })
                    if *r == required && d == declared
            ),
            "{refused:?}"
        );
        let message = refused.to_string();
        assert!(
            message.contains(&format!("needs unemupc {required} or newer")),
            "{message}"
        );
        let repository = local.split(':').next().unwrap();
        assert_eq!(
            registry.tag_digest(&format!("unemu/{repository}"), "1.0.0"),
            None
        );
    }
    // A declaration at least as high as the computed release passes.
    push_to("exact:1.0.0", "unemu/exact:1.0.0").unwrap();
    assert_eq!(
        registry.tag_digest("unemu/exact", "1.0.0"),
        Some(exact.digest)
    );
    // Other namespaces are not checked.
    push_to("none:1.0.0", "private/none:1.0.0").unwrap();
    // A provider whose tables rate the image at the baseline needs no marker.
    let baseline = Rated(image_store::compat::BASELINE);
    let plan = push::plan(
        &store,
        "none:1.0.0",
        target(&registry, "unemu/none:1.0.0"),
        Some(&entry),
    )
    .unwrap();
    push::push(
        &store,
        &plan,
        &[&baseline],
        &client,
        &PushOptions::default(),
        &mut |_, _| {},
    )
    .unwrap();
}

#[test]
fn the_release_guard_reads_the_marker_and_names_the_computed_release() {
    let manifest = |marker: Option<&str>| {
        let config = Descriptor::new(mt::CONFIG, Digest::of(b"config"), 6);
        let metadata = Descriptor::new(mt::METADATA, Digest::of(b"metadata"), 8);
        let mut manifest = Manifest::new("synthetic", config, vec![metadata]).unwrap();
        if let Some(marker) = marker {
            manifest
                .annotations
                .insert(image_store::compat::MIN_APP_VERSION.into(), marker.into());
        }
        manifest
    };
    let registry = MockRegistry::start(open_policy());
    let official = target(&registry, "unemu/lab:1.0.0");
    let private = target(&registry, "private/lab:1.0.0");
    let next = image_store::compat::NEXT_RELEASE;
    let baseline = image_store::compat::BASELINE;
    use crate::guard::check_release;
    assert!(check_release(&official, &manifest(None), baseline).is_ok());
    assert!(check_release(&official, &manifest(Some("0.0.3")), next).is_ok());
    assert!(check_release(&official, &manifest(Some("1.0.0")), next).is_ok());
    for marker in [None, Some("0.0.2"), Some("0.0.3-rc.1"), Some("soon")] {
        let refusal = check_release(&official, &manifest(marker), next).unwrap_err();
        assert!(
            refusal.to_string().contains("needs unemupc 0.0.3 or newer"),
            "{refusal}"
        );
        assert!(check_release(&private, &manifest(marker), next).is_ok());
    }
}

/// A push plan reads the marker before it decodes the manifest, so an image
/// a newer release tagged in a shared store, whose envelope this build cannot
/// decode, is refused with the release it needs.
#[test]
fn a_push_plan_refuses_an_unreadable_newer_image_with_the_release_it_needs() {
    let dir = Temp::new();
    let store = dir.store();
    let bytes = br#"{"schemaVersion":3,"mediaType":"application/vnd.oci.image.manifest.v2+json","annotations":{"unemu.platform":"synthetic","unemu.min-app-version":"99.0.0"}}"#;
    let blob = store.import(&bytes[..], None).unwrap();
    let image = Descriptor::new(mt::MANIFEST, blob.digest, blob.size);
    store.tag_as_newer_release("future:1", &image).unwrap();
    let registry = MockRegistry::start(open_policy());
    let entry = registry.entry(false);
    let sentence = format!(
        "future:1 needs unemupc 99.0.0 or newer (this is {})",
        image_store::compat::level()
    );
    let refused = push::plan_with_marks(
        &store,
        "future:1",
        target(&registry, "o/future:1"),
        Some(&entry),
        crate::guard::LocalMarks::default(),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(refused, sentence);
    // The scan of local marks, which every push makes, fails closed on it
    // with the same sentence.
    let refused = push::plan(
        &store,
        "future:1",
        target(&registry, "o/future:1"),
        Some(&entry),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(refused, sentence);
}
