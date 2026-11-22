// SPDX-License-Identifier: AGPL-3.0-or-later
//! Core fixture tree selection; `backlog/docs/design/repo-split.md` §2.4.

use std::{ffi::OsString, path::PathBuf};

/// The core checkout holding `machines/` and `vendor/`.
///
/// An explicit root must be an existing absolute directory. Invalid settings
/// are configuration errors even when missing fixtures would otherwise skip.
pub fn root() -> PathBuf {
    resolve(std::env::var_os("UNEMUPC_FIXTURE_ROOT")).unwrap_or_else(|reason| {
        panic!("invalid fixture configuration: UNEMUPC_FIXTURE_ROOT {reason}")
    })
}

fn resolve(configured: Option<OsString>) -> Result<PathBuf, &'static str> {
    let Some(configured) = configured else {
        return Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    };
    let path = PathBuf::from(configured);
    if !path.is_absolute() {
        return Err("must be an absolute path");
    }
    if !path.is_dir() {
        return Err("must name an existing directory");
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_root_uses_the_owning_repository() {
        assert_eq!(
            resolve(None).unwrap(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
        );
    }

    #[test]
    fn explicit_root_is_validated_without_fallback() {
        let directory = std::env::temp_dir();
        assert_eq!(resolve(Some(directory.clone().into())).unwrap(), directory);
        for relative in ["", "relative"] {
            assert_eq!(
                resolve(Some(relative.into())),
                Err("must be an absolute path")
            );
        }
        assert_eq!(
            resolve(Some(
                directory.join("missing-fixture-root").join("child").into()
            )),
            Err("must name an existing directory")
        );
        assert_eq!(
            resolve(Some(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("Cargo.toml")
                    .into()
            )),
            Err("must name an existing directory")
        );
    }
}
