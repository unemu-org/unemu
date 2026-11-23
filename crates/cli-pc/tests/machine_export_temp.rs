// SPDX-License-Identifier: AGPL-3.0-or-later
//! Process-isolated TMPDIR checks, separate from tests holding workspace locks.

#[allow(dead_code)]
#[path = "../../pc-image/tests/support/workspace.rs"]
mod support;

#[path = "support/export.rs"]
mod export_support;
use export_support::Export;
use pc_image::machine;
use std::{fs, path::PathBuf, process::Command};

#[test]
fn compressed_layered_export_and_listing_create_no_files_in_tmpdir() {
    let Some(f) = support::Fixture::new() else {
        return;
    };
    let id = machine::create_from_tag(&f.store, "test:source", None, None).unwrap();
    f.changed(&id, 0x23);
    machine::commit(&f.store, &id, "test:layered", None, None, false).unwrap();
    pc_image::compress_image(&f.store, "test:layered", "test:compressed").unwrap();
    let id = machine::create_from_tag(&f.store, "test:compressed", None, None).unwrap();
    let watched = f.store.root().join("watched-tmpdir");
    let output = f.path.join("output");
    fs::create_dir(&watched).unwrap();
    fs::create_dir(&output).unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "export_tempdir_child", "--nocapture"])
        .env("TMPDIR", &watched)
        .env("UNEMU_EXPORT_TEST_STORE", f.store.root())
        .env("UNEMU_EXPORT_TEST_ID", &id)
        .env("UNEMU_EXPORT_TEST_OUTPUT", &output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn export_tempdir_child() {
    let Some(root) = std::env::var_os("UNEMU_EXPORT_TEST_STORE") else {
        return;
    };
    let store = image_store::Store::open(root).unwrap().with_compatibility(
        pc_image::compat::compatibility(image_store::compat::EXTRACTION_BASELINE),
    );
    let id = std::env::var("UNEMU_EXPORT_TEST_ID").unwrap();
    let output = PathBuf::from(std::env::var_os("UNEMU_EXPORT_TEST_OUTPUT").unwrap());
    let watched = std::env::temp_dir();
    let snapshot = machine::export::open(&store, &id).unwrap();
    for action in ["success", "failure", "listing"] {
        let before = fs::metadata(&watched).unwrap().modified().unwrap();
        match action {
            "listing" => {
                snapshot.media(&store).unwrap();
            }
            "success" => {
                let path = output.join("slot-2.img");
                snapshot.export(&store, "C:", &output).unwrap();
                let bytes = fs::read(&path).unwrap();
                assert_eq!(bytes.len(), 32 * 512);
                assert_eq!(&bytes[..512], &[0x23; 512]);
                assert_eq!(
                    fs::read_dir(&output).unwrap().count(),
                    1,
                    "temp file leaked"
                );
                fs::remove_file(path).unwrap();
            }
            _ => {
                let error = snapshot
                    .export_with_hook(&store, "C:", &output, |_| {
                        Err(std::io::Error::other("injected failure"))
                    })
                    .err()
                    .unwrap();
                assert!(error.to_string().contains("injected failure"));
            }
        }
        assert_eq!(
            fs::read_dir(&watched).unwrap().count(),
            0,
            "{action}: TMPDIR is not empty"
        );
        // Creation followed by immediate unlink still changes directory metadata.
        assert_eq!(
            fs::metadata(&watched).unwrap().modified().unwrap(),
            before,
            "{action}: a transient file was created in TMPDIR"
        );
        assert_eq!(
            fs::read_dir(&output).unwrap().count(),
            0,
            "{action}: temp file leaked"
        );
    }
}
