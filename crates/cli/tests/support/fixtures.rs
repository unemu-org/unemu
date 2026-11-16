// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared fixture policy for firmware/media goldens (see `backlog/docs/commands.md`).

use std::{io::ErrorKind, path::PathBuf};

pub fn fixture(relative: &str) -> Option<Vec<u8>> {
    read_fixture(relative, true)
}

/// Local-only fixtures remain optional under the CI boot-fixture gate.
#[allow(dead_code)]
pub fn optional_fixture(relative: &str) -> Option<Vec<u8>> {
    read_fixture(relative, false)
}

fn read_fixture(relative: &str, required_by_gate: bool) -> Option<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor")
        .join(relative);
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let directory = relative.split('/').next().expect("fixture directory");
            // The FreeDOS 1.4 Floppy Edition has a fetch script of its own.
            let script = if relative.starts_with("freedos/floppy-1.4/") {
                "fetch-floppy-1.4.sh"
            } else {
                fixture_script(directory)
            };
            let message = format!(
                "missing fixture {} — run vendor/{directory}/{script}",
                path.display()
            );
            assert!(
                !required_by_gate
                    || std::env::var_os("UNEMUPC_REQUIRE_FIXTURES").as_deref()
                        != Some(std::ffi::OsStr::new("1")),
                "{message}"
            );
            eprintln!("skipping: {message}");
            None
        }
        Err(error) => panic!("cannot read fixture {}: {error}", path.display()),
    }
}

/// Fixtures built offline from committed sources use `build.sh`; fixtures
/// fetched from upstream use `fetch.sh`.
fn fixture_script(directory: &str) -> &'static str {
    let vendor = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor");
    if vendor.join(directory).join("build.sh").is_file() {
        "build.sh"
    } else {
        "fetch.sh"
    }
}

/// Read exactly one SHA-256 pin from a fetched-fixture script.
#[allow(dead_code)]
pub fn parse_pin<'a>(script: &'a str, prefix: &str) -> Result<&'a str, &'static str> {
    let mut matches = script
        .lines()
        .filter_map(|line| line.trim().strip_prefix(prefix));
    let value = matches
        .next()
        .filter(|_| matches.next().is_none())
        .ok_or("script format changed: expected exactly one output SHA-256 pin line")?;
    value
        .strip_suffix('"')
        .filter(|pin| pin.len() == 64 && pin.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("script format changed: expected a quoted 64-digit output SHA-256 pin")
}
