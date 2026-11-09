// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared fixture policy for firmware/media goldens (see `backlog/docs/commands.md`).

use std::{io::ErrorKind, path::PathBuf};

pub fn fixture(relative: &str) -> Option<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor")
        .join(relative);
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let directory = relative.split('/').next().expect("fixture directory");
            let message = format!(
                "missing fixture {} — run vendor/{directory}/fetch.sh",
                path.display()
            );
            assert!(
                std::env::var_os("UNEMUPC98_REQUIRE_FIXTURES").as_deref()
                    != Some(std::ffi::OsStr::new("1")),
                "{message}"
            );
            eprintln!("skipping: {message}");
            None
        }
        Err(error) => panic!("cannot read fixture {}: {error}", path.display()),
    }
}
