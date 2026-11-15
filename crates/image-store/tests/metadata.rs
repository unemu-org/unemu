// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic platform-neutral catalogue, overlay and summary acceptance tests.

use image_store::{
    library::{CatalogueEdit, Personal, TitleEdit},
    media_type as mt,
    metadata::{Metadata, Players, Title},
    summary::{Facets, SummaryCache},
    BlobSource, ConfigValidator, Descriptor, Digest, Error, Manifest, Result, Store,
    MAX_SMALL_BLOB,
};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "image-catalogue-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
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
struct Provider;
impl ConfigValidator for Provider {
    fn platform(&self) -> &str {
        "c64"
    }
    fn validate(
        &self,
        _: &toml::Table,
        _: &[Descriptor],
        _: &dyn BlobSource,
    ) -> Result<Vec<Digest>> {
        Ok(vec![])
    }
}
fn put(store: &Store, kind: &str, bytes: &[u8]) -> Descriptor {
    let blob = store.import(bytes, None).unwrap();
    Descriptor::new(kind, blob.digest, blob.size)
}
fn title(id: &str) -> Title {
    Title {
        id: id.into(),
        title: format!("Title {id}"),
        ..Title::default()
    }
}
fn catalogue() -> Metadata {
    let mut metadata = Metadata::new("c64");
    metadata.labels = vec!["travel".into()];
    metadata.titles = vec![Title { sort_title: Some("Sort".into()), series: Some("Saga".into()),
        genres: vec!["adventure".into(), "puzzle".into()], developers: vec!["Dev A".into(),"Dev B".into()],
        publishers: vec!["Publisher".into()], platform: Some("Future software environment".into()),
        release_year: Some(1987), players: Some(Players { min:1, max:2 }), languages: vec!["English".into()],
        regions: vec!["Europe".into()], description: Some("Moonlit labyrinth".into()), version: Some("Versionword".into()),
        edition: Some("Editionword".into()), source_media: Some("Cartridgeword".into()), tags: vec!["tagword".into()],
        launch: Some(serde_json::from_value(json!({"profile":"providerword", "opaque":{"nested":"launchword", "ordered":[3,1]}})).unwrap()),
        ..title("alpha") }, Title { genres: vec!["racing".into()], publishers: vec!["Other".into()], ..title("beta") }];
    metadata
}
fn fixture(store: &Store, metadata: &Metadata, art: &[Descriptor]) -> (Descriptor, Manifest) {
    let config = put(store, mt::CONFIG, b"schema = 1\nplatform = \"c64\"\n");
    let bytes = metadata.encode(art).unwrap();
    let mut layers = vec![put(store, mt::METADATA, &bytes)];
    layers.extend_from_slice(art);
    let manifest = Manifest::new("c64", config, layers).unwrap();
    let image = store.import_manifest(&manifest).unwrap();
    (image, manifest)
}
fn publish(store: &Store, metadata: &Metadata, name: &str) -> Descriptor {
    let (image, _) = fixture(store, metadata, &[]);
    store.update_tag(name, &image, None, &Provider).unwrap();
    image
}
fn edit_description(entry: &mut image_store::library::LibraryEntry) -> Result<()> {
    let mut edit = TitleEdit::default();
    edit.set("description", json!("Published description"))?;
    entry.catalogue.titles.insert("alpha".into(), edit);
    entry.personal.insert(
        "alpha".into(),
        Personal {
            user_rating: Some(5),
            notes: Some("Personal secretword".into()),
        },
    );
    Ok(())
}

#[test]
fn codec_round_trip_is_complete_deterministic_and_provider_neutral() {
    let metadata = catalogue();
    let bytes = metadata.encode(&[]).unwrap();
    let decoded = Metadata::decode(&bytes, &[]).unwrap();
    assert_eq!(decoded, metadata);
    assert_eq!(decoded.encode(&[]).unwrap(), bytes);
    assert_eq!(
        Digest::of(&decoded.encode(&[]).unwrap()),
        Digest::of(&bytes)
    );
    assert_eq!(
        decoded.titles[0].platform.as_deref(),
        Some("Future software environment")
    );
    let mut launch = json!({"schema":1,"platform":"c64","labels":[],"titles":[{"id":"opaque","title":"Opaque","launch":{"z":3,"profile":"any","a":{"z":2,"a":1}}}]});
    let one = Metadata::decode(&serde_json::to_vec(&launch).unwrap(), &[])
        .unwrap()
        .encode(&[])
        .unwrap();
    launch["titles"][0]["launch"] = json!({"a":{"a":1,"z":2},"profile":"any","z":3});
    assert_eq!(
        one,
        Metadata::decode(&serde_json::to_vec(&launch).unwrap(), &[])
            .unwrap()
            .encode(&[])
            .unwrap()
    );
}

#[test]
fn strict_codec_rejects_invalid_records_artwork_and_unknown_fields() {
    for bad in [
        json!({"id":"a"}),
        json!({"title":"No id"}),
        json!({"id":"","title":"Empty"}),
        json!({"id":"a","title":""}),
        json!({"id":"a","title":"A","players":{"min":0,"max":2}}),
        json!({"id":"a","title":"A","players":{"min":3,"max":2}}),
        json!({"id":"a","title":"A","players":{"min":-1,"max":2}}),
        json!({"id":"a","title":"A","players":{"min":1,"max":2,"extra":0}}),
        json!({"id":"a","title":"A","user_rating":3}),
        json!({"id":"a","title":"A","notes":"private"}),
        json!({"id":"a","title":"A","extra":true}),
        json!({"id":"a","title":"A","launch":{}}),
        json!({"id":"a","title":"A","launch":"command"}),
        json!({"id":"a","title":"A","cover":Digest::of(b"unlisted")}),
        json!({"id":"a","title":"A","screenshots":[Digest::of(b"unlisted")]}),
    ] {
        let bytes =
            serde_json::to_vec(&json!({"schema":1,"platform":"c64","titles":[bad]})).unwrap();
        assert!(Metadata::decode(&bytes, &[]).is_err(), "accepted {bad}");
    }
    for bad in [
        json!({"schema":1,"platform":"c64","titles":[{"id":"a","title":"One"},{"id":"a","title":"Two"}]}),
        json!({"schema":1,"platform":"c64","titles":[],"extra":true}),
        json!({"schema":0,"platform":"c64","titles":[]}),
        json!({"schema":1,"platform":"bad/platform","titles":[]}),
    ] {
        assert!(Metadata::decode(&serde_json::to_vec(&bad).unwrap(), &[]).is_err());
    }
    assert!(Metadata::decode(
        br#"{"schema":1,"schema":1,"platform":"c64","titles":[]}"#,
        &[]
    )
    .is_err());
    assert!(Metadata::decode(&vec![b' '; MAX_SMALL_BLOB as usize + 1], &[]).is_err());
    let digest = Digest::of(b"wrong type");
    let mut metadata = Metadata::new("c64");
    metadata.titles.push(Title {
        cover: Some(digest.clone()),
        ..title("a")
    });
    assert!(metadata
        .encode(&[Descriptor::new(mt::FIRMWARE, digest, 10)])
        .is_err());
}

#[test]
fn artwork_preserves_first_use_and_future_schemas_are_read_only_projections() {
    let art = Descriptor::new(mt::PNG, Digest::of(b"art"), 3);
    let mut metadata = catalogue();
    metadata.titles[0].cover = Some(art.digest.clone());
    metadata.titles[0].screenshots = vec![art.digest.clone(), art.digest.clone()];
    let bytes = metadata.encode(std::slice::from_ref(&art)).unwrap();
    assert_eq!(Metadata::decode(&bytes, &[art]).unwrap(), metadata);
    let future = json!({"schema":42,"platform":"c64","extra":true,"labels":["Known", 3],"titles":[
        {"id":"a","title":"Readable","genres":["demo"],"description":"Known description","players":"changed","new_field":true},
        {"id":"b","title":"Second","launch":[1,2]}, {"id":"broken","title":8}]});
    let decoded = Metadata::decode(&serde_json::to_vec(&future).unwrap(), &[]).unwrap();
    assert_eq!(decoded.schema(), 42);
    assert!(decoded.is_read_only());
    assert_eq!(decoded.titles.len(), 2);
    assert_eq!(
        decoded.titles[0].description.as_deref(),
        Some("Known description")
    );
    assert_eq!(decoded.titles[0].genres, ["demo"]);
    assert_eq!(decoded.titles[0].players, None);
    assert!(decoded.encode(&[]).is_err());
    assert!(CatalogueEdit::default().apply(&decoded).is_err());
}

#[test]
fn local_edits_round_trip_revisions_and_publish_exclude_all_personal_data() {
    let temp = Temp::new();
    let store = temp.store();
    let original = catalogue();
    let image = publish(&store, &original, "demo:original");
    let before = fs::read_dir(store.root().join("images/blobs/sha256"))
        .unwrap()
        .count();
    store.edit_library(&image.digest, edit_description).unwrap();
    store
        .edit_library(&image.digest, |entry| {
            entry.labels.push("local".into());
            entry.favorite = true;
            entry.last_run = Some("recorded externally".into());
            Ok(())
        })
        .unwrap();
    let library = temp.store().read_library().unwrap();
    let entry = &library.images[&image.digest];
    assert_eq!(
        (
            entry.overlay_revision,
            entry.personal_revision,
            entry.local_revision
        ),
        (1, 1, 1)
    );
    assert_eq!(entry.personal["alpha"].user_rating, Some(5));
    assert_eq!(
        before,
        fs::read_dir(store.root().join("images/blobs/sha256"))
            .unwrap()
            .count()
    );
    store.edit_library(&image.digest, |_| Ok(())).unwrap();
    assert_eq!(store.read_library().unwrap(), library);
    let new_image = store
        .publish_overlay(&image, "demo:edited", &Provider)
        .unwrap();
    assert_ne!(new_image.digest, image.digest);
    let old = store.resolve("demo:original", &Provider).unwrap();
    let new = store.resolve("demo:edited", &Provider).unwrap();
    assert_eq!(old.manifest.config, new.manifest.config);
    let decoded = Metadata::decode(&new.metadata, &new.manifest.layers).unwrap();
    assert_eq!(
        decoded.titles[0].description.as_deref(),
        Some("Published description")
    );
    assert_eq!(
        Metadata::decode(&old.metadata, &old.manifest.layers).unwrap(),
        original
    );
    for path in fs::read_dir(store.root().join("images/blobs/sha256")).unwrap() {
        let bytes = fs::read(path.unwrap().path()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        for private in [
            "user_rating",
            "notes",
            "Personal secretword",
            "recorded externally",
        ] {
            assert!(!text.contains(private), "leaked {private}");
        }
    }
    assert!(matches!(
        store.publish_overlay(&image, "demo:original", &Provider),
        Err(Error::Conflict { .. })
    ));
    let after = store.read_library().unwrap();
    let source = &after.images[&image.digest];
    assert!(source.catalogue.is_empty());
    assert!(source.personal.is_empty());
    assert!(source.labels.is_empty());
    assert!(!source.favorite);
    let destination = &after.images[&new_image.digest];
    assert_eq!(destination.personal, entry.personal);
    assert_eq!(destination.labels, ["local"]);
    assert!(destination.favorite);
    assert_eq!(
        new.manifest.annotations["org.opencontainers.image.base.digest"],
        image.digest.to_string()
    );
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 64, &[], &[]).unwrap();
    let results = cache.query("secretword", &Facets::default());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].reference, "demo:edited");
}

#[test]
fn overlay_artwork_is_rooted_and_publication_verifies_it() {
    let temp = Temp::new();
    let store = temp.store();
    let image = publish(&store, &catalogue(), "demo:one");
    let art = put(&store, mt::PNG, b"new art");
    store
        .edit_library(&image.digest, |entry| {
            let mut edit = TitleEdit::default();
            edit.set("cover", json!(art.digest))?;
            edit.set("screenshots", json!([art.digest]))?;
            entry.catalogue.titles.insert("alpha".into(), edit);
            entry.catalogue.artwork.push(art.clone());
            Ok(())
        })
        .unwrap();
    let roots = store.read_library().unwrap().gc_roots().unwrap();
    assert!(roots.contains(&image.digest));
    assert!(roots.contains(&art.digest));
    let mut roots_without_descriptors = store.read_library().unwrap();
    roots_without_descriptors
        .images
        .get_mut(&image.digest)
        .unwrap()
        .catalogue
        .artwork
        .clear();
    assert!(roots_without_descriptors
        .gc_roots()
        .unwrap()
        .contains(&art.digest));
    let published_art = store
        .publish_overlay(&image, "demo:art", &Provider)
        .unwrap();
    let resolved = store.resolve("demo:art", &Provider).unwrap();
    assert_eq!(resolved.manifest.layers.last(), Some(&art));
    fs::remove_file(
        store
            .root()
            .join("images/blobs/sha256")
            .join(art.digest.hex()),
    )
    .unwrap();
    assert!(store
        .publish_overlay(&published_art, "demo:missing", &Provider)
        .is_err());
    assert_eq!(store.list_tags(None, 100).unwrap().tags.len(), 2);
}

#[test]
fn invalid_and_newer_library_files_are_preserved_and_never_overwritten() {
    let temp = Temp::new();
    let store = temp.store();
    let image = Digest::of(b"source");
    store.edit_library(&image, |_| Ok(())).unwrap();
    fs::create_dir_all(store.root().join("local")).unwrap();
    let path = store.root().join("local/library.toml");
    for bytes in [
        "invalid = [",
        "schema = 2\n",
        "schema = 1\nunknown = true\n",
    ] {
        fs::write(&path, bytes).unwrap();
        assert!(store.read_library().is_err());
        assert!(store.edit_library(&image, |_| Ok(())).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
    }
}

#[test]
fn personal_rating_and_patch_validation_are_atomic() {
    let temp = Temp::new();
    let store = temp.store();
    let image = Digest::of(b"image");
    for rating in [0, 5] {
        store
            .edit_library(&image, |entry| {
                entry.personal.insert(
                    "alpha".into(),
                    Personal {
                        user_rating: Some(rating),
                        notes: None,
                    },
                );
                Ok(())
            })
            .unwrap();
    }
    let before = store.read_library().unwrap();
    assert!(store
        .edit_library(&image, |entry| {
            entry.personal.get_mut("alpha").unwrap().user_rating = Some(6);
            Ok(())
        })
        .is_err());
    assert_eq!(before, store.read_library().unwrap());
    for field in ["id", "notes", "user_rating", "unknown", "schema"] {
        assert!(TitleEdit::default().set(field, json!("invalid")).is_err());
    }
    assert!(TitleEdit::default()
        .set("players", json!({"min":3,"max":1}))
        .is_err());
    let mut edit = TitleEdit::default();
    edit.set("description", serde_json::Value::Null).unwrap();
    edit.set("genres", serde_json::Value::Null).unwrap();
    let patch = CatalogueEdit {
        titles: [("alpha".into(), edit)].into(),
        ..CatalogueEdit::default()
    };
    let cleared = patch.apply(&catalogue()).unwrap();
    assert_eq!(cleared.titles[0].description, None);
    assert!(cleared.titles[0].genres.is_empty());
}

#[test]
fn concurrent_library_edits_preserve_both_writers_and_revision_increments() {
    let temp = Temp::new();
    let store = temp.store();
    let image = Digest::of(b"source");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let handles: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|label| {
            let store = store.clone();
            let image = image.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                loop {
                    match store.edit_library(&image, |entry| {
                        entry.labels.push(label.into());
                        Ok(())
                    }) {
                        Ok(()) => break,
                        Err(Error::LibraryConflict) => continue,
                        Err(error) => panic!("edit failed: {error}"),
                    }
                }
            })
        })
        .collect();
    barrier.wait();
    for handle in handles {
        handle.join().unwrap();
    }
    let entry = store.read_library().unwrap().images.remove(&image).unwrap();
    assert_eq!(entry.labels.len(), 2);
    assert!(entry.labels.contains(&"first".into()));
    assert!(entry.labels.contains(&"second".into()));
    assert_eq!(entry.local_revision, 2);
}

#[test]
fn cache_facets_match_the_same_title_and_search_covers_all_text_fields() {
    let temp = Temp::new();
    let store = temp.store();
    let image = publish(&store, &catalogue(), "demo:one");
    store
        .edit_library(&image.digest, |entry| {
            entry.personal.insert(
                "alpha".into(),
                Personal {
                    notes: Some("Personalword".into()),
                    user_rating: Some(4),
                },
            );
            entry.labels.push("localword".into());
            Ok(())
        })
        .unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    assert_eq!(cache.pending().len(), 1);
    assert!(cache.query("", &Facets::default()).is_empty());
    assert_eq!(cache.rebuild(&store, 1, &[], &[]).unwrap(), 1);
    for term in [
        "alpha",
        "sort",
        "saga",
        "adventure",
        "puzzle",
        "dev",
        "publisher",
        "future",
        "1987",
        "english",
        "europe",
        "moonlit",
        "labyrinth",
        "versionword",
        "editionword",
        "cartridgeword",
        "tagword",
        "providerword",
        "launchword",
        "personalword",
        "localword",
        "travel",
        "demo",
        "one",
    ] {
        let results = cache.query(term, &Facets::default());
        assert_eq!(results.len(), 1, "missing term {term}");
    }
    let mixed = Facets {
        genres: vec!["adventure".into()],
        publishers: vec!["Other".into()],
        ..Facets::default()
    };
    assert!(cache.query("", &mixed).is_empty());
    let matching = Facets {
        genres: vec!["adventure".into()],
        platforms: vec!["Future software environment".into()],
        years: vec![1987],
        developers: vec!["Dev B".into()],
        publishers: vec!["Publisher".into()],
        series: vec!["Saga".into()],
        labels: vec!["localword".into(), "travel".into()],
        ..Facets::default()
    };
    assert_eq!(cache.query("moon", &matching)[0].title_ids, ["alpha"]);
    assert_eq!(
        cache.query("", &Facets::default())[0].title_ids,
        ["alpha", "beta"]
    );
}

#[test]
fn cache_invalidates_local_revisions_and_tag_mappings_and_prioritizes_visible_rows() {
    let temp = Temp::new();
    let store = temp.store();
    let image = publish(&store, &catalogue(), "demo:one");
    let mut second = catalogue();
    second.titles[0].title = "Replacement".into();
    let other = publish(&store, &second, "demo:two");
    let mut cache = SummaryCache::open(&store).unwrap();
    assert_eq!(cache.rebuild(&store, 1, &["demo:two"], &[]).unwrap(), 1);
    assert!(cache.summary(&other.digest).is_some());
    assert!(cache.summary(&image.digest).is_none());
    cache.rebuild(&store, 64, &[], &[]).unwrap();
    let metadata_digest = cache.summary(&image.digest).unwrap().metadata.clone();
    store.edit_library(&image.digest, edit_description).unwrap();
    cache.refresh(&store).unwrap();
    assert!(cache.summary(&image.digest).is_none());
    assert!(cache.summary(&other.digest).is_some());
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    assert_eq!(
        cache.summary(&image.digest).unwrap().metadata,
        metadata_digest
    );
    assert_eq!(cache.query("secretword", &Facets::default()).len(), 1);
    store
        .edit_library(&image.digest, |entry| {
            entry.personal.get_mut("alpha").unwrap().notes = Some("Revisedword".into());
            Ok(())
        })
        .unwrap();
    cache.refresh(&store).unwrap();
    assert!(cache.summary(&image.digest).is_none());
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    assert!(cache.query("secretword", &Facets::default()).is_empty());
    assert_eq!(cache.query("revisedword", &Facets::default()).len(), 1);
    store
        .update_tag("demo:one", &other, Some(&image.digest), &Provider)
        .unwrap();
    cache.refresh(&store).unwrap();
    assert!(cache.pending().is_empty());
    assert_eq!(cache.query("replacement", &Facets::default()).len(), 2);
    second.titles[0].description = Some("Newmetadatum".into());
    let (newer, _) = fixture(&store, &second, &[]);
    store
        .update_tag("demo:one", &newer, Some(&other.digest), &Provider)
        .unwrap();
    cache.refresh(&store).unwrap();
    assert_eq!(cache.pending().len(), 1);
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    assert_ne!(
        cache.summary(&newer.digest).unwrap().metadata,
        cache.summary(&other.digest).unwrap().metadata
    );
    assert_eq!(cache.query("newmetadatum", &Facets::default()).len(), 1);
}

#[test]
fn older_summary_caches_without_decoded_sizes_are_rebuilt() {
    let temp = Temp::new();
    let store = temp.store();
    let image = publish(&store, &catalogue(), "demo:one");
    let path = store.root().join("cache/summaries/index.json");
    // Schemas 3 and 4 (the firmware-package format) predate decoded sizes,
    // and schema 5 predates firmware licences and labels.
    for schema in [3, 4, 5] {
        let mut cache = SummaryCache::open(&store).unwrap();
        while !cache.pending().is_empty() {
            cache.rebuild(&store, 1, &[], &[]).unwrap();
        }
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["schema"], 6);
        value["schema"] = schema.into();
        value["images"][image.digest.to_string()]
            .as_object_mut()
            .unwrap()
            .remove("decoded_size")
            .unwrap();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let mut cache = SummaryCache::open(&store).unwrap();
        assert!(cache.summary(&image.digest).is_none(), "schema {schema}");
        assert_eq!(cache.pending().len(), 1);
        cache.rebuild(&store, 1, &[], &[]).unwrap();
        let row = cache.summary(&image.digest).unwrap();
        assert_eq!(row.decoded_size, row.stored_size);
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["schema"], 6);
    }
}

#[test]
fn corrupt_cache_rebuilds_but_corrupt_library_is_not_discarded() {
    let temp = Temp::new();
    let store = temp.store();
    let image = publish(&store, &catalogue(), "demo:one");
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    fs::write(store.root().join("cache/summaries/index.json"), b"{broken").unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    assert_eq!(cache.pending().len(), 1);
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    assert_eq!(SummaryCache::open(&store).unwrap().pending().len(), 0);
    store.edit_library(&image.digest, |_| Ok(())).unwrap();
    fs::create_dir_all(store.root().join("local")).unwrap();
    fs::write(store.root().join("local/library.toml"), b"broken").unwrap();
    let cache = SummaryCache::open(&store).unwrap();
    assert!(cache.is_read_only());
    assert!(cache.library_error().is_some());
    assert_eq!(cache.tags().len(), 1);
    assert!(store.edit_library(&image.digest, |_| Ok(())).is_err());
}

#[test]
fn thousand_image_warm_start_and_queries_read_zero_payload_blobs() {
    let temp = Temp::new();
    let store = temp.store();
    let config = put(&store, mt::CONFIG, b"schema=1\nplatform=\"c64\"\n");
    let blob_dir = store.root().join("images/blobs/sha256");
    let mut tags = Vec::new();
    for i in 0..1000 {
        let mut metadata = catalogue();
        metadata.titles[0].title = format!("Synthetic {i}");
        let bytes = metadata.encode(&[]).unwrap();
        let digest = Digest::of(&bytes);
        fs::write(blob_dir.join(digest.hex()), &bytes).unwrap();
        let descriptor = Descriptor::new(mt::METADATA, digest, bytes.len() as u64);
        let manifest = Manifest::new("c64", config.clone(), vec![descriptor]).unwrap();
        let bytes = manifest.encode().unwrap();
        let digest = Digest::of(&bytes);
        fs::write(blob_dir.join(digest.hex()), &bytes).unwrap();
        let mut descriptor = Descriptor::new(mt::MANIFEST, digest, bytes.len() as u64);
        descriptor.annotations.insert(
            "org.opencontainers.image.ref.name".into(),
            format!("synthetic-{i}:one"),
        );
        tags.push(descriptor);
    }
    fs::write(
        store.root().join("images/index.json"),
        serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":mt::INDEX,"manifests":tags}))
            .unwrap(),
    )
    .unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    assert_eq!(cache.pending().len(), 1000);
    assert_eq!(
        cache
            .rebuild(&store, usize::MAX, &["synthetic-999:one"], &[])
            .unwrap(),
        64
    );
    assert!(cache.summary(&tags[999].digest).is_some());
    while !cache.pending().is_empty() {
        cache.rebuild(&store, 64, &[], &[]).unwrap();
    }
    // Making every blob inaccessible proves that startup and queries need none.
    fs::rename(&blob_dir, store.root().join("hidden-blobs")).unwrap();
    let started = std::time::Instant::now();
    let warm = SummaryCache::open(&store).unwrap();
    assert!(warm.pending().is_empty());
    let facets = Facets {
        genres: vec!["adventure".into()],
        years: vec![1987],
        ..Facets::default()
    };
    assert_eq!(warm.query("moonlit", &facets).len(), 1000);
    assert_eq!(warm.query("synthetic 999", &facets).len(), 1);
    assert_eq!(
        warm.query(
            "adventure",
            &Facets {
                publishers: vec!["Other".into()],
                ..Facets::default()
            }
        )
        .len(),
        0
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "warm startup and three queries exceeded the generous host-side bound"
    );
}

#[test]
fn newer_metadata_can_be_summarized_but_overlay_publication_is_refused() {
    let temp = Temp::new();
    let store = temp.store();
    let config = put(&store, mt::CONFIG, b"schema=1\nplatform=\"c64\"\n");
    let metadata=put(&store,mt::METADATA,br#"{"schema":2,"platform":"c64","unknown":true,"titles":[{"id":"a","title":"Future readable","description":"Searchable future text","new_field":12}]}"#);
    let manifest = Manifest::new("c64", config, vec![metadata]).unwrap();
    let image = store.import_manifest(&manifest).unwrap();
    store
        .update_tag("future:one", &image, None, &Provider)
        .unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    assert_eq!(cache.query("searchable", &Facets::default()).len(), 1);
    assert!(cache
        .summary(&image.digest)
        .unwrap()
        .metadata_is_read_only());
    assert!(store
        .publish_overlay(&image, "future:rewrite", &Provider)
        .is_err());
    assert_eq!(store.list_tags(None, 100).unwrap().tags.len(), 1);
}

#[test]
fn local_labels_favorites_and_last_run_invalidate_only_local_summary_values() {
    let temp = Temp::new();
    let store = temp.store();
    let image = publish(&store, &catalogue(), "demo:one");
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    store
        .edit_library(&image.digest, |entry| {
            entry.labels.push("collectionword".into());
            entry.favorite = true;
            entry.last_run = Some("external timestamp".into());
            Ok(())
        })
        .unwrap();
    cache.refresh(&store).unwrap();
    assert!(cache.summary(&image.digest).is_none());
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    let row = cache.summary(&image.digest).unwrap();
    assert!(row.favorite);
    assert_eq!(row.last_run.as_deref(), Some("external timestamp"));
    assert_eq!(cache.query("collectionword", &Facets::default()).len(), 1);
    assert_eq!(
        (
            row.overlay_revision,
            row.personal_revision,
            row.local_revision
        ),
        (0, 0, 1)
    );
}

#[test]
fn personal_only_edits_create_no_metadata_or_machine_revision() {
    let temp = Temp::new();
    let store = temp.store();
    let (_, mut manifest) = fixture(&store, &catalogue(), &[]);
    let noncanonical = br#" { "titles": [{ "title": "Title alpha", "id": "alpha" }], "platform": "c64", "schema": 1 } "#;
    manifest.layers[0] = put(&store, mt::METADATA, noncanonical);
    let image = store.import_manifest(&manifest).unwrap();
    store
        .update_tag("demo:one", &image, None, &Provider)
        .unwrap();
    let count = fs::read_dir(store.root().join("images/blobs/sha256"))
        .unwrap()
        .count();
    let empty = store
        .publish_overlay(&image, "demo:empty", &Provider)
        .unwrap();
    assert_eq!(empty.digest, image.digest);
    assert!(!store.root().join("local/library.toml").exists());
    store
        .edit_library(&image.digest, |entry| {
            entry.personal.insert(
                "alpha".into(),
                Personal {
                    user_rating: Some(2),
                    notes: Some("Local personal text".into()),
                },
            );
            Ok(())
        })
        .unwrap();
    let alias = store
        .publish_overlay(&image, "demo:alias", &Provider)
        .unwrap();
    assert_eq!(alias.digest, image.digest);
    assert_eq!(
        count,
        fs::read_dir(store.root().join("images/blobs/sha256"))
            .unwrap()
            .count()
    );
}

#[test]
fn edited_metadata_does_not_inherit_the_old_blobs_redistribution_evidence() {
    let temp = Temp::new();
    let store = temp.store();
    let (_, mut manifest) = fixture(&store, &catalogue(), &[]);
    manifest.layers[0].annotations.extend([
        ("unemu.redistribution".into(), "open".into()),
        ("unemu.license".into(), "CC0-1.0".into()),
        ("unemu.provenance".into(), "Synthetic test catalogue".into()),
    ]);
    let image = store.import_manifest(&manifest).unwrap();
    store
        .update_tag("reviewed:one", &image, None, &Provider)
        .unwrap();
    store
        .edit_library(&image.digest, |entry| {
            entry.personal.insert(
                "alpha".into(),
                Personal {
                    user_rating: Some(5),
                    notes: None,
                },
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store
            .publish_overlay(&image, "reviewed:alias", &Provider)
            .unwrap()
            .digest,
        image.digest
    );
    store.edit_library(&image.digest, edit_description).unwrap();
    store
        .publish_overlay(&image, "reviewed:edited", &Provider)
        .unwrap();
    let edited = store.resolve("reviewed:edited", &Provider).unwrap();
    for mark in ["unemu.redistribution", "unemu.license", "unemu.provenance"] {
        assert!(
            !edited.manifest.layers[0].annotations.contains_key(mark),
            "new bytes inherited {mark}"
        );
    }
    let original = store.resolve("reviewed:one", &Provider).unwrap();
    assert_eq!(
        original.manifest.layers[0].annotations["unemu.redistribution"],
        "open"
    );
}

#[test]
fn damaged_first_images_do_not_starve_good_images_and_failures_survive_restart() {
    let temp = Temp::new();
    let store = temp.store();
    let mut descriptors = Vec::new();
    for (name, bytes, platform) in [
        ("a-missing:one", None, "c64"),
        (
            "b-strict:one",
            Some(br#"{"schema":1,"platform":"c64","titles":[{"id":"bad"}]}"#.as_slice()),
            "c64",
        ),
        (
            "c-platform:one",
            Some(br#"{"schema":1,"platform":"zx-spectrum","titles":[]}"#.as_slice()),
            "c64",
        ),
    ] {
        let (_, mut manifest) = fixture(&store, &catalogue(), &[]);
        manifest
            .annotations
            .insert("unemu.platform".into(), platform.into());
        manifest
            .annotations
            .insert("org.opencontainers.image.title".into(), name.into());
        if let Some(bytes) = bytes {
            manifest.layers[0] = put(&store, mt::METADATA, bytes);
        }
        let mut image = store.import_manifest(&manifest).unwrap();
        if bytes.is_none() {
            fs::remove_file(
                store
                    .root()
                    .join("images/blobs/sha256")
                    .join(image.digest.hex()),
            )
            .unwrap();
        }
        image
            .annotations
            .insert("org.opencontainers.image.ref.name".into(), name.into());
        descriptors.push(image);
    }
    let good = publish(&store, &catalogue(), "z-good:one");
    let mut good_tag = good.clone();
    good_tag.annotations.insert(
        "org.opencontainers.image.ref.name".into(),
        "z-good:one".into(),
    );
    descriptors.push(good_tag);
    fs::write(
        store.root().join("images/index.json"),
        serde_json::to_vec(
            &json!({"schemaVersion":2,"mediaType":mt::INDEX,"manifests":descriptors}),
        )
        .unwrap(),
    )
    .unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    assert_eq!(cache.rebuild(&store, 64, &[], &[]).unwrap(), 4);
    assert!(cache.pending().is_empty());
    for image in &descriptors[..3] {
        assert!(!cache.failure(&image.digest).unwrap().reason.is_empty());
    }
    assert!(cache.summary(&good.digest).is_some());
    let warm = SummaryCache::open(&store).unwrap();
    assert!(warm.pending().is_empty());
    assert_eq!(warm.query("moonlit", &Facets::default()).len(), 1);
    assert!(warm
        .failure(&descriptors[0].digest)
        .unwrap()
        .reason
        .contains("missing"));
    cache.retry(&descriptors[0].digest);
    assert_eq!(cache.pending().len(), 1);
}

#[test]
fn summary_accepts_unknown_provider_media_types_without_enabling_execution() {
    let temp = Temp::new();
    let store = temp.store();
    let (_, manifest) = fixture(&store, &catalogue(), &[]);
    let mut value = serde_json::to_value(&manifest).unwrap();
    value["layers"].as_array_mut().unwrap().insert(0, json!({"mediaType":"application/vnd.example.cartridge","digest":Digest::of(b"cartridge"),"size":9}));
    let bytes = serde_json::to_vec(&value).unwrap();
    assert!(Manifest::decode(&bytes).is_err());
    let mut image = put(&store, mt::MANIFEST, &bytes);
    image.annotations.insert(
        "org.opencontainers.image.ref.name".into(),
        "mixed:one".into(),
    );
    fs::write(
        store.root().join("images/index.json"),
        serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":mt::INDEX,"manifests":[image]}))
            .unwrap(),
    )
    .unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    assert!(cache.failure(&image.digest).is_none());
    assert_eq!(cache.query("moonlit", &Facets::default()).len(), 1);
    assert!(store.resolve("mixed:one", &Provider).is_err());
}

#[test]
fn unknown_overlay_title_is_skipped_with_a_warning_and_cannot_be_published() {
    let temp = Temp::new();
    let store = temp.store();
    let image = publish(&store, &catalogue(), "unknown:one");
    store
        .edit_library(&image.digest, |entry| {
            let mut edit = TitleEdit::default();
            edit.set("title", json!("Unavailable"))?;
            entry.catalogue.titles.insert("absent".into(), edit);
            Ok(())
        })
        .unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    assert!(cache
        .summary(&image.digest)
        .unwrap()
        .warnings
        .iter()
        .any(|w| w.contains("absent")));
    assert_eq!(cache.query("moonlit", &Facets::default()).len(), 1);
    assert!(store
        .publish_overlay(&image, "unknown:edited", &Provider)
        .is_err());
}

#[test]
fn restoring_local_backup_with_reused_revisions_invalidates_deleted_notes() {
    let temp = Temp::new();
    let store = temp.store();
    let image = publish(&store, &catalogue(), "backup:one");
    let note = |word: &str| Personal {
        user_rating: None,
        notes: Some(word.into()),
    };
    store
        .edit_library(&image.digest, |e| {
            e.personal.insert("alpha".into(), note("Keptword"));
            Ok(())
        })
        .unwrap();
    let path = store.root().join("local/library.toml");
    let backup = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    store
        .edit_library(&image.digest, |e| {
            e.personal.insert("alpha".into(), note("Deletedword"));
            Ok(())
        })
        .unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    assert_eq!(cache.summary(&image.digest).unwrap().personal_revision, 1);
    assert_eq!(cache.query("deletedword", &Facets::default()).len(), 1);
    fs::write(&path, backup).unwrap();
    let mut restored = SummaryCache::open(&store).unwrap();
    assert!(restored.summary(&image.digest).is_none());
    assert!(restored.query("deletedword", &Facets::default()).is_empty());
    restored.rebuild(&store, 1, &[], &[]).unwrap();
    assert_eq!(
        restored.summary(&image.digest).unwrap().personal_revision,
        1
    );
    assert_eq!(restored.query("keptword", &Facets::default()).len(), 1);
    assert!(restored.query("deletedword", &Facets::default()).is_empty());
}

#[test]
fn corrupt_and_newer_library_allow_read_only_listing_without_overwriting_files() {
    let temp = Temp::new();
    let store = temp.store();
    let image = publish(&store, &catalogue(), "readonly:one");
    store.edit_library(&image.digest, edit_description).unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    let cache_path = store.root().join("cache/summaries/index.json");
    let original_cache = fs::read(&cache_path).unwrap();
    let path = store.root().join("local/library.toml");
    for bytes in ["broken = [", "schema = 900\n"] {
        fs::write(&path, bytes).unwrap();
        let mut read_only = SummaryCache::open(&store).unwrap();
        assert!(read_only.is_read_only());
        assert!(read_only.library_error().is_some());
        assert_eq!(read_only.tags().len(), 1);
        read_only.rebuild(&store, 1, &[], &[]).unwrap();
        assert_eq!(read_only.query("moonlit", &Facets::default()).len(), 1);
        assert!(read_only.query("secretword", &Facets::default()).is_empty());
        assert!(store.edit_library(&image.digest, |_| Ok(())).is_err());
        assert!(store
            .publish_overlay(&image, "readonly:edited", &Provider)
            .is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes.as_bytes());
        assert_eq!(fs::read(&cache_path).unwrap(), original_cache);
    }
}

#[test]
fn cache_writers_merge_disjoint_rows_under_the_index_lock() {
    let temp = Temp::new();
    let store = temp.store();
    let first = publish(&store, &catalogue(), "merge:first");
    let mut other = catalogue();
    other.titles[0].title = "Different".into();
    let second = publish(&store, &other, "merge:second");
    let mut a = SummaryCache::open(&store).unwrap();
    let mut b = SummaryCache::open(&store).unwrap();
    a.rebuild(&store, 1, &["merge:first"], &[]).unwrap();
    b.rebuild(&store, 1, &["merge:second"], &[]).unwrap();
    let warm = SummaryCache::open(&store).unwrap();
    assert!(warm.pending().is_empty());
    assert!(warm.summary(&first.digest).is_some());
    assert!(warm.summary(&second.digest).is_some());
}

#[test]
fn local_only_entries_are_not_gc_roots_and_noop_edits_do_not_write() {
    let temp = Temp::new();
    let store = temp.store();
    let image = Digest::of(b"source");
    let path = store.root().join("local/library.toml");
    store.edit_library(&image, |_| Ok(())).unwrap();
    assert!(!path.exists());
    store
        .edit_library(&image, |e| {
            e.favorite = true;
            e.last_run = Some("recorded".into());
            e.personal.insert(
                "alpha".into(),
                Personal {
                    notes: Some("Local".into()),
                    user_rating: None,
                },
            );
            Ok(())
        })
        .unwrap();
    assert!(store.read_library().unwrap().gc_roots().unwrap().is_empty());
    let bytes = fs::read(&path).unwrap();
    store.edit_library(&image, |_| Ok(())).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    store
        .edit_library(&image, |e| {
            e.catalogue.labels = Some(vec!["Travel".into()]);
            Ok(())
        })
        .unwrap();
    assert!(store
        .read_library()
        .unwrap()
        .gc_roots()
        .unwrap()
        .contains(&image));
    store
        .edit_library(&image, |e| {
            *e = Default::default();
            Ok(())
        })
        .unwrap();
    assert!(store.read_library().unwrap().images.is_empty());
}

#[test]
fn reentrant_edit_runs_without_index_lock_and_detects_conflicts() {
    let temp = Temp::new();
    let store = temp.store();
    let image = Digest::of(b"source");
    let result = store.edit_library(&image, |outer| {
        store.edit_library(&image, |inner| {
            inner.labels.push("Inner".into());
            Ok(())
        })?;
        outer.labels.push("Outer".into());
        Ok(())
    });
    assert!(matches!(result, Err(Error::LibraryConflict)));
    assert_eq!(
        store.read_library().unwrap().images[&image].labels,
        ["Inner"]
    );
}

#[test]
fn artwork_is_verified_and_caller_annotations_and_optional_fields_are_not_carried() {
    let temp = Temp::new();
    let store = temp.store();
    let image = publish(&store, &catalogue(), "art:one");
    let mut art = put(&store, mt::PNG, b"Artwork");
    art.annotations
        .insert("unemu.redistribution".into(), "open".into());
    art.annotations.insert(
        "org.opencontainers.image.title".into(),
        "Unreviewed title".into(),
    );
    art.platform = Some(json!({"architecture":"untrusted"}));
    art.artifact_type = Some("untrusted".into());
    for (urls, data) in [
        (Some(vec!["untrusted".into()]), None),
        (None, Some("embedded".into())),
    ] {
        let mut bad = art.clone();
        bad.urls = urls;
        bad.data = data;
        assert!(store
            .edit_library(&image.digest, |e| {
                e.catalogue.artwork = vec![bad];
                Ok(())
            })
            .is_err());
    }
    let mut wrong = art.clone();
    wrong.size += 1;
    assert!(store
        .edit_library(&image.digest, |e| {
            e.catalogue.artwork = vec![wrong];
            Ok(())
        })
        .is_err());
    store
        .edit_library(&image.digest, |e| {
            let mut edit = TitleEdit::default();
            edit.set("cover", json!(art.digest))?;
            e.catalogue.titles.insert("alpha".into(), edit);
            e.catalogue.artwork = vec![art.clone()];
            Ok(())
        })
        .unwrap();
    let clean = Descriptor::new(mt::PNG, art.digest.clone(), art.size);
    assert_eq!(
        store.read_library().unwrap().images[&image.digest]
            .catalogue
            .artwork
            .as_slice(),
        std::slice::from_ref(&clean)
    );
    store
        .publish_overlay(&image, "art:edited", &Provider)
        .unwrap();
    assert_eq!(
        store
            .resolve("art:edited", &Provider)
            .unwrap()
            .manifest
            .layers
            .last(),
        Some(&clean)
    );
}

#[test]
fn search_folds_latin_accents_indexes_cjk_and_omits_artwork_and_ratings() {
    let temp = Temp::new();
    let store = temp.store();
    let mut metadata = catalogue();
    metadata.titles[0].description = Some("Café Łódź 中文探险".into());
    let art = put(&store, mt::PNG, b"fold art");
    metadata.titles[0].cover = Some(art.digest.clone());
    let (image, _) = fixture(&store, &metadata, std::slice::from_ref(&art));
    store
        .update_tag("fold:one", &image, None, &Provider)
        .unwrap();
    store
        .edit_library(&image.digest, |e| {
            e.personal.insert(
                "alpha".into(),
                Personal {
                    notes: Some("Zu\u{308}rich".into()),
                    user_rating: Some(5),
                },
            );
            Ok(())
        })
        .unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 1, &[], &[]).unwrap();
    assert_eq!(cache.query("CAFE lodz", &Facets::default()).len(), 1);
    assert_eq!(cache.query("zurich", &Facets::default()).len(), 1);
    assert_eq!(cache.query("文探", &Facets::default()).len(), 1);
    assert!(cache.query(art.digest.hex(), &Facets::default()).is_empty());
    assert!(cache.query("sha256", &Facets::default()).is_empty());
    assert!(cache.query("5", &Facets::default()).is_empty());
}

#[test]
fn future_projection_with_many_unknown_keys_and_large_description_is_bounded() {
    let mut record = json!({"id":"large", "title":"Large", "description":"x".repeat(300_000)});
    for i in 0..40_000 {
        record[format!("zz{i:05}")] = json!(0);
    }
    let bytes =
        serde_json::to_vec(&json!({"schema":42,"platform":"c64","titles":[record]})).unwrap();
    assert!(bytes.len() < MAX_SMALL_BLOB as usize);
    let started = std::time::Instant::now();
    let metadata = Metadata::decode(&bytes, &[]).unwrap();
    assert_eq!(
        metadata.titles[0].description.as_ref().unwrap().len(),
        300_000
    );
    assert!(metadata.is_read_only());
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}

#[test]
fn overlay_publication_validates_the_source_closure_only_once() {
    struct Counting {
        calls: std::cell::Cell<usize>,
        base: Digest,
    }
    impl ConfigValidator for Counting {
        fn platform(&self) -> &str {
            "c64"
        }
        fn validate(
            &self,
            _: &toml::Table,
            _: &[Descriptor],
            _: &dyn BlobSource,
        ) -> Result<Vec<Digest>> {
            self.calls.set(self.calls.get() + 1);
            Ok(vec![self.base.clone()])
        }
    }
    let temp = Temp::new();
    let store = temp.store();
    let (_, mut manifest) = fixture(&store, &catalogue(), &[]);
    let base = put(&store, mt::DISK_BASE, &vec![0x5a; 1024 * 1024]);
    manifest.layers.insert(0, base.clone());
    manifest.annotations.insert(
        "org.opencontainers.image.base.digest".into(),
        Digest::of(b"earlier").to_string(),
    );
    let image = store.import_manifest(&manifest).unwrap();
    let provider = Counting {
        calls: std::cell::Cell::new(0),
        base: base.digest,
    };
    store
        .update_tag("verify:one", &image, None, &provider)
        .unwrap();
    store.edit_library(&image.digest, edit_description).unwrap();
    provider.calls.set(0);
    store
        .publish_overlay(&image, "verify:edited", &provider)
        .unwrap();
    assert_eq!(provider.calls.get(), 1);
    assert_eq!(
        store
            .resolve("verify:edited", &provider)
            .unwrap()
            .manifest
            .annotations["org.opencontainers.image.base.digest"],
        image.digest.to_string()
    );
}

#[test]
fn overlay_provenance_names_the_source_image_instead_of_its_ancestor() {
    let temp = Temp::new();
    let store = temp.store();
    let (_, mut manifest) = fixture(&store, &catalogue(), &[]);
    manifest.annotations.insert(
        "org.opencontainers.image.base.digest".into(),
        Digest::of(b"ancestor").to_string(),
    );
    let source = store.import_manifest(&manifest).unwrap();
    store
        .update_tag("derived:one", &source, None, &Provider)
        .unwrap();
    store
        .edit_library(&source.digest, edit_description)
        .unwrap();
    store
        .publish_overlay(&source, "derived:edited", &Provider)
        .unwrap();
    let derived = store.resolve("derived:edited", &Provider).unwrap();
    assert_eq!(
        derived.manifest.annotations["org.opencontainers.image.base.digest"],
        source.digest.to_string()
    );
    assert_eq!(derived.manifest.config, manifest.config);
}

#[test]
fn unassigned_artwork_stays_rooted_without_creating_an_image_revision() {
    let temp = Temp::new();
    let store = temp.store();
    let image = publish(&store, &catalogue(), "pending:one");
    let art = put(&store, mt::PNG, b"Unassigned art");
    store
        .edit_library(&image.digest, |e| {
            e.catalogue.artwork.push(art.clone());
            Ok(())
        })
        .unwrap();
    let alias = store
        .publish_overlay(&image, "pending:alias", &Provider)
        .unwrap();
    assert_eq!(alias.digest, image.digest);
    assert!(store
        .read_library()
        .unwrap()
        .gc_roots()
        .unwrap()
        .contains(&art.digest));
}
