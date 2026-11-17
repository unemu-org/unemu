// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic disks and open firmware exercise whole-machine sessions and crashes.
//! See machine-images §§3–5; no controller needs to know a disk's layer depth.

use image_store::{media_type as mt, Descriptor};
use pc_image::{
    workspace::{self, PublicationStep},
    PcProvider,
};
use std::fs;
#[path = "support/workspace.rs"]
mod fixture;
use fixture::Fixture;
#[test]
fn whole_image_publication_layers_fixed_disks_freezes_catalogue_and_cmos() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture.changed("play", 42);
    let owner = fixture.store.workspace("play").unwrap();
    let published = workspace::publish(&owner, "test:saved", None).unwrap();
    assert_ne!(published.digest, fixture.image.digest);
    fixture.assert_complete("test:saved", 42);
    fixture.assert_complete("test:source", 0);
    let image = fixture.store.resolve("test:saved", &PcProvider).unwrap();
    assert_eq!(
        image
            .manifest
            .layers
            .iter()
            .filter(|d| d.media_type == mt::DISK_LAYER)
            .count(),
        1
    );
    assert_eq!(
        image
            .manifest
            .layers
            .iter()
            .filter(|d| d.decoded_media_type() == mt::DISK_SECTORS)
            .count(),
        1
    );
    assert_eq!(
        image
            .manifest
            .layers
            .iter()
            .filter(|d| d.decoded_media_type() == mt::FLOPPY)
            .count(),
        1
    );
    assert!(!owner.path().join("media/medium-0.upfj").exists());
    assert!(owner.read("transaction.toml").unwrap().is_none());
    let blobs = || {
        let mut names = fs::read_dir(fixture.store.root().join("images/blobs/sha256"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>();
        names.sort();
        names
    };
    let before = blobs();
    fixture
        .store
        .remove_tag("test:source", &fixture.image.digest)
        .unwrap();
    assert_eq!(blobs(), before);
    fixture.assert_complete("test:saved", 42);
    fixture
        .store
        .resolve_descriptor(&fixture.image, &PcProvider)
        .unwrap();
    assert!(workspace::start(&owner, "test:saved").is_ok());
}
#[test]
fn every_publication_step_can_fail_and_retry_without_mixed_heads() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture.changed("probe", 61);
    let owner = fixture.store.workspace("probe").unwrap();
    let mut steps = Vec::new();
    workspace::publish_with_hook(&owner, "test:probe", None, |step| {
        steps.push(step);
        Ok(())
    })
    .unwrap();
    eprintln!("publication interruption boundaries: {}", steps.len());
    drop(owner);
    drop(fixture);
    assert!(steps.contains(&PublicationStep::IntentStored));
    for index in 0..steps.len() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        fixture.changed("play", 61);
        fixture
            .store
            .update_tag("test:target", &fixture.image, None, &PcProvider)
            .unwrap();
        let owner = fixture.store.workspace("play").unwrap();
        let mut current = 0;
        let result = workspace::publish_with_hook(
            &owner,
            "test:target",
            Some(&fixture.image.digest),
            |_| {
                let fail = current == index;
                current += 1;
                if fail {
                    Err(pc_image::Error::Invalid("injected interruption".into()))
                } else {
                    Ok(())
                }
            },
        );
        assert!(result.is_err(), "step {index}");
        let session: toml::Table =
            std::str::from_utf8(&owner.read("session.toml").unwrap().unwrap())
                .unwrap()
                .parse()
                .unwrap();
        if session["image"]["digest"].as_str().unwrap() == fixture.image.digest.to_string() {
            assert!(
                owner.path().join("media/cmos.upfj").exists()
                    || owner.path().join("retired/cmos.upfj").exists()
            );
        }
        let target = fixture.store.resolve("test:target", &PcProvider).unwrap();
        if target.image.digest == fixture.image.digest {
            fixture.assert_complete("test:target", 0);
        } else {
            fixture.assert_complete("test:target", 61);
        }
        drop(owner);
        let owner = fixture.store.workspace("play").unwrap();
        if owner.read("transaction.toml").unwrap().is_some() {
            assert!(workspace::start(&owner, "test:source").is_err());
            workspace::publish(&owner, "test:target", Some(&fixture.image.digest)).unwrap();
        } else if target.image.digest == fixture.image.digest {
            workspace::publish(&owner, "test:target", Some(&fixture.image.digest)).unwrap();
        }
        fixture.assert_complete("test:target", 61);
        fixture.assert_complete("test:source", 0);
    }
}
#[test]
fn workspace_lock_discard_and_tag_deletion_preserve_other_roots() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture.changed("one", 11);
    fixture.changed("two", 22);
    for name in ["", "../escape", "/absolute", ".", "..", "bad/name", "UPPER"] {
        assert!(fixture.store.workspace(name).is_err());
    }
    let one = fixture.store.workspace("one").unwrap();
    assert!(fixture.store.workspace("one").is_err());
    assert_eq!(fixture.store.list_workspaces().unwrap(), ["one", "two"]);
    one.discard().unwrap();
    assert_eq!(fixture.store.list_workspaces().unwrap(), ["two"]);
    let two = fixture.store.workspace("two").unwrap();
    fixture
        .store
        .remove_tag("test:source", &fixture.image.digest)
        .unwrap();
    workspace::start(&two, "test:source").unwrap();
    workspace::publish(&two, "test:kept", None).unwrap();
    fixture.assert_complete("test:kept", 22);
    fixture
        .store
        .resolve_descriptor(&fixture.image, &PcProvider)
        .unwrap();
}
#[test]
fn live_journal_writer_and_tag_race_block_publication() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture.changed("play", 35);
    let owner = fixture.store.workspace("play").unwrap();
    let config = workspace::start(&owner, "test:source").unwrap();
    let assembly::ImageSource::File(path) =
        &config.drives[0].media.as_ref().unwrap().entries[0].image
    else {
        panic!()
    };
    let bytes = fs::read(path).unwrap();
    let live = media::JournalMedia::open(
        &owner.path().join("media/medium-0.upfj"),
        &bytes,
        bytes.len() as u64,
        Default::default(),
    )
    .unwrap();
    assert!(workspace::publish(&owner, "test:blocked", None).is_err());
    drop(live);
    fixture
        .store
        .update_tag("test:occupied", &fixture.image, None, &PcProvider)
        .unwrap();
    assert!(workspace::publish(&owner, "test:occupied", None).is_err());
    fixture.assert_complete("test:occupied", 0);
    assert!(owner.path().join("media/medium-0.upfj").exists());
    fixture.changed("retry", 36);
    let retry = fixture.store.workspace("retry").unwrap();
    assert!(
        workspace::publish_with_hook(&retry, "test:retry", None, |step| {
            if step == PublicationStep::IntentStored {
                Err(pc_image::Error::Invalid("injected interruption".into()))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    let live = media::JournalMedia::open(
        &retry.path().join("media/medium-0.upfj"),
        &bytes,
        bytes.len() as u64,
        Default::default(),
    )
    .unwrap();
    assert!(workspace::publish(&retry, "test:retry", None).is_err());
    assert!(matches!(
        fixture.store.resolve("test:retry", &PcProvider),
        Err(image_store::Error::TagNotFound(_))
    ));
    drop(live);
    workspace::publish(&retry, "test:retry", None).unwrap();
    fixture.assert_complete("test:retry", 36);
    fixture.changed("same", 37);
    let same = fixture.store.workspace("same").unwrap();
    let result = workspace::publish_with_hook(&same, "test:same", None, |step| {
        if step == PublicationStep::IntentStored {
            let transaction: toml::Table =
                std::str::from_utf8(&same.read("transaction.toml")?.unwrap())
                    .unwrap()
                    .parse()
                    .unwrap();
            let proposed: Descriptor = transaction["proposed"].clone().try_into().unwrap();
            fixture
                .store
                .update_tag("test:same", &proposed, None, &PcProvider)?;
        }
        Ok(())
    });
    assert!(matches!(
        result,
        Err(pc_image::Error::Store(image_store::Error::Conflict { .. }))
    ));
    assert!(same.read("transaction.toml").unwrap().is_none());
    workspace::start(&same, "test:source").unwrap();
    workspace::publish(&same, "test:alternate", None).unwrap();
    fixture.assert_complete("test:alternate", 37);
}

#[test]
fn serial_layers_share_a_verified_materialization_and_keep_retention() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture.changed("play", 41);
    let owner = fixture.store.workspace("play").unwrap();
    workspace::publish(&owner, "test:first", None).unwrap();
    let first = workspace::start(&owner, "test:first").unwrap();
    let other = fixture.store.workspace("other").unwrap();
    let second = workspace::start(&other, "test:first").unwrap();
    let first_fixed = first
        .drives
        .iter()
        .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
        .unwrap();
    let other_fixed = second
        .drives
        .iter()
        .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
        .unwrap();
    assert_eq!(first_fixed.image, other_fixed.image);
    assert!(matches!(
        first_fixed.write,
        assembly::WriteMode::Journal {
            keep_generations: 3,
            ..
        }
    ));
    let assembly::ImageSource::File(path) = &first_fixed.image else {
        panic!()
    };
    assert!(path.starts_with(fixture.store.root().join("cache/materialized")));
    let assembly::WriteMode::Journal {
        path: journal_path, ..
    } = &first_fixed.write
    else {
        panic!()
    };
    let base = media::VerifiedBase::open(path).unwrap();
    let mut journal =
        media::JournalMedia::open_verified(journal_path, &base, 32 * 512, Default::default())
            .unwrap();
    journal.write_batch(&[(1, [99; 512])]);
    journal.finish().unwrap();
    workspace::publish(&owner, "test:second", None).unwrap();
    let image = fixture.store.resolve("test:second", &PcProvider).unwrap();
    assert_eq!(
        image
            .manifest
            .layers
            .iter()
            .filter(|d| d.media_type == mt::DISK_LAYER)
            .count(),
        2
    );
    let config = workspace::start(&owner, "test:second").unwrap();
    let disk = config
        .drives
        .iter()
        .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
        .unwrap();
    assert!(matches!(
        disk.write,
        assembly::WriteMode::Journal {
            keep_generations: 3,
            ..
        }
    ));
    let assembly::ImageSource::File(path) = &disk.image else {
        panic!()
    };
    let base = media::VerifiedBase::open(path).unwrap();
    assert_eq!(
        media::SectorSource::read_sector(&base, 0).unwrap(),
        [41; 512]
    );
    assert_eq!(
        media::SectorSource::read_sector(&base, 1).unwrap(),
        [99; 512]
    );
    // Cache names never substitute for verifying the complete bytes at Start.
    fs::write(path, vec![3; 32 * 512]).unwrap();
    assert!(workspace::start(&owner, "test:second").is_err());
}

#[test]
fn newer_sessions_and_active_writers_cannot_be_discarded() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture.changed("play", 12);
    let owner = fixture.store.workspace("play").unwrap();
    let config = workspace::start(&owner, "test:source").unwrap();
    let first = &config.drives[0].media.as_ref().unwrap().entries[0];
    let assembly::ImageSource::File(path) = &first.image else {
        panic!()
    };
    let bytes = fs::read(path).unwrap();
    let live = media::JournalMedia::open(
        &owner.path().join("media/medium-0.upfj"),
        &bytes,
        bytes.len() as u64,
        Default::default(),
    )
    .unwrap();
    assert!(owner.discard().is_err());
    drop(live);
    let owner = fixture.store.workspace("play").unwrap();
    let before = owner.read("session.toml").unwrap().unwrap();
    let text = String::from_utf8(before)
        .unwrap()
        .replacen("schema = 1", "schema = 2", 1);
    owner.write("session.toml", text.as_bytes()).unwrap();
    assert!(workspace::start(&owner, "test:source").is_err());
    assert!(workspace::publish(&owner, "test:new", None).is_err());
    assert!(owner.discard().is_err());
    let owner = fixture.store.workspace("play").unwrap();
    assert_eq!(
        owner.read("session.toml").unwrap().unwrap(),
        text.as_bytes()
    );
}

#[test]
fn insufficient_space_is_reported_before_staging_media() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    assert!(fixture.store.require_space(u64::MAX).is_err());
    fixture.store.require_space(0).unwrap();
    fixture.assert_complete("test:source", 0);
}

#[test]
fn conflicting_publication_rolls_back_intent_and_can_publish_elsewhere() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture.changed("play", 43);
    let owner = fixture.store.open_workspace("play").unwrap();
    let before = fs::read(owner.path().join("media/medium-0.upfj")).unwrap();
    let result = workspace::publish_with_hook(&owner, "test:race", None, |step| {
        if step == PublicationStep::IntentStored {
            fixture
                .store
                .update_tag("test:race", &fixture.image, None, &PcProvider)?;
        }
        Ok(())
    });
    assert!(matches!(
        result,
        Err(pc_image::Error::Store(image_store::Error::Conflict { .. }))
    ));
    assert!(owner.read("transaction.toml").unwrap().is_none());
    assert_eq!(
        fs::read(owner.path().join("media/medium-0.upfj")).unwrap(),
        before
    );
    workspace::start(&owner, "test:source").unwrap();
    workspace::publish(&owner, "test:other", None).unwrap();
    fixture.assert_complete("test:other", 43);
}
fn image_with_metadata(
    fixture: &Fixture,
    bytes: &[u8],
    artwork: Option<Descriptor>,
    tag: &str,
) -> Descriptor {
    let original = fixture.store.resolve("test:source", &PcProvider).unwrap();
    let metadata = fixture.store.import(bytes, None).unwrap();
    let mut manifest = original.manifest;
    manifest
        .layers
        .retain(|d| !matches!(d.media_type.as_str(), mt::METADATA | mt::PNG));
    manifest.layers.push(Descriptor::new(
        mt::METADATA,
        metadata.digest,
        metadata.size,
    ));
    if let Some(artwork) = artwork {
        manifest.layers.push(artwork);
    }
    let descriptor = fixture
        .store
        .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
        .unwrap();
    fixture
        .store
        .update_tag(tag, &descriptor, Some(&fixture.image.digest), &PcProvider)
        .unwrap();
    descriptor
}
#[test]
fn missing_artwork_and_read_only_metadata_fail_before_intent() {
    for read_only in [false, true] {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let png = fixture
            .store
            .import(b"synthetic artwork".as_slice(), None)
            .unwrap();
        let artwork = Descriptor::new(mt::PNG, png.digest.clone(), png.size);
        let metadata = if read_only {
            br#"{"schema":3,"platform":"ibm-pc","labels":[],"titles":[]}"#.to_vec()
        } else {
            format!("{{\"schema\":1,\"platform\":\"ibm-pc\",\"labels\":[],\"titles\":[{{\"id\":\"test\",\"title\":\"synthetic\",\"cover\":\"{}\"}}]}}", png.digest).into_bytes()
        };
        let descriptor = image_with_metadata(
            &fixture,
            &metadata,
            (!read_only).then_some(artwork),
            "test:source",
        );
        let missing = fixture
            .store
            .root()
            .join("images/blobs/sha256")
            .join(png.digest.hex());
        if !read_only {
            fs::remove_file(&missing).unwrap();
        }
        fixture.changed("play", 44);
        let owner = fixture.store.open_workspace("play").unwrap();
        assert!(workspace::publish(&owner, "test:bad", None).is_err());
        assert!(owner.read("transaction.toml").unwrap().is_none());
        workspace::start(&owner, "test:source").unwrap();
        if !read_only {
            fs::write(missing, b"synthetic artwork").unwrap();
        } else {
            // Repair the source metadata while preserving the workspace's media.
            let mut state: toml::Table =
                std::str::from_utf8(&owner.read("session.toml").unwrap().unwrap())
                    .unwrap()
                    .parse()
                    .unwrap();
            let original = fixture
                .store
                .resolve_descriptor(&fixture.image, &PcProvider)
                .unwrap();
            state["image"] = toml::Value::try_from(&fixture.image).unwrap();
            state["machine"] = original.manifest.config.digest.to_string().into();
            owner
                .write("session.toml", toml::to_string(&state).unwrap().as_bytes())
                .unwrap();
            let _ = descriptor;
        }
        workspace::publish(&owner, "test:repaired", None).unwrap();
        fixture.assert_complete("test:repaired", 44);
    }
}
#[test]
fn abandon_preserves_journals_and_start_refuses_orphans_and_wrong_tags() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture.changed("play", 45);
    let owner = fixture.store.open_workspace("play").unwrap();
    let before = fs::read(owner.path().join("media/medium-0.upfj")).unwrap();
    assert!(
        workspace::publish_with_hook(&owner, "test:abandoned", None, |step| {
            if step == PublicationStep::IntentStored {
                Err(pc_image::Error::Invalid("injected interruption".into()))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    workspace::abandon_publication(&owner).unwrap();
    assert_eq!(
        fs::read(owner.path().join("media/medium-0.upfj")).unwrap(),
        before
    );
    workspace::start(&owner, "test:source").unwrap();
    assert!(workspace::start_new(&owner, "test:source").is_err());
    workspace::publish(&owner, "test:kept", None).unwrap();
    assert!(workspace::start(&owner, "test:source")
        .unwrap_err()
        .to_string()
        .contains("workspace pins"));
    let orphan = fixture.store.workspace("orphan").unwrap();
    fs::create_dir(orphan.path().join("media")).unwrap();
    fs::write(orphan.path().join("media/written"), b"preserve").unwrap();
    assert!(workspace::start(&orphan, "test:source").is_err());
    assert!(orphan.read("session.toml").unwrap().is_none());
}
#[test]
fn failed_pack_is_removed_and_flatten_produces_a_new_raw_head() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture.changed("play", 46);
    let owner = fixture.store.open_workspace("play").unwrap();
    assert!(
        workspace::publish_with_hook(&owner, "test:failed", None, |step| {
            if matches!(step, PublicationStep::PackStaged(_)) {
                Err(pc_image::Error::Invalid("injected interruption".into()))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    assert!(!owner.path().join("publication").exists());
    workspace::publish(&owner, "test:layer", None).unwrap();
    workspace::publish_with_options(&owner, "test:flat", None, true, |_| Ok(())).unwrap();
    let flat = fixture.store.resolve("test:flat", &PcProvider).unwrap();
    assert!(!flat
        .manifest
        .layers
        .iter()
        .any(|d| d.media_type == mt::DISK_LAYER));
    fixture.assert_complete("test:flat", 46);
    fixture.assert_complete("test:layer", 46);
    let next = fixture.store.workspace("new-session").unwrap();
    let config = workspace::start(&next, "test:flat").unwrap();
    let fixed = config
        .drives
        .iter()
        .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
        .unwrap();
    assert!(matches!(
        fixed.write,
        assembly::WriteMode::Journal {
            keep_generations: 3,
            ..
        }
    ));
}
#[test]
fn raw_and_depth_three_starts_verify_each_full_disk_once() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let raw = fixture.store.resolve("test:source", &PcProvider).unwrap();
    let root = raw
        .manifest
        .layers
        .iter()
        .find(|d| d.media_type == mt::DISK_BASE)
        .unwrap()
        .digest
        .clone();
    let before = image_store::verification_passes(&root);
    let media_before = media::data_passes(32 * 512);
    let owner = fixture.store.workspace("raw-count").unwrap();
    let prepared = workspace::prepare_start(&owner, "test:source", false).unwrap();
    let mut machine = assembly::assemble_with_verified_bases(
        &prepared.machine,
        &Default::default(),
        Default::default(),
        &prepared.bases,
    )
    .unwrap();
    machine.finish_persistence().unwrap();
    drop(machine);
    assert_eq!(
        image_store::verification_passes(&root) - before + media::data_passes(32 * 512)
            - media_before,
        1
    );
    drop(owner);
    for (index, parent) in ["test:source", "test:depth1", "test:depth2"]
        .into_iter()
        .enumerate()
    {
        let name = format!("depth-{index}");
        fixture.changed_from(&name, parent, 60 + index as u8);
        let owner = fixture.store.open_workspace(&name).unwrap();
        let before = image_store::verification_passes(&root);
        workspace::publish(&owner, &format!("test:depth{}", index + 1), None).unwrap();
        assert_eq!(image_store::verification_passes(&root) - before, 1);
    }
    for cold in [false, true] {
        if cold {
            let cache = fixture.store.root().join("cache/materialized");
            for entry in fs::read_dir(cache).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_none() {
                    fs::remove_file(path).unwrap();
                }
            }
        }
        let before = image_store::verification_passes(&root);
        let media_before = media::data_passes(32 * 512);
        let owner = fixture
            .store
            .workspace(if cold { "cold-count" } else { "warm-count" })
            .unwrap();
        let prepared = workspace::prepare_start(&owner, "test:depth3", false).unwrap();
        let mut machine = assembly::assemble_with_verified_bases(
            &prepared.machine,
            &Default::default(),
            Default::default(),
            &prepared.bases,
        )
        .unwrap();
        machine.finish_persistence().unwrap();
        assert_eq!(image_store::verification_passes(&root) - before, 1);
        assert_eq!(media::data_passes(32 * 512) - media_before, 1);
    }
}

#[test]
fn depth_limit_accepts_sixty_four_and_flatten_allows_further_publication() {
    use image_store::ConfigValidator;
    let Some(fixture) = Fixture::new() else {
        return;
    };
    let original = fixture.store.resolve("test:source", &PcProvider).unwrap();
    let root = original
        .manifest
        .layers
        .iter()
        .find(|d| d.media_type == mt::DISK_BASE)
        .unwrap()
        .digest
        .clone();
    let mut descriptors = original.manifest.layers.clone();
    let mut parent = root.clone();
    let geometry = media::layer::Geometry {
        cylinders: 1,
        heads: 1,
        sectors: 32,
        sector_bytes: 512,
    };
    let mut bytes = vec![0; 32 * 512];
    for depth in 1..=65 {
        let before = media::VerifiedBase::from_bytes(&bytes).unwrap();
        bytes[..512].fill(depth);
        let after = media::VerifiedBase::from_bytes(&bytes).unwrap();
        let mut pack = Vec::new();
        let layer = media::layer::seal(
            &before,
            &after,
            geometry.clone(),
            &parent.to_string(),
            &root.to_string(),
            &mut pack,
        )
        .unwrap();
        let pack_blob = fixture.store.import(pack.as_slice(), None).unwrap();
        descriptors.push(Descriptor::new(
            mt::DISK_SECTORS,
            pack_blob.digest,
            pack_blob.size,
        ));
        let blob = fixture
            .store
            .import(layer.encode().unwrap().as_slice(), None)
            .unwrap();
        parent = blob.digest.clone();
        descriptors.push(Descriptor::new(mt::DISK_LAYER, blob.digest, blob.size));
        if depth < 64 {
            continue;
        }
        let mut config: toml::Table = std::str::from_utf8(&original.config)
            .unwrap()
            .parse()
            .unwrap();
        for m in config["media"].as_array_mut().unwrap() {
            if m["head"].as_str().unwrap() == root.to_string() {
                m["head"] = parent.to_string().into();
            }
        }
        let refs = PcProvider.validate(&config, &descriptors, &fixture.store);
        if depth == 65 {
            let error = refs.unwrap_err().to_string();
            assert!(error.contains("64"));
            assert!(!error.contains("cycle"));
            break;
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut layers = refs
            .unwrap()
            .into_iter()
            .filter(|digest| seen.insert(digest.clone()))
            .map(|digest| {
                descriptors
                    .iter()
                    .find(|d| d.names(&digest))
                    .unwrap()
                    .clone()
            })
            .collect::<Vec<_>>();
        layers.extend(
            original
                .manifest
                .layers
                .iter()
                .filter(|d| d.media_type == mt::METADATA)
                .cloned(),
        );
        let config_blob = fixture
            .store
            .import(toml::to_string(&config).unwrap().as_bytes(), None)
            .unwrap();
        let manifest = image_store::Manifest::new_with_types(
            "ibm-pc",
            Descriptor::new(mt::CONFIG, config_blob.digest, config_blob.size),
            layers,
            pc_image::PAYLOAD_TYPES,
        )
        .unwrap();
        let image = fixture
            .store
            .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
            .unwrap();
        fixture
            .store
            .update_tag("test:limit", &image, None, &PcProvider)
            .unwrap();
        let owner = fixture.store.workspace("limit").unwrap();
        workspace::start(&owner, "test:limit").unwrap();
        workspace::publish_with_options(&owner, "test:compacted", None, true, |_| Ok(())).unwrap();
        let flat = fixture
            .store
            .resolve("test:compacted", &PcProvider)
            .unwrap();
        assert!(!flat
            .manifest
            .layers
            .iter()
            .any(|d| d.media_type == mt::DISK_LAYER));
    }
}

#[test]
fn shared_result_cache_does_not_validate_an_unseen_invalid_head() {
    use image_store::{BlobSource, ConfigValidator};
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture.changed("good", 91);
    workspace::publish(
        &fixture.store.open_workspace("good").unwrap(),
        "test:good",
        None,
    )
    .unwrap();
    let image = fixture.store.resolve("test:good", &PcProvider).unwrap();
    let d = image
        .manifest
        .layers
        .iter()
        .find(|d| d.media_type == mt::DISK_LAYER)
        .unwrap();
    let mut layer = media::layer::Descriptor::decode(
        &fixture
            .store
            .read_verified(d, image_store::MAX_SMALL_BLOB)
            .unwrap(),
    )
    .unwrap();
    let mut pack = 0_u64.to_le_bytes().to_vec();
    pack.extend([92; 512]);
    let packed = fixture.store.import(pack.as_slice(), None).unwrap();
    layer.sectors = packed.digest.to_string();
    let bad = fixture
        .store
        .import(layer.encode().unwrap().as_slice(), None)
        .unwrap();
    let mut descriptors = image.manifest.layers.clone();
    descriptors.push(Descriptor::new(
        mt::DISK_SECTORS,
        packed.digest,
        packed.size,
    ));
    descriptors.push(Descriptor::new(
        mt::DISK_LAYER,
        bad.digest.clone(),
        bad.size,
    ));
    let mut config: toml::Table = std::str::from_utf8(&image.config).unwrap().parse().unwrap();
    for m in config["media"].as_array_mut().unwrap() {
        if m["head"].as_str().unwrap() == d.digest.to_string() {
            m["head"] = bad.digest.to_string().into();
        }
    }
    let refs = PcProvider
        .validate(&config, &descriptors, &fixture.store)
        .unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let mut layers = refs
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .map(|id| descriptors.iter().find(|d| d.names(&id)).unwrap().clone())
        .collect::<Vec<_>>();
    layers.extend(
        image
            .manifest
            .layers
            .iter()
            .filter(|d| d.media_type == mt::METADATA)
            .cloned(),
    );
    let blob = fixture
        .store
        .import(toml::to_string(&config).unwrap().as_bytes(), None)
        .unwrap();
    let manifest = image_store::Manifest::new_with_types(
        "ibm-pc",
        Descriptor::new(mt::CONFIG, blob.digest, blob.size),
        layers,
        pc_image::PAYLOAD_TYPES,
    )
    .unwrap();
    let descriptor = fixture
        .store
        .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
        .unwrap();
    assert!(fixture
        .store
        .update_tag("test:invalid", &descriptor, None, &PcProvider)
        .is_err());
    fixture.assert_complete("test:good", 91);
}

#[test]
fn compressed_bases_and_sector_packs_publish_and_resume_through_disk_chains() {
    use image_store::BlobSource;
    let Some(fixture) = Fixture::new() else {
        return;
    };
    fixture
        .store
        .compress_image("test:source", "test:compressed", &PcProvider)
        .unwrap();
    fixture.changed_from("compressed", "test:compressed", 0x91);
    let owner = fixture.store.workspace("compressed").unwrap();
    let machine = workspace::start(&owner, "test:compressed").unwrap();
    let fixed = machine
        .drives
        .iter()
        .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
        .unwrap();
    let assembly::ImageSource::File(base) = &fixed.image else {
        panic!()
    };
    let assembly::WriteMode::Journal { path, .. } = &fixed.write else {
        panic!()
    };
    let raw = fs::read(base).unwrap();
    let mut journal =
        media::JournalMedia::open(path, &raw, raw.len() as u64, Default::default()).unwrap();
    let sectors: Vec<_> = (0..9).map(|lba| (lba, [0x91; 512])).collect();
    journal.write_batch(&sectors);
    journal.finish().unwrap();
    let published = workspace::publish(&owner, "test:layered-compressed", None).unwrap();
    let image = fixture
        .store
        .resolve_descriptor(&published, &PcProvider)
        .unwrap();
    let pack = image
        .manifest
        .layers
        .iter()
        .find(|d| d.decoded_media_type() == mt::DISK_SECTORS)
        .unwrap();
    assert!(pack.is_compressed());
    assert_eq!(
        fixture.store.read_verified(pack, 8192).unwrap().len(),
        9 * 520
    );
    let resumed = workspace::start(&owner, "test:layered-compressed").unwrap();
    let fixed = resumed
        .drives
        .iter()
        .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
        .unwrap();
    let assembly::ImageSource::File(path) = &fixed.image else {
        panic!()
    };
    assert_eq!(&fs::read(path).unwrap()[..9 * 512], &[0x91; 9 * 512]);
}

/// Decoded payload and flattened-head files in the cache, without lock files.
fn decoded_files(store: &image_store::Store) -> Vec<String> {
    let mut names: Vec<_> = match fs::read_dir(store.root().join("cache/materialized")) {
        Ok(entries) => entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !name.ends_with(".lock"))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}
fn fixed_disk(machine: &assembly::MachineConfig) -> &assembly::DriveConfig {
    machine
        .drives
        .iter()
        .find(|d| matches!(d.kind, assembly::DriveKind::HardDisk { .. }))
        .unwrap()
}
/// Every regular file under `root` with its length and modification time.
fn tree(root: &std::path::Path) -> Vec<(std::path::PathBuf, u64, std::time::SystemTime)> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                files.push((entry.path(), metadata.len(), metadata.modified().unwrap()));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn layered_heads_stream_compressed_bases_and_packs_and_flatten_compressed_bases() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    pc_image::compress_image(&fixture.store, "test:source", "test:compressed").unwrap();
    let source = fixture
        .store
        .resolve("test:compressed", &PcProvider)
        .unwrap();
    let base = source
        .manifest
        .layers
        .iter()
        .find(|d| d.decoded_media_type() == mt::DISK_BASE)
        .unwrap()
        .clone();
    assert!(base.is_compressed());
    let base_hex = base.decoded_digest().unwrap().hex().to_owned();
    let owner = fixture.store.workspace("layer").unwrap();
    let prepared = workspace::prepare_start(&owner, "test:compressed", true).unwrap();
    // The session mounted the hard disk only.
    assert_eq!(
        decoded_files(&fixture.store),
        std::slice::from_ref(&base_hex)
    );
    let fixed = fixed_disk(&prepared.machine);
    let (assembly::ImageSource::File(path), assembly::WriteMode::Journal { path: journal, .. }) =
        (&fixed.image, &fixed.write)
    else {
        panic!("journaled file-backed disk expected")
    };
    let raw = fs::read(path).unwrap();
    let mut media =
        media::JournalMedia::open(journal, &raw, raw.len() as u64, Default::default()).unwrap();
    // Enough sectors that the pack is large enough to compress.
    let sectors: Vec<_> = (0..9).map(|lba| (lba, [0x93; 512])).collect();
    media.write_batch(&sectors);
    media.finish().unwrap();
    drop(prepared);
    workspace::publish(&owner, "test:layered", None).unwrap();
    drop(owner);
    let layered = fixture.store.resolve("test:layered", &PcProvider).unwrap();
    let pack = layered
        .manifest
        .layers
        .iter()
        .find(|d| d.decoded_media_type() == mt::DISK_SECTORS)
        .unwrap();
    assert!(pack.is_compressed());
    // Publication marks every compressed payload for the push guard.
    assert!(layered
        .manifest
        .layers
        .iter()
        .all(|d| d.annotations.contains_key("unemu.redistribution")));
    let pack_hex = pack.decoded_digest().unwrap().hex().to_owned();
    // Without a decoded base, the head must stream the compressed base.
    fs::remove_file(
        fixture
            .store
            .root()
            .join("cache/materialized")
            .join(&base_hex),
    )
    .unwrap();
    let before = tree(fixture.store.root());
    fixture.store.resolve("test:layered", &PcProvider).unwrap();
    pc_image::inspection::inspect(
        &fixture.store,
        &fixture.store.tag_descriptor("test:layered").unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(tree(fixture.store.root()), before);
    let next = fixture.store.workspace("resume").unwrap();
    let prepared = workspace::prepare_start(&next, "test:layered", true).unwrap();
    let names = decoded_files(&fixture.store);
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.iter().any(|n| n.ends_with(".verified")));
    assert!(!names.contains(&base_hex) && !names.contains(&pack_hex));
    let assembly::ImageSource::File(head) = &fixed_disk(&prepared.machine).image else {
        panic!("file-backed head expected")
    };
    let head = fs::read(head).unwrap();
    assert_eq!(&head[..9 * 512], &[0x93; 9 * 512]);
    assert_eq!(&head[9 * 512..], &raw[9 * 512..]);
    drop(prepared);
    drop(next);
    // Flattening a compressed base produces a new base, not a layer.
    fixture.changed_from("flat", "test:compressed", 0x94);
    let owner = fixture.store.open_workspace("flat").unwrap();
    workspace::publish_with_options(&owner, "test:flat", None, true, |_| Ok(())).unwrap();
    drop(owner);
    let flat = fixture.store.resolve("test:flat", &PcProvider).unwrap();
    assert!(!flat
        .manifest
        .layers
        .iter()
        .any(|d| d.media_type == mt::DISK_LAYER));
    fixture.assert_complete("test:flat", 0x94);
}

#[test]
fn sweep_keeps_decoded_bases_of_tags_and_workspace_pins_only() {
    let Some(fixture) = Fixture::new() else {
        return;
    };
    pc_image::compress_image(&fixture.store, "test:source", "test:compressed").unwrap();
    let descriptor = fixture.store.tag_descriptor("test:compressed").unwrap();
    let owner = fixture.store.workspace("pinned").unwrap();
    workspace::prepare_start(&owner, "test:compressed", true).unwrap();
    drop(owner);
    let decoded = decoded_files(&fixture.store);
    assert_eq!(decoded.len(), 1);
    fixture
        .store
        .remove_tag("test:compressed", &descriptor.digest)
        .unwrap();
    // The workspace still pins the detached image, so its base stays.
    workspace::sweep_materialized(&fixture.store).unwrap();
    assert_eq!(decoded_files(&fixture.store), decoded);
    fixture
        .store
        .open_workspace("pinned")
        .unwrap()
        .discard_and_remove()
        .unwrap();
    let sweep = workspace::sweep_materialized(&fixture.store).unwrap();
    assert!(decoded_files(&fixture.store).is_empty());
    assert_eq!(sweep.removed_bytes, 32 * 512);
    assert_eq!(fixture.store.materialized_bytes().unwrap(), 0);
}
