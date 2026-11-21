// SPDX-License-Identifier: AGPL-3.0-or-later
//! Stored run refusal precedence before document decoding; machine-images §8.

use image_store::{compat, media_type, Descriptor, Digest, Store, MAX_SMALL_BLOB};
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Lab(PathBuf);
impl Lab {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "run-refusals-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Store::open(&root).unwrap();
        Self(root)
    }

    fn store(&self) -> Store {
        Store::open_existing(&self.0).unwrap()
    }

    fn future_image(&self) -> Descriptor {
        let bytes = format!(
            r#"{{"schemaVersion":99,"mediaType":"{}","artifactType":"{}","config":{{"mediaType":"{}","digest":"{}","size":0}},"layers":[],"annotations":{{"unemu.platform":"ibm-pc","unemu.min-app-version":"99.0.0"}}}}"#,
            media_type::MANIFEST,
            media_type::MACHINE,
            media_type::CONFIG,
            Digest::of(b"")
        );
        let store = self.store();
        let blob = store.import(bytes.as_bytes(), None).unwrap();
        let image = Descriptor::new(media_type::MANIFEST, blob.digest, blob.size);
        store.tag_as_newer_release("test:future", &image).unwrap();
        image
    }

    fn machine(&self, image: Descriptor) {
        self.store()
            .edit_machines(|machines| {
                machines.insert(
                    "run",
                    image_store::machine::Machine::new(
                        "Run",
                        image,
                        "test:future",
                        image_store::machine::Origin::New,
                    ),
                )
            })
            .unwrap();
    }

    fn run(&self, machine: bool, fresh: bool) -> (u8, String, String) {
        let words = if machine {
            vec!["image", "machine", "run", "run"]
        } else {
            vec!["image", "run", "test:future", "--workspace", "run"]
        };
        let mut args: Vec<OsString> = words.into_iter().map(Into::into).collect();
        args.extend(["--store".into(), self.0.clone().into_os_string()]);
        if fresh {
            args.push("--new".into());
        }
        let (mut output, mut errors) = (Vec::new(), Vec::new());
        let code = cli_pc::command::run(&args, &mut output, &mut errors);
        (
            code,
            String::from_utf8(output).unwrap(),
            String::from_utf8(errors).unwrap(),
        )
    }

    fn refusal(&self, machine: bool, fresh: bool, expected: &str) {
        assert_eq!(
            self.run(machine, fresh),
            (1, String::new(), format!("{expected}\n"))
        );
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn release_before_future_manifest_envelope(machine: bool) {
    let lab = Lab::new();
    let image = lab.future_image();
    if machine {
        lab.machine(image);
    }
    lab.refusal(
        machine,
        false,
        &format!(
            "test:future needs unemupc 99.0.0 or newer (this is {})",
            compat::EXTRACTION_BASELINE
        ),
    );
}

#[test]
fn workspace_run_checks_release_before_future_manifest_envelope() {
    release_before_future_manifest_envelope(false);
}

#[test]
fn machine_run_checks_release_before_future_manifest_envelope() {
    release_before_future_manifest_envelope(true);
}

#[test]
fn workspace_run_preserves_session_size_refusal() {
    for fresh in [false, true] {
        let lab = Lab::new();
        let store = lab.store();
        let owner = store.workspace("run").unwrap();
        std::fs::File::create(owner.path().join("session.toml"))
            .unwrap()
            .set_len(MAX_SMALL_BLOB + 1)
            .unwrap();
        drop(owner);
        lab.refusal(false, fresh, "invalid image: document exceeds size limit");
    }
}

#[test]
fn workspace_run_checks_fresh_before_decoding_session() {
    let lab = Lab::new();
    let store = lab.store();
    let owner = store.workspace("run").unwrap();
    owner.write("session.toml", b"\xff").unwrap();
    drop(owner);
    lab.refusal(
        false,
        true,
        "invalid PC image: --new requires a fresh workspace name",
    );
}

fn pending_operations_before_manifest_decoding(machine: bool) {
    for (document, expected) in [
        (
            "discard.toml",
            "discard pending; finish discarding this workspace",
        ),
        (
            "transaction.toml",
            "publication pending; retry or abandon publication before running",
        ),
    ] {
        let lab = Lab::new();
        let image = lab.future_image();
        if machine {
            lab.machine(image);
        }
        let store = lab.store();
        let owner = store.workspace("run").unwrap();
        owner.write(document, b"\xff").unwrap();
        drop(owner);
        lab.refusal(machine, false, &format!("invalid PC image: {expected}"));
    }
}

#[test]
fn workspace_run_checks_pending_operations_before_manifest_decoding() {
    pending_operations_before_manifest_decoding(false);
}

#[test]
fn machine_run_checks_pending_operations_before_manifest_decoding() {
    pending_operations_before_manifest_decoding(true);
}

#[test]
fn workspace_run_checks_orphaned_media_before_reading_tag() {
    let lab = Lab::new();
    let store = lab.store();
    let owner = store.workspace("run").unwrap();
    std::fs::create_dir(owner.path().join("media")).unwrap();
    std::fs::write(owner.path().join("media/base.img"), b"base").unwrap();
    drop(owner);
    lab.refusal(
        false,
        false,
        "invalid PC image: orphaned media without a session; refusing to overwrite it",
    );
}

#[test]
fn machine_run_checks_ownership_before_reading_session() {
    let lab = Lab::new();
    lab.machine(lab.future_image());
    let store = lab.store();
    let owner = store.workspace("run").unwrap();
    owner.write("session.toml", b"\xff").unwrap();
    lab.refusal(
        true,
        false,
        "invalid PC image: machine run is running; stop it first",
    );
}

#[test]
fn machine_run_preserves_owned_session_size_refusal() {
    let lab = Lab::new();
    lab.machine(lab.future_image());
    let store = lab.store();
    let owner = store.workspace("run").unwrap();
    std::fs::File::create(owner.path().join("session.toml"))
        .unwrap()
        .set_len(MAX_SMALL_BLOB + 1)
        .unwrap();
    drop(owner);
    lab.refusal(true, false, "invalid image: document exceeds size limit");
    lab.refusal(
        false,
        false,
        "invalid PC image: workspace document exceeds size limit",
    );
}

#[test]
fn workspace_run_checks_pin_mismatch_before_manifest_decoding() {
    let lab = Lab::new();
    let image = lab.future_image();
    let pinned = Descriptor::new(media_type::MANIFEST, Digest::of(b"missing"), 7);
    let store = lab.store();
    let owner = store.workspace("run").unwrap();
    let session = format!(
        "schema=1\nmachine='{}'\n[image]\n{}",
        Digest::of(b""),
        toml::to_string(&pinned).unwrap()
    );
    owner.write("session.toml", session.as_bytes()).unwrap();
    drop(owner);
    lab.refusal(
        false,
        false,
        &format!(
            "invalid PC image: workspace pins {}; requested tag points to {}",
            pinned.digest, image.digest
        ),
    );
}
