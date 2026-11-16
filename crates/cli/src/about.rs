// SPDX-License-Identifier: AGPL-3.0-or-later
//! Licence, source and third-party notices that both binaries print for
//! `--version` and `--about`.

/// The program's licence, where the source of this very build is and where
/// the full licence texts are.
pub fn licence_notice() -> String {
    format!(
        "licence: AGPL-3.0-or-later\nsource: {}\n{}{}",
        source_pointer(option_env!("UNEMUPC_SOURCE_REV")),
        commit_line(option_env!("UNEMUPC_SOURCE_COMMIT")),
        concat!(
            "full licence texts: LICENSE, and THIRD-PARTY-LICENSES.md for the components\n",
            "and Rust crates inside, both shipped with the binaries; in the source,\n",
            "scripts/third-party-licenses.sh writes THIRD-PARTY-LICENSES.md",
        ),
    )
}

/// The corresponding source: the repository at the tag or commit a release
/// build names in `UNEMUPC_SOURCE_REV` at compile time, otherwise the
/// repository and the package version.
fn source_pointer(revision: Option<&str>) -> String {
    let repository = env!("CARGO_PKG_REPOSITORY");
    match revision.filter(|revision| !revision.is_empty()) {
        Some(revision) => format!("{repository}/tree/{revision}"),
        None => format!("{repository}, version {}", env!("CARGO_PKG_VERSION")),
    }
}

/// The full commit a release build was made from, from `UNEMUPC_SOURCE_COMMIT`
/// at compile time: a tag can be moved, a commit cannot, so a pipeline that
/// pins this binary can compare the commit. Empty for other builds.
fn commit_line(commit: Option<&str>) -> String {
    match commit.filter(|commit| !commit.is_empty()) {
        Some(commit) => format!("commit: {commit}\n"),
        None => String::new(),
    }
}

/// `upcli --version` and `upcli --about`.
pub fn upcli_notice() -> String {
    format!(
        "upcli {}\nemulator of period IBM-compatible PCs, command line\n{}\n\nthird-party components:\n\n{}\n",
        env!("CARGO_PKG_VERSION"),
        licence_notice(),
        devices::OPL3_NOTICE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_source_pointer_names_the_revision_a_release_build_was_made_from() {
        assert_eq!(
            source_pointer(Some("v1.0.0")),
            "https://github.com/unemu-org/unemupc/tree/v1.0.0"
        );
        let unnamed = format!(
            "https://github.com/unemu-org/unemupc, version {}",
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(source_pointer(None), unnamed);
        assert_eq!(source_pointer(Some("")), unnamed);
        assert!(licence_notice().contains("source: https://github.com/unemu-org/unemupc"));
    }

    #[test]
    fn a_release_build_names_its_full_commit() {
        let commit = "55d8a8c0123456789abcdef0123456789abcdef0";
        assert_eq!(commit_line(Some(commit)), format!("commit: {commit}\n"));
        assert_eq!(commit_line(None), "");
        assert_eq!(commit_line(Some("")), "");
    }
}
