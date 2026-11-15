// SPDX-License-Identifier: AGPL-3.0-or-later
//! Exercise the golden fixture policy in child processes to isolate environment
//! variables from other tests; see `backlog/docs/commands.md`.

#[path = "support/fixtures.rs"]
mod fixtures;

#[test]
fn fixture_probe() {
    if let Some(relative) = std::env::var_os("UNEMUPC98_FIXTURE_PROBE") {
        assert!(fixtures::fixture(relative.to_str().unwrap()).is_none());
    }
}

#[test]
fn missing_fixture_skips_unless_required() {
    // The message names the directory's own script: fetched fixtures use
    // fetch.sh, fixtures built from committed sources use build.sh.
    for (relative, script) in [
        (
            "freedos/missing-fixture-policy.img",
            "vendor/freedos/fetch.sh",
        ),
        ("hdd/missing-fixture-policy.img", "vendor/hdd/build.sh"),
        (
            "at-pm-test/missing-fixture-policy.img",
            "vendor/at-pm-test/build.sh",
        ),
    ] {
        for required in [None, Some("0"), Some("1")] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "fixture_probe", "--nocapture"])
                .env("UNEMUPC98_FIXTURE_PROBE", relative)
                .env_remove("UNEMUPC98_REQUIRE_FIXTURES");
            if let Some(value) = required {
                command.env("UNEMUPC98_REQUIRE_FIXTURES", value);
            }
            let output = command.output().unwrap();
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert_eq!(output.status.success(), required != Some("1"), "{stderr}");
            assert!(stderr.contains("missing-fixture-policy.img"), "{stderr}");
            assert!(stderr.contains(script), "{stderr}");
            if required != Some("1") {
                assert!(stderr.contains("skipping:"), "{stderr}");
            }
        }
    }
}
