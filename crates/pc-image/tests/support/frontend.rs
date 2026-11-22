// SPDX-License-Identifier: AGPL-3.0-or-later
//! Front-end recipe media stay with their fetch scripts and licence index;
//! `backlog/docs/design/repo-split.md` §2.4.

use std::{ffi::OsStr, io::ErrorKind, path::PathBuf};

pub fn vendor() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor")
}

pub fn fixture(relative: &str) -> Option<Vec<u8>> {
    let path = vendor().join(relative);
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let message = format!(
                "missing fixture {} — run vendor/freedos/fetch-floppy-1.4.sh",
                path.display()
            );
            assert!(
                std::env::var_os("UNEMUPC_REQUIRE_FIXTURES").as_deref() != Some(OsStr::new("1")),
                "{message}"
            );
            eprintln!("skipping: {message}");
            None
        }
        Err(error) => panic!("cannot read fixture {}: {error}", path.display()),
    }
}

#[test]
fn recipe_media_probe() {
    if std::env::var_os("UNEMUPC_RECIPE_MEDIA_PROBE").is_some() {
        let core = PathBuf::from(std::env::var_os("UNEMUPC_FIXTURE_ROOT").unwrap());
        assert!(!core.join("vendor/freedos/floppy-1.4-licenses.txt").exists());
        assert!(!fixture("freedos/floppy-1.4-licenses.txt")
            .unwrap()
            .is_empty());
    }
}

#[test]
fn recipe_media_stay_in_the_frontend_checkout() {
    let temp = machine_file::test_support::TempDir::new("recipe-media-root");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "frontend::recipe_media_probe", "--nocapture"])
        .env("UNEMUPC_FIXTURE_ROOT", temp.path())
        .env("UNEMUPC_REQUIRE_FIXTURES", "1")
        .env("UNEMUPC_RECIPE_MEDIA_PROBE", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}
