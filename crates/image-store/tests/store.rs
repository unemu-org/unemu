// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic OCI Image Layout integration tests. No emulator or guest fixtures.

use image_store::{
    media_type as mt, BlobSource, ConfigValidator, Descriptor, Digest, Error, Manifest, Result,
    Store, MAX_SMALL_BLOB,
};
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Barrier,
    },
    thread,
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "image-store-test-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("create test directory: {e}"),
            }
        }
    }
    fn store(&self) -> Store {
        Store::open(&self.0).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Provider(&'static str);
impl ConfigValidator for Provider {
    fn platform(&self) -> &str {
        self.0
    }
    fn validate(
        &self,
        table: &toml::Table,
        _: &[Descriptor],
        _: &dyn BlobSource,
    ) -> Result<Vec<Digest>> {
        if table
            .keys()
            .any(|k| !matches!(k.as_str(), "schema" | "platform" | "blobs"))
        {
            return Err(Error::Invalid("unknown synthetic config field".into()));
        }
        table
            .get("blobs")
            .and_then(toml::Value::as_array)
            .ok_or_else(|| Error::Invalid("missing blobs".into()))?
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| Error::Invalid("digest is not a string".into()))?
                    .parse()
            })
            .collect()
    }
}

fn put(store: &Store, kind: &str, bytes: &[u8]) -> Descriptor {
    let blob = store.import(bytes, None).unwrap();
    Descriptor::new(kind, blob.digest, blob.size)
}

fn config(store: &Store, platform: &str, references: &[&Digest]) -> Descriptor {
    let blobs = references
        .iter()
        .map(|d| format!("\"{d}\""))
        .collect::<Vec<_>>()
        .join(",");
    put(
        store,
        mt::CONFIG,
        format!("schema = 1\nplatform = \"{platform}\"\nblobs = [{blobs}]\n").as_bytes(),
    )
}

fn fixture(store: &Store, label: &[u8]) -> (Manifest, Descriptor) {
    let payload = put(store, mt::FIRMWARE, label);
    let cfg = config(store, "zx-spectrum", &[&payload.digest, &payload.digest]);
    let metadata = put(
        store,
        mt::METADATA,
        br#"{"schema":1,"platform":"zx-spectrum","titles":[]}"#,
    );
    (
        Manifest::new("zx-spectrum", cfg, vec![payload.clone(), metadata]).unwrap(),
        payload,
    )
}

fn blob_path(store: &Store, descriptor: &Descriptor) -> PathBuf {
    store
        .root()
        .join("images/blobs/sha256")
        .join(descriptor.digest.hex())
}

fn publish(store: &Store, manifest: &Manifest, name: &str) -> Descriptor {
    let image = store.import_manifest(manifest).unwrap();
    store
        .update_tag(name, &image, None, &Provider("zx-spectrum"))
        .unwrap();
    image
}

#[test]
fn layout_is_standard_and_reopen_preserves_content_and_stale_lock_inode() {
    let dir = Temp::new();
    let store = dir.store();
    assert_eq!(
        fs::read_to_string(dir.0.join("store.toml")).unwrap(),
        "schema = 1\n"
    );
    assert_eq!(
        fs::read_to_string(dir.0.join("images/oci-layout")).unwrap(),
        r#"{"imageLayoutVersion":"1.0.0"}"#
    );
    let (manifest, _) = fixture(&store, b"firmware");
    let image = publish(&store, &manifest, "lab:1");
    let lock_path = dir.0.join("locks/index.lock");
    fs::write(&lock_path, b"leftover metadata is not ownership").unwrap();
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&lock_path).unwrap().ino()
    };
    let reopened = dir.store();
    assert_eq!(
        reopened
            .resolve("lab:1", &Provider("zx-spectrum"))
            .unwrap()
            .image
            .digest,
        image.digest
    );
    assert_eq!(
        fs::read(&lock_path).unwrap(),
        b"leftover metadata is not ownership"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(&lock_path).unwrap().ino(), inode);
    }
}

#[test]
fn unsupported_store_and_layout_versions_are_not_rewritten() {
    let dir = Temp::new();
    let store = dir.store();
    let before = fs::read(store.root().join("images/index.json")).unwrap();
    for version in ["schema = 2\n", "schema = 1\nfuture = true\n"] {
        fs::write(dir.0.join("store.toml"), version).unwrap();
        assert!(Store::open(&dir.0).is_err());
        assert_eq!(
            fs::read_to_string(dir.0.join("store.toml")).unwrap(),
            version
        );
        assert_eq!(fs::read(dir.0.join("images/index.json")).unwrap(), before);
    }
    fs::write(dir.0.join("store.toml"), "schema = 1\n").unwrap();
    fs::write(
        dir.0.join("images/oci-layout"),
        br#"{"imageLayoutVersion":"2.0.0"}"#,
    )
    .unwrap();
    assert!(Store::open(&dir.0).is_err());
}

#[test]
fn import_deduplicates_without_replacing_an_existing_inode() {
    let dir = Temp::new();
    let store = dir.store();
    let first = put(&store, mt::FIRMWARE, b"immutable");
    let path = blob_path(&store, &first);
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&path).unwrap().ino()
    };
    let second = store
        .import(&b"immutable"[..], Some(&first.digest))
        .unwrap();
    assert_eq!(first.digest, second.digest);
    assert_eq!(
        fs::read_dir(store.root().join("images/blobs/sha256"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(store.root().join("staging")).unwrap().count(),
        0
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(path).unwrap().ino(), inode);
    }
}

#[test]
fn imported_source_is_copied_and_remains_independent() {
    let dir = Temp::new();
    let store = dir.store();
    let source = dir.0.join("source.bin");
    fs::write(&source, b"original").unwrap();
    let blob = store
        .import_file(&source, Some(&Digest::of(b"original")))
        .unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"original");
    fs::write(&source, b"modified").unwrap();
    store.verify(&blob.digest, blob.size).unwrap();
    let mut bytes = Vec::new();
    store
        .open_blob(&blob.digest, blob.size)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"original");
}

#[test]
fn digest_mismatch_cleans_staging_and_publishes_nothing() {
    let dir = Temp::new();
    let store = dir.store();
    assert!(matches!(
        store.import(&b"actual"[..], Some(&Digest::of(b"expected"))),
        Err(Error::DigestMismatch { .. })
    ));
    assert_eq!(
        fs::read_dir(store.root().join("images/blobs/sha256"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        fs::read_dir(store.root().join("staging")).unwrap().count(),
        0
    );
}

#[test]
fn corrupt_existing_blob_is_rejected_without_overwrite() {
    let dir = Temp::new();
    let store = dir.store();
    let original = put(&store, mt::DISK_BASE, b"original");
    fs::write(blob_path(&store, &original), b"tampered").unwrap();
    assert!(matches!(
        store.verify(&original.digest, original.size),
        Err(Error::DigestMismatch { .. })
    ));
    assert!(matches!(
        store.import(&b"original"[..], None),
        Err(Error::DigestMismatch { .. })
    ));
    assert_eq!(fs::read(blob_path(&store, &original)).unwrap(), b"tampered");
    assert_eq!(
        fs::read_dir(store.root().join("staging")).unwrap().count(),
        0
    );
}

#[test]
fn open_and_verify_check_exact_size_and_regular_file() {
    let dir = Temp::new();
    let store = dir.store();
    let payload = put(&store, mt::FIRMWARE, b"bytes");
    assert!(matches!(
        store.open_blob(&payload.digest, 4),
        Err(Error::SizeMismatch {
            expected: 4,
            actual: 5
        })
    ));
    fs::write(blob_path(&store, &payload), b"tiny").unwrap();
    assert!(matches!(
        store.verify(&payload.digest, payload.size),
        Err(Error::SizeMismatch {
            expected: 5,
            actual: 4
        })
    ));
    #[cfg(unix)]
    {
        fs::remove_file(blob_path(&store, &payload)).unwrap();
        let target = dir.0.join("external.bin");
        fs::write(&target, b"bytes").unwrap();
        std::os::unix::fs::symlink(&target, blob_path(&store, &payload)).unwrap();
        assert!(store.open_blob(&payload.digest, payload.size).is_err());
    }
}

#[test]
fn digest_parser_rejects_path_traversal_and_noncanonical_addresses() {
    for bad in ["../blob", "sha512:abc", "sha256:", "sha256:../anything"] {
        assert!(bad.parse::<Digest>().is_err());
    }
    assert!(format!("sha256:{}", "A".repeat(64))
        .parse::<Digest>()
        .is_err());
    let valid = format!("sha256:{}", "a".repeat(64));
    assert_eq!(valid.parse::<Digest>().unwrap().to_string(), valid);
}

#[test]
fn manifest_bytes_have_fixed_field_order_and_sorted_annotations() {
    let cfg = Descriptor::new(mt::CONFIG, Digest::of(b"config"), 6);
    let metadata = Descriptor::new(mt::METADATA, Digest::of(b"metadata"), 8);
    let mut manifest = Manifest::new("c64", cfg.clone(), vec![metadata.clone()]).unwrap();
    manifest.annotations.insert("z".into(), "last".into());
    manifest.annotations.insert("a".into(), "first".into());
    let expected = format!(
        r#"{{"schemaVersion":2,"mediaType":"{}","artifactType":"{}","config":{{"mediaType":"{}","digest":"{}","size":6}},"layers":[{{"mediaType":"{}","digest":"{}","size":8}}],"annotations":{{"a":"first","unemu.platform":"c64","z":"last"}}}}"#,
        mt::MANIFEST,
        mt::MACHINE,
        mt::CONFIG,
        cfg.digest,
        mt::METADATA,
        metadata.digest
    );
    assert_eq!(manifest.encode().unwrap(), expected.as_bytes());
    let mut second = manifest.clone();
    second.annotations = BTreeMap::from([
        ("a".into(), "first".into()),
        ("z".into(), "last".into()),
        ("unemu.platform".into(), "c64".into()),
    ]);
    assert_eq!(manifest.encode().unwrap(), second.encode().unwrap());
    assert_eq!(Manifest::decode(expected.as_bytes()).unwrap(), manifest);
    assert_eq!(
        Digest::of(&manifest.encode().unwrap()),
        Digest::of(expected.as_bytes())
    );
}

fn valid_json() -> serde_json::Value {
    let manifest = Manifest::new(
        "c64",
        Descriptor::new(mt::CONFIG, Digest::of(b"config"), 6),
        vec![Descriptor::new(mt::METADATA, Digest::of(b"meta"), 4)],
    )
    .unwrap();
    serde_json::from_slice(&manifest.encode().unwrap()).unwrap()
}

#[test]
fn decoder_rejects_unknown_fields_in_manifest_and_descriptors() {
    for path in ["root", "config", "layer"] {
        let mut value = valid_json();
        match path {
            "root" => value["future"] = true.into(),
            "config" => value["config"]["future"] = serde_json::json!(["external"]),
            _ => value["layers"][0]["future"] = true.into(),
        }
        assert!(
            Manifest::decode(&serde_json::to_vec(&value).unwrap()).is_err(),
            "{path}"
        );
    }
}

#[test]
fn decoder_rejects_missing_required_fields_versions_types_and_duplicate_keys() {
    for key in [
        "schemaVersion",
        "mediaType",
        "artifactType",
        "config",
        "layers",
        "annotations",
    ] {
        let mut value = valid_json();
        value.as_object_mut().unwrap().remove(key);
        assert!(
            Manifest::decode(&serde_json::to_vec(&value).unwrap()).is_err(),
            "missing {key}"
        );
    }
    for (key, bad) in [
        ("schemaVersion", serde_json::json!(3)),
        ("artifactType", serde_json::json!("unknown")),
        ("mediaType", serde_json::json!(mt::INDEX)),
    ] {
        let mut value = valid_json();
        value[key] = bad;
        assert!(Manifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    for key in ["mediaType", "digest", "size"] {
        let mut value = valid_json();
        value["config"].as_object_mut().unwrap().remove(key);
        assert!(Manifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let bytes =
        serde_json::to_string(&valid_json())
            .unwrap()
            .replacen('{', "{\"schemaVersion\":2,", 1);
    assert!(Manifest::decode(bytes.as_bytes()).is_err());
    let duplicate_annotation = serde_json::to_string(&valid_json()).unwrap().replace(
        "\"unemu.platform\":\"c64\"",
        "\"unemu.platform\":\"c64\",\"unemu.platform\":\"zx-spectrum\"",
    );
    assert!(Manifest::decode(duplicate_annotation.as_bytes()).is_err());
    assert!(Manifest::decode(b"{}").is_err());
    assert!(Manifest::decode(b"[]").is_err());
    assert!(Manifest::decode(&vec![b' '; MAX_SMALL_BLOB as usize + 1]).is_err());
    for bad in [
        serde_json::json!(-1),
        serde_json::json!("1"),
        serde_json::json!(null),
    ] {
        let mut value = valid_json();
        value["config"]["size"] = bad;
        assert!(Manifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}

#[test]
fn decoder_rejects_unsupported_media_platforms_duplicates_and_size_limits() {
    for kind in [
        "application/vnd.unemu.media.optical.v1",
        "application/vnd.unemu.machine.state.v1",
        "application/vnd.unemu.disk.base.v1+zstd",
        "application/vnd.unemu.unknown.v1",
    ] {
        let mut value = valid_json();
        value["layers"][0]["mediaType"] = kind.into();
        assert!(Manifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    for platform in [
        serde_json::json!(null),
        serde_json::json!(""),
        serde_json::json!("../c64"),
    ] {
        let mut value = valid_json();
        value["annotations"]["unemu.platform"] = platform;
        assert!(Manifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let mut value = valid_json();
    value["layers"][0]["size"] = (MAX_SMALL_BLOB + 1).into();
    assert!(Manifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = valid_json();
    let duplicate = value["layers"][0].clone();
    value["layers"].as_array_mut().unwrap().push(duplicate);
    assert!(Manifest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn missing_blob_refuses_tag_and_preserves_existing_index() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, payload) = fixture(&store, b"firmware");
    let image = store.import_manifest(&manifest).unwrap();
    fs::remove_file(blob_path(&store, &payload)).unwrap();
    let before = fs::read(dir.0.join("images/index.json")).unwrap();
    assert!(store
        .update_tag("lab:1", &image, None, &Provider("zx-spectrum"))
        .is_err());
    assert_eq!(fs::read(dir.0.join("images/index.json")).unwrap(), before);
    assert!(store.list_tags(None, 10).unwrap().tags.is_empty());
}

#[test]
fn missing_manifest_config_or_metadata_refuses_tag() {
    for role in ["manifest", "config", "metadata"] {
        let dir = Temp::new();
        let store = dir.store();
        let (manifest, _) = fixture(&store, b"firmware");
        let image = store.import_manifest(&manifest).unwrap();
        let missing = match role {
            "manifest" => &image,
            "config" => &manifest.config,
            _ => &manifest.layers[1],
        };
        fs::remove_file(blob_path(&store, missing)).unwrap();
        assert!(
            store
                .update_tag("lab:1", &image, None, &Provider("zx-spectrum"))
                .is_err(),
            "{role}"
        );
    }
}

#[test]
fn closure_rejects_unreferenced_missing_duplicate_and_reordered_descriptors() {
    let dir = Temp::new();
    let store = dir.store();
    let (original, _) = fixture(&store, b"firmware");
    for variant in ["extra", "missing", "order"] {
        let mut manifest = original.clone();
        match variant {
            "extra" => manifest
                .layers
                .insert(1, put(&store, mt::FLOPPY, b"unreferenced")),
            "missing" => {
                manifest.layers.remove(0);
            }
            _ => manifest.layers.swap(0, 1),
        }
        let image = store.import_manifest(&manifest).unwrap();
        assert!(
            store
                .update_tag("lab:1", &image, None, &Provider("zx-spectrum"))
                .is_err(),
            "{variant}"
        );
    }
    let mut duplicate = original;
    duplicate.layers.push(duplicate.layers[0].clone());
    assert!(duplicate.encode().is_err());
}

#[test]
fn platform_agrees_in_config_manifest_metadata_and_provider() {
    let dir = Temp::new();
    let store = dir.store();
    let (original, _) = fixture(&store, b"firmware");
    for variant in [
        "config", "manifest", "metadata", "provider", "missing", "schema", "unknown",
    ] {
        let mut manifest = original.clone();
        match variant {
            "config" => manifest.config = config(&store, "c64", &[&manifest.layers[0].digest]),
            "manifest" => {
                manifest
                    .annotations
                    .insert("unemu.platform".into(), "c64".into());
            }
            "metadata" => {
                manifest.layers[1] = put(
                    &store,
                    mt::METADATA,
                    br#"{"schema":1,"platform":"c64","titles":[]}"#,
                )
            }
            "missing" => manifest.config = put(&store, mt::CONFIG, b"schema = 1\nblobs = []\n"),
            "schema" => {
                manifest.config = put(
                    &store,
                    mt::CONFIG,
                    b"schema = 2\nplatform = 'zx-spectrum'\nblobs = []\n",
                )
            }
            "unknown" => {
                let mut bytes = store
                    .read_verified(&manifest.config, MAX_SMALL_BLOB)
                    .unwrap();
                bytes.extend_from_slice(b"future = true\n");
                manifest.config = put(&store, mt::CONFIG, &bytes)
            }
            _ => (),
        }
        let image = store.import_manifest(&manifest).unwrap();
        let provider = if variant == "provider" {
            "c64"
        } else {
            "zx-spectrum"
        };
        assert!(
            store
                .update_tag("lab:1", &image, None, &Provider(provider))
                .is_err(),
            "{variant}"
        );
    }
}

#[test]
fn artwork_is_in_metadata_first_use_order_and_missing_artwork_is_reported() {
    let dir = Temp::new();
    let store = dir.store();
    let (mut manifest, _) = fixture(&store, b"firmware");
    let cover = put(&store, mt::PNG, b"synthetic cover");
    let screenshot = put(&store, mt::JPEG, b"synthetic screenshot");
    let json = serde_json::json!({"schema":1,"platform":"zx-spectrum", "titles":[{"id":"demo","title":"Demo", "cover":cover.digest,"screenshots":[screenshot.digest,cover.digest]}]});
    let metadata_bytes = serde_json::to_vec(&json).unwrap();
    manifest.layers[1] = put(&store, mt::METADATA, &metadata_bytes);
    manifest.layers.extend([cover.clone(), screenshot]);
    publish(&store, &manifest, "art:1");
    let resolved = store.resolve("art:1", &Provider("zx-spectrum")).unwrap();
    assert_eq!(resolved.metadata, metadata_bytes);
    assert_eq!(resolved.manifest, manifest);
    fs::remove_file(blob_path(&store, &cover)).unwrap();
    let image = store.import_manifest(&manifest).unwrap();
    assert!(store
        .update_tag("art:2", &image, None, &Provider("zx-spectrum"))
        .is_err());
    let resolved = store.resolve("art:1", &Provider("zx-spectrum")).unwrap();
    assert_eq!(resolved.missing_artwork, vec![cover]);
}

#[test]
fn tag_compare_and_swap_returns_current_digest_and_preserves_other_tags() {
    let dir = Temp::new();
    let store = dir.store();
    let (first, _) = fixture(&store, b"one");
    let one = publish(&store, &first, "lab:1");
    publish(&store, &first, "other:1");
    let (second, _) = fixture(&store, b"two");
    let two = store.import_manifest(&second).unwrap();
    assert!(
        matches!(store.update_tag("lab:1", &two, None, &Provider("zx-spectrum")), Err(Error::Conflict { current: Some(ref d) }) if *d == one.digest)
    );
    store
        .update_tag("lab:1", &two, Some(&one.digest), &Provider("zx-spectrum"))
        .unwrap();
    assert!(
        matches!(store.update_tag("lab:1", &one, Some(&one.digest), &Provider("zx-spectrum")), Err(Error::Conflict { current: Some(ref d) }) if *d == two.digest)
    );
    assert_eq!(
        store
            .resolve("lab:1", &Provider("zx-spectrum"))
            .unwrap()
            .image
            .digest,
        two.digest
    );
    assert_eq!(
        store
            .resolve("other:1", &Provider("zx-spectrum"))
            .unwrap()
            .image
            .digest,
        one.digest
    );
}

#[test]
fn tag_pages_are_sorted_and_cursor_resumes_without_duplicates() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    for name in ["z:1", "a:1", "m:1"] {
        publish(&store, &manifest, name);
    }
    let first = store.list_tags(None, 2).unwrap();
    assert_eq!(
        first
            .tags
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["a:1", "m:1"]
    );
    assert_eq!(first.next_after.as_deref(), Some("m:1"));
    let last = store.list_tags(first.next_after.as_deref(), 2).unwrap();
    assert_eq!(last.tags[0].name, "z:1");
    assert_eq!(last.next_after, None);
    assert!(store.list_tags(Some("z:1"), 1).unwrap().tags.is_empty());
    assert!(store.list_tags(None, 0).is_err());
    assert!(store.list_tags(None, 1001).is_err());
}

#[test]
fn concurrent_blob_importers_publish_one_verified_inode() {
    let dir = Temp::new();
    let store = dir.store();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let store = store.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                store.import(io::repeat(42).take(256 * 1024), None).unwrap()
            })
        })
        .collect();
    let blobs: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(blobs[0], blobs[1]);
    store.verify(&blobs[0].digest, blobs[0].size).unwrap();
    assert_eq!(
        fs::read_dir(store.root().join("images/blobs/sha256"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(store.root().join("staging")).unwrap().count(),
        0
    );
}

#[test]
fn racing_tag_updates_have_one_winner_and_one_conflict() {
    let dir = Temp::new();
    let store = dir.store();
    let images: Vec<_> = [b"one".as_slice(), b"two"]
        .iter()
        .map(|label| {
            let (m, _) = fixture(&store, label);
            store.import_manifest(&m).unwrap()
        })
        .collect();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = images
        .iter()
        .map(|image| {
            let image = image.clone();
            let store = store.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                store.update_tag("race:1", &image, None, &Provider("zx-spectrum"))
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict { current: Some(_) })))
            .count(),
        1
    );
    let winner = store.resolve("race:1", &Provider("zx-spectrum")).unwrap();
    assert!(images.iter().any(|i| i.digest == winner.image.digest));
}

#[test]
fn concurrent_unrelated_tag_updates_preserve_both_references() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let image = store.import_manifest(&manifest).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = ["a:1", "b:1"]
        .into_iter()
        .map(|name| {
            let store = store.clone();
            let image = image.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                store
                    .update_tag(name, &image, None, &Provider("zx-spectrum"))
                    .unwrap();
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(store.list_tags(None, 10).unwrap().tags.len(), 2);
}

struct FailingReader {
    reads: usize,
}
impl Read for FailingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reads += 1;
        if self.reads > 1 {
            return Err(io::Error::other("injected import interruption"));
        }
        buffer.fill(17);
        Ok(buffer.len())
    }
}

#[test]
fn interrupted_reader_leaves_no_visible_blob_or_staged_file() {
    let dir = Temp::new();
    let store = dir.store();
    let result = store.import(FailingReader { reads: 0 }, None);
    assert!(
        matches!(result, Err(Error::Io(ref e)) if e.to_string() == "injected import interruption")
    );
    assert_eq!(
        fs::read_dir(store.root().join("images/blobs/sha256"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        fs::read_dir(store.root().join("staging")).unwrap().count(),
        0
    );
    assert!(store.list_tags(None, 10).unwrap().tags.is_empty());
}

struct Generated {
    remaining: u64,
    calls: usize,
    largest_request: usize,
}
impl Read for Generated {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        assert!(
            buffer.len() <= 64 * 1024,
            "ingestion must keep its bounded buffer"
        );
        self.largest_request = self.largest_request.max(buffer.len());
        self.calls += 1;
        let count = buffer.len().min(4096).min(self.remaining as usize);
        buffer[..count].fill(0x5a);
        self.remaining -= count as u64;
        Ok(count)
    }
}

#[test]
fn large_blob_streams_sixty_four_mib_with_bounded_reads_and_full_hash() {
    let dir = Temp::new();
    let store = dir.store();
    let size = 64 * 1024 * 1024;
    let mut hasher = Sha256::new();
    for _ in 0..size / 4096 {
        hasher.update([0x5a; 4096]);
    }
    let expected: Digest = format!("sha256:{:x}", hasher.finalize()).parse().unwrap();
    let mut reader = Generated {
        remaining: size,
        calls: 0,
        largest_request: 0,
    };
    let blob = store.import(&mut reader, Some(&expected)).unwrap();
    assert_eq!(blob.size, size);
    assert_eq!(blob.digest, expected);
    assert_eq!(reader.remaining, 0);
    assert!(reader.calls > 1000);
    assert_eq!(reader.largest_request, 64 * 1024);
    store.verify(&blob.digest, size).unwrap();
    assert_eq!(
        store
            .open_blob(&blob.digest, size)
            .unwrap()
            .metadata()
            .unwrap()
            .len(),
        size
    );
}

#[test]
fn bounded_reads_refuse_oversized_descriptors_before_reading() {
    let dir = Temp::new();
    let store = dir.store();
    let descriptor = Descriptor::new(mt::CONFIG, Digest::of(b"absent"), MAX_SMALL_BLOB + 1);
    assert!(matches!(
        store.read_verified(&descriptor, u64::MAX),
        Err(Error::Invalid(_))
    ));
    let descriptor = put(&store, mt::FIRMWARE, b"bytes");
    assert!(matches!(
        store.read_verified(&descriptor, 4),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn invalid_local_tag_names_are_rejected_before_publication() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let image = store.import_manifest(&manifest).unwrap();
    for name in [
        "no-tag",
        ":tag",
        "repo:",
        "../repo:1",
        "/repo:1",
        "Repo:1",
        "repo-:1",
        "a..b:1",
        "a._b:1",
        "repo:.tag",
        "repo:-tag",
    ] {
        assert!(
            store
                .update_tag(name, &image, None, &Provider("zx-spectrum"))
                .is_err(),
            "{name}"
        );
    }
    store
        .update_tag(
            "family/repo:version-1",
            &image,
            None,
            &Provider("zx-spectrum"),
        )
        .unwrap();
}

#[test]
fn local_tags_may_start_with_a_registry_host() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let image = store.import_manifest(&manifest).unwrap();
    // A first component with a dot or a colon, or `localhost`, names a host,
    // as in Docker references; the port is part of the host.
    for (name, host) in [
        (
            "registry.lan.example/private/games/dune2:1.0.0",
            "registry.lan.example",
        ),
        ("nas.example:5000/private/media/x:1.1", "nas.example:5000"),
        ("localhost/lab:1", "localhost"),
        ("localhost:5000/lab:1", "localhost:5000"),
        ("127.0.0.1:5000/lab:2", "127.0.0.1:5000"),
        ("[::1]:5000/lab:3", "[::1]:5000"),
        (
            "registry.unemu.dev/unemu/open-at:1.2.0",
            "registry.unemu.dev",
        ),
    ] {
        store
            .update_tag(name, &image, None, &Provider("zx-spectrum"))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(Store::tag_host(name), Some(host), "{name}");
        assert_eq!(store.tag_descriptor(name).unwrap().digest, image.digest);
    }
    // A plain first component stays a repository component.
    for name in [
        "private/x:1",
        "unemu/media/freedos:1.4",
        "a.b:1",
        "localhost:1",
    ] {
        Store::validate_tag_name(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(Store::tag_host(name), None, "{name}");
    }
    let listed: Vec<_> = store
        .list_tags(None, 100)
        .unwrap()
        .tags
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(listed.len(), 7, "{listed:?}");
    // A bad host is refused with a message that names it as one.
    for name in [
        "NAS.example/x:1",
        "nas.example:/x:1",
        "nas.example:99999/x:1",
        "nas.example:50a/x:1",
        "nas..example/x:1",
        "-nas.example/x:1",
        "nas_box.example/x:1",
        "[::1/x:1",
        "[::g]:5000/x:1",
    ] {
        let error = Store::validate_tag_name(name).unwrap_err().to_string();
        assert!(error.contains("registry host"), "{name}: {error}");
        assert!(store
            .update_tag(name, &image, None, &Provider("zx-spectrum"))
            .is_err());
    }
    // So is a host without a valid repository after it.
    for name in [
        "nas.example:5000/:1",
        "nas.example:5000/Repo:1",
        "nas.example:5000/x",
    ] {
        assert!(Store::validate_tag_name(name).is_err(), "{name}");
    }
}

#[test]
fn a_tag_records_the_registry_a_pull_wrote_it_from() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let image = publish(&store, &manifest, "made-here:1");
    store
        .update_pulled_tag(
            "lab:latest",
            &image,
            None,
            &Provider("zx-spectrum"),
            "registry.unemu.dev",
        )
        .unwrap();
    store
        .update_pulled_tag(
            "nas.example:5000/private/lab:latest",
            &image,
            None,
            &Provider("zx-spectrum"),
            "nas.example:5000",
        )
        .unwrap();
    let sources = |store: &Store| -> Vec<(String, Option<String>)> {
        store
            .list_tags(None, 10)
            .unwrap()
            .tags
            .iter()
            .map(|t| (t.name.clone(), t.pulled_from().map(str::to_owned)))
            .collect()
    };
    assert_eq!(
        sources(&dir.store()),
        [
            ("lab:latest".into(), Some("registry.unemu.dev".into())),
            ("made-here:1".into(), None),
            (
                "nas.example:5000/private/lab:latest".into(),
                Some("nas.example:5000".into())
            ),
        ]
    );
    let index = fs::read_to_string(dir.0.join("images/index.json")).unwrap();
    assert!(
        index.contains(r#""unemu.pull.registry":"registry.unemu.dev""#),
        "{index}"
    );
    // Rewriting the same image keeps the record; moving the tag to an image
    // made here drops it.
    store
        .update_tag(
            "lab:latest",
            &image,
            Some(&image.digest),
            &Provider("zx-spectrum"),
        )
        .unwrap();
    assert_eq!(sources(&store)[0].1.as_deref(), Some("registry.unemu.dev"));
    let (other, _) = fixture(&store, b"made here");
    let other = store.import_manifest(&other).unwrap();
    store
        .update_tag(
            "lab:latest",
            &other,
            Some(&image.digest),
            &Provider("zx-spectrum"),
        )
        .unwrap();
    assert_eq!(sources(&store)[0], ("lab:latest".into(), None));
    // Only a registry host is recorded.
    for registry in ["", "not a host", "nas.example:5000/private"] {
        assert!(store
            .update_pulled_tag("x:1", &image, None, &Provider("zx-spectrum"), registry)
            .is_err());
    }
}

/// The pull record belongs to the tag a pull wrote. A descriptor read from
/// that tag carries it, and handing that descriptor to any other write, a new
/// tag, a move or a verified publication, must not copy it.
#[test]
fn a_pull_record_never_travels_to_a_tag_another_write_makes() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let image = store.import_manifest(&manifest).unwrap();
    store
        .update_pulled_tag(
            "lab:1.0.0",
            &image,
            None,
            &Provider("zx-spectrum"),
            "nas.example:5000",
        )
        .unwrap();
    let pulled = store.tag_descriptor("lab:1.0.0").unwrap();
    assert_eq!(
        pulled
            .annotations
            .get(image_store::PULL_REGISTRY)
            .map(String::as_str),
        Some("nas.example:5000")
    );
    let (other, _) = fixture(&store, b"made here");
    let other = publish(&store, &other, "moved:latest");
    store
        .update_tag("copy:1", &pulled, None, &Provider("zx-spectrum"))
        .unwrap();
    store
        .update_tag(
            "moved:latest",
            &pulled,
            Some(&other.digest),
            &Provider("zx-spectrum"),
        )
        .unwrap();
    let verified = store
        .verify_publication(&pulled, &Provider("zx-spectrum"))
        .unwrap();
    store
        .update_verified_tag("verified:1", &verified, None)
        .unwrap();
    let sources: Vec<_> = store
        .list_tags(None, 10)
        .unwrap()
        .tags
        .iter()
        .map(|t| (t.name.clone(), t.pulled_from().map(str::to_owned)))
        .collect();
    assert_eq!(
        sources,
        [
            ("copy:1".into(), None),
            ("lab:1.0.0".into(), Some("nas.example:5000".into())),
            ("moved:latest".into(), None),
            ("verified:1".into(), None),
        ]
    );
}

/// One registry has one spelling in the library: a port with a leading
/// zero, port 0, an over-long DNS label or a malformed IPv6 address would
/// give the same registry a second name, or name none.
#[test]
fn a_registry_host_has_one_spelling() {
    let label = "a".repeat(63);
    for host in [
        "nas.example:5000",
        "localhost:1",
        "localhost:65535",
        "[::1]",
        "[::]:5000",
        "[fe80::1:2]",
        "[1:2:3:4:5:6:7:8]:5000",
        &format!("{label}.example"),
    ] {
        Store::validate_registry_host(host).unwrap_or_else(|e| panic!("{host}: {e}"));
        Store::validate_tag_name(&format!("{host}/x:1")).unwrap_or_else(|e| panic!("{host}: {e}"));
    }
    for host in [
        "nas.example:05000",
        "nas.example:0",
        "nas.example:00",
        "nas.example:65536",
        &format!("a{label}.example"),
        "[:]",
        "[::1::2]",
        "[1:2:3:4:5:6:7]",
        "[1:2:3:4:5:6:7:8:9]",
        "[12345::1]",
        "[::1",
        "[::1]5000",
    ] {
        assert!(Store::validate_registry_host(host).is_err(), "{host}");
        let error = Store::validate_tag_name(&format!("{host}/x:1"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("registry host"), "{host}: {error}");
    }
}

#[test]
fn a_host_qualified_name_over_255_characters_says_so() {
    let host = "nas.example:5000/";
    let fits = format!("{host}{}:1", "a".repeat(255 - host.len()));
    Store::validate_tag_name(&fits).unwrap();
    let long = format!("{host}{}:1", "a".repeat(256 - host.len()));
    let error = Store::validate_tag_name(&long).unwrap_err().to_string();
    assert!(error.contains("longer than 255 characters"), "{error}");
}

/// Tags written before pulls recorded their registry carry no record: they
/// load and list as before, under the same names, and their source stays
/// unknown.
#[test]
fn a_store_without_pull_records_loads_and_lists_unchanged() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let image = store.import_manifest(&manifest).unwrap();
    let path = dir.0.join("images/index.json");
    let entry = |name: &str| {
        format!(
            r#"{{"mediaType":"{}","digest":"{}","size":{},"annotations":{{"org.opencontainers.image.ref.name":"{name}"}}}}"#,
            mt::MANIFEST,
            image.digest,
            image.size
        )
    };
    let old = format!(
        r#"{{"schemaVersion":2,"mediaType":"{}","manifests":[{},{}]}}"#,
        mt::INDEX,
        entry("open-at:1.2.0"),
        entry("private/media/x:1.1")
    );
    fs::write(&path, &old).unwrap();
    let reopened = dir.store();
    let tags = reopened.list_tags(None, 10).unwrap().tags;
    let names: Vec<_> = tags.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["open-at:1.2.0", "private/media/x:1.1"]);
    assert!(tags.iter().all(|t| t.pulled_from().is_none()));
    reopened
        .resolve("open-at:1.2.0", &Provider("zx-spectrum"))
        .unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), old);
    // A later pull adds its own tag and leaves the old entries as they were.
    reopened
        .update_pulled_tag(
            "nas.example:5000/private/media/x:1.1",
            &image,
            None,
            &Provider("zx-spectrum"),
            "nas.example:5000",
        )
        .unwrap();
    let after = fs::read_to_string(&path).unwrap();
    for name in ["open-at:1.2.0", "private/media/x:1.1"] {
        assert!(after.contains(&entry(name)), "{after}");
    }
}

#[test]
fn damaged_index_is_rejected_without_reset() {
    let dir = Temp::new();
    let store = dir.store();
    for bytes in [b"not json".as_slice(), br#"{"schemaVersion":3,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[]}"#, br#"{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[],"future":true}"#] {
        fs::write(dir.0.join("images/index.json"), bytes).unwrap();
        assert!(store.list_tags(None, 10).is_err()); assert!(Store::open(&dir.0).is_err());
        assert_eq!(fs::read(dir.0.join("images/index.json")).unwrap(), bytes);
    }
}

#[test]
fn provider_can_resolve_descriptor_types_and_verify_referenced_bytes() {
    struct ReadingProvider;
    impl ConfigValidator for ReadingProvider {
        fn platform(&self) -> &str {
            "zx-spectrum"
        }
        fn validate(
            &self,
            table: &toml::Table,
            descriptors: &[Descriptor],
            blobs: &dyn BlobSource,
        ) -> Result<Vec<Digest>> {
            let references = Provider("zx-spectrum").validate(table, descriptors, blobs)?;
            for digest in &references {
                let descriptor = descriptors
                    .iter()
                    .find(|d| &d.digest == digest)
                    .ok_or_else(|| Error::Invalid("missing provider reference".into()))?;
                if descriptor.media_type != mt::FIRMWARE
                    || blobs.read_verified(descriptor, 1024)? != b"firmware"
                {
                    return Err(Error::Invalid("unsupported synthetic firmware".into()));
                }
            }
            Ok(references)
        }
    }
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let image = store.import_manifest(&manifest).unwrap();
    store
        .update_tag("lab:1", &image, None, &ReadingProvider)
        .unwrap();
    let (bad, _) = fixture(&store, b"unsupported");
    let image = store.import_manifest(&bad).unwrap();
    assert!(store
        .update_tag("lab:2", &image, None, &ReadingProvider)
        .is_err());
}

#[test]
fn missing_required_blob_has_digest_and_repair_role() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, payload) = fixture(&store, b"firmware");
    publish(&store, &manifest, "repair:1");
    fs::remove_file(blob_path(&store, &payload)).unwrap();
    assert!(
        matches!(store.resolve("repair:1", &Provider("zx-spectrum")), Err(Error::MissingBlob { digest, media_type }) if digest == payload.digest && media_type == mt::FIRMWARE)
    );
    assert!(
        matches!(store.open_blob(&payload.digest, payload.size), Err(Error::MissingBlob { digest, .. }) if digest == payload.digest)
    );
}

#[test]
fn future_metadata_titles_are_opaque_and_artwork_still_verified() {
    let dir = Temp::new();
    let store = dir.store();
    let (mut manifest, _) = fixture(&store, b"firmware");
    let art = put(&store, mt::PNG, b"art");
    manifest.layers[1] = put(
        &store,
        mt::METADATA,
        br#"{"schema":3,"platform":"zx-spectrum","titles":{"changed":"structure"}}"#,
    );
    manifest.layers.push(art.clone());
    publish(&store, &manifest, "future:1");
    assert!(store
        .resolve("future:1", &Provider("zx-spectrum"))
        .unwrap()
        .missing_artwork
        .is_empty());
    fs::write(blob_path(&store, &art), b"bad").unwrap();
    assert!(matches!(
        store.resolve("future:1", &Provider("zx-spectrum")),
        Err(Error::DigestMismatch { .. })
    ));
    fs::remove_file(blob_path(&store, &art)).unwrap();
    assert_eq!(
        store
            .resolve("future:1", &Provider("zx-spectrum"))
            .unwrap()
            .missing_artwork,
        vec![art]
    );
    let image = store.import_manifest(&manifest).unwrap();
    assert!(store
        .update_tag("future:2", &image, None, &Provider("zx-spectrum"))
        .is_err());
    let extra = put(&store, mt::FIRMWARE, b"unreferenced after metadata");
    manifest.layers.push(extra);
    let image = store.import_manifest(&manifest).unwrap();
    assert!(store
        .update_tag("future:2", &image, None, &Provider("zx-spectrum"))
        .is_err());
}

#[test]
fn provider_declares_platform_payload_codecs_and_limits() {
    use image_store::PayloadType;
    const CMOS: &str = "application/vnd.unemu.cmos.v1";
    struct PcProvider;
    impl ConfigValidator for PcProvider {
        fn platform(&self) -> &str {
            "ibm-pc-at"
        }
        fn payload_types(&self) -> &[PayloadType] {
            &[PayloadType {
                media_type: CMOS,
                max_size: 64,
            }]
        }
        fn validate(
            &self,
            config: &toml::Table,
            descriptors: &[Descriptor],
            blobs: &dyn BlobSource,
        ) -> Result<Vec<Digest>> {
            Provider("ibm-pc-at").validate(config, descriptors, blobs)
        }
    }
    let dir = Temp::new();
    let store = dir.store();
    let payload = put(&store, CMOS, &[0; 64]);
    let cfg = config(&store, "ibm-pc-at", &[&payload.digest]);
    let metadata = put(
        &store,
        mt::METADATA,
        br#"{"schema":1,"platform":"ibm-pc-at","titles":[]}"#,
    );
    assert!(Manifest::new(
        "ibm-pc-at",
        cfg.clone(),
        vec![payload.clone(), metadata.clone()]
    )
    .is_err());
    let manifest = Manifest::new_with_types(
        "ibm-pc-at",
        cfg,
        vec![payload, metadata],
        PcProvider.payload_types(),
    )
    .unwrap();
    let bytes = manifest
        .encode_with_types(PcProvider.payload_types())
        .unwrap();
    assert!(Manifest::decode(&bytes).is_err());
    let image = store
        .import_manifest_with_types(&manifest, PcProvider.payload_types())
        .unwrap();
    store.update_tag("pc:1", &image, None, &PcProvider).unwrap();
    assert_eq!(
        store.resolve("pc:1", &PcProvider).unwrap().manifest,
        manifest
    );
    let mut oversized = manifest;
    oversized.layers[0].size = 65;
    assert!(oversized
        .encode_with_types(PcProvider.payload_types())
        .is_err());
}

#[test]
fn optional_oci_fields_and_foreign_entries_survive_local_tag_updates() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let image = publish(&store, &manifest, "ours:1");
    let index_path = dir.0.join("images/index.json");
    let mut index: serde_json::Value =
        serde_json::from_slice(&fs::read(&index_path).unwrap()).unwrap();
    let mut foreign = serde_json::to_value(&image).unwrap();
    foreign["artifactType"] = "application/example".into();
    foreign["platform"] =
        serde_json::json!({"architecture":"arm64","os":"linux","os.features":["example"]});
    foreign["urls"] = serde_json::json!(["https://example.invalid/blob"]);
    foreign["data"] = "ZXhhbXBsZQ==".into();
    let mut bare = foreign.clone();
    bare["annotations"] = serde_json::json!({"org.opencontainers.image.ref.name":"latest"});
    index["annotations"] = serde_json::json!({"example":"keep"});
    index["subject"] = foreign.clone();
    index["artifactType"] = "application/example.index".into();
    index["manifests"]
        .as_array_mut()
        .unwrap()
        .extend([foreign.clone(), bare.clone()]);
    index["manifests"][0]["platform"] = foreign["platform"].clone();
    fs::write(&index_path, serde_json::to_vec(&index).unwrap()).unwrap();
    let reopened = dir.store();
    assert_eq!(reopened.list_tags(None, 10).unwrap().tags.len(), 1);
    reopened
        .resolve("ours:1", &Provider("zx-spectrum"))
        .unwrap();
    reopened
        .update_tag("ours:2", &image, None, &Provider("zx-spectrum"))
        .unwrap();
    reopened
        .update_tag(
            "ours:1",
            &image,
            Some(&image.digest),
            &Provider("zx-spectrum"),
        )
        .unwrap();
    let after: serde_json::Value = serde_json::from_slice(&fs::read(&index_path).unwrap()).unwrap();
    for key in ["annotations", "subject", "artifactType"] {
        assert_eq!(after[key], index[key]);
    }
    assert_eq!(after["manifests"][0]["platform"], foreign["platform"]);
    assert_eq!(after["manifests"][1], foreign);
    assert_eq!(after["manifests"][2], bare);
    assert_eq!(reopened.list_tags(None, 10).unwrap().tags.len(), 2);
    // Known optional descriptor fields also decode inside the machine envelope.
    let mut json = valid_json();
    for key in ["artifactType", "platform", "urls", "data"] {
        json["config"][key] = after["manifests"][1][key].clone();
    }
    let decoded = Manifest::decode(&serde_json::to_vec(&json).unwrap()).unwrap();
    assert_eq!(
        decoded.config.urls.unwrap(),
        ["https://example.invalid/blob"]
    );
}

#[test]
fn annotations_reject_invalid_redistribution_and_base_digest_values() {
    for location in ["manifest", "config", "layer"] {
        for (key, invalid) in [
            ("unemu.redistribution", "redistributable"),
            ("org.opencontainers.image.base.digest", "sha256:ABC"),
        ] {
            let mut json = valid_json();
            let annotations = match location {
                "manifest" => &mut json["annotations"],
                "config" => &mut json["config"]["annotations"],
                _ => &mut json["layers"][0]["annotations"],
            };
            *annotations = serde_json::json!({(key): invalid});
            if location == "manifest" {
                annotations["unemu.platform"] = "c64".into();
            }
            assert!(Manifest::decode(&serde_json::to_vec(&json).unwrap()).is_err());
        }
    }
    let mut json = valid_json();
    json["annotations"]["unemu.redistribution"] = "open".into();
    json["annotations"]["org.opencontainers.image.base.digest"] =
        Digest::of(b"base").to_string().into();
    Manifest::decode(&serde_json::to_vec(&json).unwrap()).unwrap();
}

#[test]
fn interrupted_index_publication_leaves_old_index_and_allows_retry() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let image = publish(&store, &manifest, "before:1");
    let index = fs::read(dir.0.join("images/index.json")).unwrap();
    let leftover = dir.0.join("staging/tmp-crashed-index-writer");
    fs::write(&leftover, b"{partial index").unwrap();
    let reopened = dir.store();
    assert_eq!(fs::read(dir.0.join("images/index.json")).unwrap(), index);
    reopened
        .resolve("before:1", &Provider("zx-spectrum"))
        .unwrap();
    reopened
        .update_tag("after:1", &image, None, &Provider("zx-spectrum"))
        .unwrap();
    assert_eq!(reopened.list_tags(None, 10).unwrap().tags.len(), 2);
    assert_eq!(fs::read(leftover).unwrap(), b"{partial index");
}

#[test]
fn distribution_repository_and_tag_separator_forms_are_supported() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let image = store.import_manifest(&manifest).unwrap();
    for name in [
        "a.b:Tag",
        "a_b:_tag",
        "a__b:tag",
        "a---b:tag",
        "nested/a1:1.2-3_4",
    ] {
        store
            .update_tag(name, &image, None, &Provider("zx-spectrum"))
            .unwrap();
    }
}

#[test]
fn foreign_index_json_is_preserved_byte_for_byte() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let image = publish(&store, &manifest, "ours:1");
    let path = dir.0.join("images/index.json");
    let foreign = format!(
        r#"{{  "mediaType" : "application/example", "digest":"{}", "size":0, "platform":{{"opaque":1e+03}} }}"#,
        image.digest
    );
    let before = fs::read_to_string(&path)
        .unwrap()
        .replace("\"manifests\":[", &format!("\"manifests\":[{foreign},"));
    fs::write(&path, before).unwrap();
    let reopened = dir.store();
    reopened
        .update_tag("ours:2", &image, None, &Provider("zx-spectrum"))
        .unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains(&foreign));
}

#[test]
fn schema_one_catalogue_rejects_invalid_titles_before_tag_publication() {
    let dir = Temp::new();
    let store = dir.store();
    for (i, titles) in [
        serde_json::json!([{"id":"same","title":"One"},{"id":"same","title":"Two"}]),
        serde_json::json!([{"id":"missing"}]),
        serde_json::json!([{"id":"players","title":"Demo","players":{"min":0,"max":2}}]),
        serde_json::json!([{"id":"extra","title":"Demo","unknown":true}]),
        serde_json::json!([{"id":"private","title":"Demo","notes":"local only"}]),
    ]
    .into_iter()
    .enumerate()
    {
        let (mut manifest, _) = fixture(&store, b"firmware");
        let bytes = serde_json::to_vec(
            &serde_json::json!({"schema":1,"platform":"zx-spectrum","titles":titles}),
        )
        .unwrap();
        manifest.layers[1] = put(&store, mt::METADATA, &bytes);
        let image = store.import_manifest(&manifest).unwrap();
        assert!(
            store
                .update_tag(
                    &format!("invalid:{i}"),
                    &image,
                    None,
                    &Provider("zx-spectrum")
                )
                .is_err(),
            "invalid title variant {i} was published"
        );
    }
}

#[test]
fn schema_one_unknown_metadata_fields_degrade_on_resolve_but_reject_publication() {
    let dir = Temp::new();
    let store = dir.store();
    let (mut manifest, _) = fixture(&store, b"firmware");
    let bytes = br#"{"schema":1,"platform":"zx-spectrum","future":"preserved","titles":[{"id":"demo","title":"Demo","new_field":true}]}"#;
    manifest.layers[1] = put(&store, mt::METADATA, bytes);
    let mut image = store.import_manifest(&manifest).unwrap();
    assert!(store
        .update_tag("lenient:one", &image, None, &Provider("zx-spectrum"))
        .is_err());
    // Simulate a tag written by a reader which accepted additive metadata fields.
    image.annotations.insert(
        "org.opencontainers.image.ref.name".into(),
        "lenient:one".into(),
    );
    fs::write(
        store.root().join("images/index.json"),
        serde_json::to_vec(
            &serde_json::json!({"schemaVersion":2,"mediaType":mt::INDEX,"manifests":[image]}),
        )
        .unwrap(),
    )
    .unwrap();
    let resolved = store
        .resolve("lenient:one", &Provider("zx-spectrum"))
        .unwrap();
    assert!(resolved.metadata_read_only);
    assert_eq!(resolved.metadata, bytes);
    let projection =
        image_store::metadata::Metadata::decode_for_display(&resolved.metadata, &manifest.layers)
            .unwrap();
    assert_eq!(projection.titles[0].title, "Demo");
    assert!(projection.is_read_only());
    assert!(projection.encode(&manifest.layers).is_err());
    for bad in [
        br#"{"schema":1,"platform":"zx-spectrum","titles":[{"id":"missing"}]}"#.as_slice(),
        b"{broken",
        br#"{"schema":1,"platform":"c64","titles":[]}"#,
    ] {
        manifest.layers[1] = put(&store, mt::METADATA, bad);
        let mut image = store.import_manifest(&manifest).unwrap();
        assert!(store
            .update_tag("lenient:bad", &image, None, &Provider("zx-spectrum"))
            .is_err());
        image.annotations.insert(
            "org.opencontainers.image.ref.name".into(),
            "lenient:one".into(),
        );
        fs::write(
            store.root().join("images/index.json"),
            serde_json::to_vec(
                &serde_json::json!({"schemaVersion":2,"mediaType":mt::INDEX,"manifests":[image]}),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            store
                .resolve("lenient:one", &Provider("zx-spectrum"))
                .unwrap()
                .metadata_read_only
        );
    }
}

#[test]
fn publication_receipts_refuse_mutation_wrong_store_and_display_only_resolution() {
    let dir = Temp::new();
    let store = dir.store();
    let (manifest, _) = fixture(&store, b"firmware");
    let descriptor = store.import_manifest(&manifest).unwrap();
    let display = store
        .resolve_descriptor(&descriptor, &Provider("zx-spectrum"))
        .unwrap();
    assert!(store
        .update_verified_tag("receipt:display", &display, None)
        .is_err());
    let verified = store
        .verify_publication(&descriptor, &Provider("zx-spectrum"))
        .unwrap();
    let mut changed = verified.clone();
    changed.manifest.layers[0].size += 1;
    assert!(store
        .update_verified_tag("receipt:changed", &changed, None)
        .is_err());
    let another = Temp::new();
    let other_store = another.store();
    assert!(other_store
        .update_verified_tag("receipt:other", &verified, None)
        .is_err());
    store
        .update_verified_tag("receipt:valid", &verified, None)
        .unwrap();
    store
        .resolve("receipt:valid", &Provider("zx-spectrum"))
        .unwrap();
}

#[test]
fn review_busy_store_and_workspace_media_outputs() {
    let temp = Temp::new();
    let store = temp.store();
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.root().join("locks/store.lock"))
        .unwrap();
    file.lock().unwrap();
    assert!(store
        .try_read_lease()
        .unwrap_err()
        .to_string()
        .contains("store busy"));
    drop(file);
    let owner = store.workspace("review").unwrap();
    let media = image_store::location::workspace_media_dir(&owner);
    assert_eq!(media, store.root().join("workspaces/review/media"));
    assert!(store
        .validate_media_output(&media.join("disk.upfj"))
        .is_ok());
    assert!(store
        .validate_media_output(&store.root().join("images/blobs/bad.upfj"))
        .is_err());
}

#[test]
fn decoded_receipts_do_not_alias_stored_ids_or_trust_changed_bindings() {
    struct CompressedProvider;
    impl ConfigValidator for CompressedProvider {
        fn platform(&self) -> &str {
            "c64"
        }
        fn payload_types(&self) -> &[image_store::PayloadType] {
            &[image_store::PayloadType {
                media_type: "application/vnd.unemu.disk.base.v1+zstd",
                max_size: 64 * 1024,
            }]
        }
        fn validate(
            &self,
            table: &toml::Table,
            descriptors: &[Descriptor],
            blobs: &dyn BlobSource,
        ) -> Result<Vec<Digest>> {
            Provider("c64").validate(table, descriptors, blobs)
        }
    }
    let temp = Temp::new();
    let store = temp.store();
    let b = store
        .import_payload(
            [0x45; 8192].as_slice(),
            mt::DISK_BASE,
            8192,
            None,
            CompressedProvider.payload_types(),
        )
        .unwrap();
    let encoded_b = fs::read(store.blob_file_path(&b.digest, b.size).unwrap()).unwrap();
    let a_raw = Digest::of(&encoded_b);
    assert_eq!(a_raw, b.digest);
    let encoded_a = zstd::stream::encode_all(encoded_b.as_slice(), 3).unwrap();
    let blob = store.import(encoded_a.as_slice(), None).unwrap();
    let mut a = Descriptor::new(
        "application/vnd.unemu.disk.base.v1+zstd",
        blob.digest,
        blob.size,
    );
    a.annotations
        .insert(image_store::codec::DECODED_DIGEST.into(), a_raw.to_string());
    a.annotations.insert(
        image_store::codec::DECODED_SIZE.into(),
        encoded_b.len().to_string(),
    );
    let b_raw = b.decoded_digest().unwrap();
    let cfg = config(&store, "c64", &[&a_raw, &b_raw]);
    let metadata = put(
        &store,
        mt::METADATA,
        br#"{"schema":1,"platform":"c64","titles":[]}"#,
    );
    let provider = CompressedProvider;
    let manifest =
        Manifest::new_with_types("c64", cfg, vec![a, b, metadata], provider.payload_types())
            .unwrap();
    let descriptor = store
        .import_manifest_with_types(&manifest, provider.payload_types())
        .unwrap();
    let image = store.verify_publication(&descriptor, &provider).unwrap();
    let (mut file, metadata) = image.verified_file(&a_raw).unwrap();
    assert_eq!(metadata.len(), encoded_b.len() as u64);
    std::io::Seek::rewind(&mut file).unwrap();
    let mut actual = Vec::new();
    file.read_to_end(&mut actual).unwrap();
    assert_eq!(Digest::of(&actual), a_raw);
    let mut changed_source = image.clone();
    let fake = Digest::of(b"unrelated decoded identity");
    changed_source.manifest.layers[0]
        .annotations
        .insert(image_store::codec::DECODED_DIGEST.into(), fake.to_string());
    let mut changed = changed_source.manifest.clone();
    changed.config = config(&store, "c64", &[&fake, &b_raw]);
    let proposed = store
        .import_manifest_with_types(&changed, provider.payload_types())
        .unwrap();
    let error = store
        .verify_publication_reusing(&proposed, &provider, &changed_source)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&changed.layers[0].digest.to_string()),
        "{error}"
    );
}

#[test]
fn version_tags_never_move_and_other_tags_move_only_from_the_confirmed_digest() {
    let dir = Temp::new();
    let store = dir.store();
    let (first, _) = fixture(&store, b"first");
    let (second, _) = fixture(&store, b"second");
    let first = publish(&store, &first, "lab:1.0.0");
    let second = store.import_manifest(&second).unwrap();
    let provider = Provider("zx-spectrum");
    store.check_tag_target("lab:1.1.0", None, None).unwrap();
    // Without the current digest an update loses the compare-and-swap; with
    // it, a version tag still refuses to move.
    assert!(matches!(
        store.update_tag("lab:1.0.0", &second, None, &provider),
        Err(Error::Conflict { current: Some(d) }) if d == first.digest
    ));
    let error = store
        .update_tag("lab:1.0.0", &second, Some(&first.digest), &provider)
        .unwrap_err()
        .to_string();
    assert!(error.contains("version tags never move"), "{error}");
    for expected in [None, Some(&first.digest)] {
        let error = store
            .check_tag_target("lab:1.0.0", expected, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("version tags never move"), "{error}");
    }
    assert_eq!(
        store.tag_descriptor("lab:1.0.0").unwrap().digest,
        first.digest
    );
    // Writing the same digest again is not a move.
    store
        .update_tag("lab:1.0.0", &first, Some(&first.digest), &provider)
        .unwrap();
    store
        .update_tag("lab:latest", &first, None, &provider)
        .unwrap();
    assert!(matches!(
        store.check_tag_target("lab:latest", None, None),
        Err(Error::Conflict { current: Some(d) }) if d == first.digest
    ));
    store
        .check_tag_target("lab:latest", Some(&first.digest), None)
        .unwrap();
    store
        .update_tag("lab:latest", &second, Some(&first.digest), &provider)
        .unwrap();
    assert_eq!(
        store.tag_descriptor("lab:latest").unwrap().digest,
        second.digest
    );
    assert_eq!(
        store.tag_descriptor("lab:1.0.0").unwrap().digest,
        first.digest
    );
}

/// Neutral installation media: every medium is a floppy.
struct Floppies;
impl image_store::media_set::MediaFormats for Floppies {
    fn platform(&self) -> &str {
        "zx-spectrum"
    }
    fn validate_name(&self, _: &str) -> Result<()> {
        Ok(())
    }
    fn validate_format(&self, _: &toml::Table, _: u64) -> Result<&'static str> {
        Ok(mt::FLOPPY)
    }
}

/// Store an installation media set of one floppy holding `label`, untagged.
fn media_set(store: &Store, label: &[u8]) -> Descriptor {
    use image_store::media_set::{Config, Medium};
    let medium = put(store, mt::FLOPPY, label);
    let config = Config {
        schema: 1,
        platform: "zx-spectrum".into(),
        media: vec![Medium {
            id: "disk-1".into(),
            label: "Disk 1".into(),
            format: toml::Table::new(),
            blob: medium.digest.clone(),
        }],
        extras: Vec::new(),
    };
    let config = put(store, mt::MEDIA_SET_CONFIG, &config.encode().unwrap());
    let metadata = put(
        store,
        mt::METADATA,
        br#"{"schema":1,"platform":"zx-spectrum","titles":[]}"#,
    );
    let manifest = Manifest::new_media_set("zx-spectrum", config, vec![medium, metadata]).unwrap();
    store.import_manifest(&manifest).unwrap()
}

#[test]
fn media_set_tags_never_move_unless_they_are_aliases() {
    let dir = Temp::new();
    let store = dir.store();
    let sets = image_store::media_set::Validator(&Floppies);
    let first = media_set(&store, b"first set");
    let second = media_set(&store, b"second set");
    for tag in ["sets:1.4", "sets:1.4-registered-720k-r2", "sets:latest"] {
        store.update_tag(tag, &first, None, &sets).unwrap();
    }
    // Not even the confirmed current digest moves a set's tag, and the
    // refusal offers the next revision.
    for (tag, revision) in [
        ("sets:1.4", "sets:1.4-r2"),
        ("sets:1.4-registered-720k-r2", "sets:1.4-registered-720k-r3"),
    ] {
        let error = store
            .update_tag(tag, &second, Some(&first.digest), &sets)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("never move") && error.contains(revision),
            "{error}"
        );
        for expected in [None, Some(&first.digest)] {
            let error = store
                .check_tag_target(tag, expected, None)
                .unwrap_err()
                .to_string();
            assert!(error.contains("never move"), "{error}");
        }
        assert_eq!(store.tag_descriptor(tag).unwrap().digest, first.digest);
        // Writing the same digest again is not a move.
        store
            .update_tag(tag, &first, Some(&first.digest), &sets)
            .unwrap();
    }
    // An alias moves from the confirmed digest.
    store
        .check_tag_target("sets:latest", Some(&first.digest), None)
        .unwrap();
    store
        .update_tag("sets:latest", &second, Some(&first.digest), &sets)
        .unwrap();
    assert_eq!(
        store.tag_descriptor("sets:latest").unwrap().digest,
        second.digest
    );
    // A machine image keeps the version-tag rule: a tag of the same shape
    // moves from the confirmed digest.
    let provider = Provider("zx-spectrum");
    let (machine, _) = fixture(&store, b"machine");
    let (other, _) = fixture(&store, b"other machine");
    let machine = publish(&store, &machine, "lab:1.4");
    let other = store.import_manifest(&other).unwrap();
    store
        .check_tag_target("lab:1.4", Some(&machine.digest), None)
        .unwrap();
    store
        .update_tag("lab:1.4", &other, Some(&machine.digest), &provider)
        .unwrap();
    // But no set takes its tag, and it takes no set's tag.
    let error = store
        .update_tag("lab:1.4", &second, Some(&other.digest), &sets)
        .unwrap_err()
        .to_string();
    assert!(error.contains("never move"), "{error}");
    let error = store
        .update_tag("sets:1.4", &other, Some(&first.digest), &provider)
        .unwrap_err()
        .to_string();
    assert!(error.contains("never move"), "{error}");
    assert_eq!(
        store.tag_descriptor("lab:1.4").unwrap().digest,
        other.digest
    );
    assert_eq!(
        store.tag_descriptor("sets:1.4").unwrap().digest,
        first.digest
    );
}

#[test]
fn a_deleted_version_tag_can_name_only_its_old_digest_again() {
    let dir = Temp::new();
    let store = dir.store();
    let (first, _) = fixture(&store, b"first");
    let (second, _) = fixture(&store, b"second");
    let first = publish(&store, &first, "lab:1.0.0");
    let second = store.import_manifest(&second).unwrap();
    let provider = Provider("zx-spectrum");
    store
        .update_tag("lab:scratch", &second, None, &provider)
        .unwrap();
    store.remove_tag("lab:1.0.0", &first.digest).unwrap();
    store.remove_tag("lab:scratch", &second.digest).unwrap();
    assert!(matches!(
        store.tag_descriptor("lab:1.0.0"),
        Err(Error::TagNotFound(_))
    ));
    let error = store
        .update_tag("lab:1.0.0", &second, None, &provider)
        .unwrap_err()
        .to_string();
    assert!(error.contains("version tags never move"), "{error}");
    for replacement in [None, Some(&second.digest)] {
        assert!(store
            .check_tag_target("lab:1.0.0", None, replacement)
            .is_err());
    }
    // The same image may take its version tag back; other tags stay free.
    store
        .check_tag_target("lab:1.0.0", None, Some(&first.digest))
        .unwrap();
    store
        .update_tag("lab:1.0.0", &first, None, &provider)
        .unwrap();
    store
        .update_tag("lab:scratch", &first, None, &provider)
        .unwrap();
    // The record survives reopening the store.
    store.remove_tag("lab:1.0.0", &first.digest).unwrap();
    let reopened = Store::open(&dir.0).unwrap();
    assert!(reopened
        .update_tag("lab:1.0.0", &second, None, &provider)
        .is_err());
}
