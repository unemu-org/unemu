// SPDX-License-Identifier: AGPL-3.0-or-later
//! Public fixture-root policy across process environments; see
//! `backlog/docs/design/repo-split.md` §2.4.
#![cfg(feature = "test-support")]

use pc_harness::test_support::{at_games, fixtures, root};
use std::{ffi::OsStr, path::PathBuf, process::Command};

fn probe(action: &str, configured: Option<&OsStr>) -> std::process::Output {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "fixture_root_probe", "--nocapture"])
        .env("UNEMUPC_TEST_FIXTURE_ACTION", action)
        .env_remove("UNEMUPC_FIXTURE_ROOT")
        .env_remove("UNEMUPC_REQUIRE_FIXTURES");
    if let Some(configured) = configured {
        command.env("UNEMUPC_FIXTURE_ROOT", configured);
    }
    command.output().unwrap()
}

#[test]
fn fixture_root_probe() {
    let Ok(action) = std::env::var("UNEMUPC_TEST_FIXTURE_ACTION") else {
        return;
    };
    match action.as_str() {
        "unset" => assert_eq!(
            root(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
        ),
        "read" => {
            let expected = PathBuf::from(std::env::var_os("UNEMUPC_FIXTURE_ROOT").unwrap());
            assert_eq!(root(), expected);
            assert_eq!(fixtures::root(), expected);
            assert_eq!(at_games::repository(), expected);
            assert_eq!(
                fixtures::fixture("probe/fixture.bin").unwrap(),
                b"external fixture"
            );
            assert_eq!(
                fixtures::optional_fixture("probe/fixture.bin").unwrap(),
                b"external fixture"
            );
        }
        "invalid" => {
            let _ = fixtures::optional_fixture("probe/missing.bin");
            panic!("invalid fixture root was accepted");
        }
        _ => panic!("unknown fixture probe action"),
    }
}

#[test]
fn unset_fixture_root_uses_the_harness_repository() {
    let output = probe("unset", None);
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn external_fixture_root_supplies_shared_helpers() {
    let temp = machine_file::test_support::TempDir::new("fixture-root");
    let external = temp.path().join("another checkout");
    std::fs::create_dir_all(external.join("vendor/probe")).unwrap();
    std::fs::create_dir(external.join("machines")).unwrap();
    std::fs::write(
        external.join("vendor/probe/fixture.bin"),
        b"external fixture",
    )
    .unwrap();
    let output = probe("read", Some(external.as_os_str()));
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn invalid_fixture_roots_are_configuration_errors_even_for_optional_reads() {
    let temp = machine_file::test_support::TempDir::new("invalid-fixture-root");
    let file = temp.path().join("file");
    std::fs::write(&file, b"not a directory").unwrap();
    for configured in [
        PathBuf::new(),
        "relative".into(),
        temp.path().join("missing"),
        file,
    ] {
        let output = probe("invalid", Some(configured.as_os_str()));
        assert!(!output.status.success(), "{output:?}");
        let errors = String::from_utf8_lossy(&output.stderr);
        assert!(
            errors.contains("invalid fixture configuration: UNEMUPC_FIXTURE_ROOT"),
            "{errors}"
        );
    }
}
