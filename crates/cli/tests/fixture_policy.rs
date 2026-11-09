// SPDX-License-Identifier: AGPL-3.0-or-later
//! Exercise the golden fixture policy in child processes to isolate environment
//! variables from other tests; see `backlog/docs/commands.md`.

#[path = "support/fixtures.rs"]
mod fixtures;

#[test]
fn fixture_probe() {
    if std::env::var_os("UNEMUPC98_FIXTURE_PROBE").is_some() {
        assert!(fixtures::fixture("freedos/missing-fixture-policy.img").is_none());
    }
}

#[test]
fn missing_fixture_skips_unless_required() {
    for required in [None, Some("0"), Some("1")] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "fixture_probe", "--nocapture"])
            .env("UNEMUPC98_FIXTURE_PROBE", "1")
            .env_remove("UNEMUPC98_REQUIRE_FIXTURES");
        if let Some(value) = required {
            command.env("UNEMUPC98_REQUIRE_FIXTURES", value);
        }
        let output = command.output().unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.success(), required != Some("1"), "{stderr}");
        assert!(stderr.contains("missing-fixture-policy.img"), "{stderr}");
        assert!(stderr.contains("vendor/freedos/fetch.sh"), "{stderr}");
        if required != Some("1") {
            assert!(stderr.contains("skipping:"), "{stderr}");
        }
    }
}
