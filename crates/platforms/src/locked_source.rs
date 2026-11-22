// SPDX-License-Identifier: AGPL-3.0-or-later
//! Locked source validation for the combined program's licence notice.

// Check the whole core graph: a partial patch must not advertise one revision
// as though it described every package compiled from that repository.
pub const PC_PACKAGES: &[&str] = &[
    "unemu-pc-core",
    "assembly",
    "audio",
    "bus",
    "chipset",
    "cpu",
    "device",
    "devices",
    "journal",
    "journal-files",
    "machine",
    "machine-file",
    "pc-config",
    "pc-harness",
    "unemupc",
    "video",
];
pub const NES_PACKAGES: &[&str] = &["unemu-nes-core", "nes", "x65"];

pub fn core_source(
    lock: &str,
    packages: &[&str],
    repository: &str,
    allow_local: bool,
) -> Result<String, String> {
    let mut sources = Vec::new();
    for package in packages {
        let blocks: Vec<_> = lock
            .split("[[package]]")
            .filter(|block| {
                block
                    .lines()
                    .any(|line| line == format!("name = \"{package}\""))
            })
            .collect();
        if blocks.len() != 1 {
            return Err(format!("Cargo.lock must contain exactly one {package}"));
        }
        let source = blocks[0]
            .lines()
            .find_map(|line| line.strip_prefix("source = \"")?.strip_suffix('"'));
        let revision = match source {
            None => None,
            Some(source) => {
                let prefix = format!("git+{repository}?rev=");
                match source.strip_prefix(&prefix).and_then(|s| s.split_once('#')) {
                    Some((requested, resolved))
                        if requested == resolved
                            && resolved.len() == 40
                            && resolved.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
                    {
                        Some(resolved)
                    }
                    _ => return Err(format!("{package} needs a pinned git source from {repository} in Cargo.lock (rev=<40 hex>#<same 40 hex>)")),
                }
            }
        };
        sources.push(revision);
    }
    if sources.windows(2).any(|pair| pair[0] != pair[1]) {
        return Err(format!("Cargo.lock contains mixed sources for {repository}; all core packages must use the same git revision or all use a local checkout"));
    }
    match sources.first() {
        Some(Some(revision)) => Ok(format!("{repository}/tree/{revision}")),
        Some(None) if allow_local => Ok(format!("{repository} (local checkout)")),
        Some(None) => Err(format!("release builds require a pinned git source for {repository} in Cargo.lock; local checkout refused")),
        None => Err(format!("Cargo.lock core package list is empty for {repository}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPOSITORY: &str = "https://example.org/core";
    const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
    const PACKAGES: &[&str] = &["core", "device"];

    fn package(name: &str, revision: Option<&str>) -> String {
        let mut block = format!("[[package]]\nname = \"{name}\"\n");
        if let Some(revision) = revision {
            block.push_str(&format!(
                "source = \"git+{REPOSITORY}?rev={revision}#{revision}\"\n"
            ));
        }
        block
    }

    #[test]
    fn git_core_notice_names_the_exact_revision_in_development_and_release() {
        let lock = package("core", Some(REVISION)) + &package("device", Some(REVISION));
        for allow_local in [true, false] {
            assert_eq!(
                core_source(&lock, PACKAGES, REPOSITORY, allow_local).unwrap(),
                format!("{REPOSITORY}/tree/{REVISION}")
            );
        }
    }

    #[test]
    fn path_core_has_an_honest_local_notice_but_is_refused_for_release() {
        let lock = package("core", None) + &package("device", None);
        assert_eq!(
            core_source(&lock, PACKAGES, REPOSITORY, true).unwrap(),
            format!("{REPOSITORY} (local checkout)")
        );
        let error = core_source(&lock, PACKAGES, REPOSITORY, false).unwrap_err();
        assert!(error.contains("release builds"), "{error}");
        assert!(error.contains("local checkout refused"), "{error}");
    }

    #[test]
    fn mixed_path_and_git_or_different_revisions_are_rejected_clearly() {
        for revision in [None, Some("abcdef0123456789abcdef0123456789abcdef01")] {
            let lock = package("core", Some(REVISION)) + &package("device", revision);
            for allow_local in [true, false] {
                let error = core_source(&lock, PACKAGES, REPOSITORY, allow_local).unwrap_err();
                assert!(
                    error.contains("Cargo.lock contains mixed sources"),
                    "{error}"
                );
                assert!(error.contains(REPOSITORY), "{error}");
            }
        }
    }

    #[test]
    fn missing_duplicate_or_unpinned_core_source_is_rejected_clearly() {
        for lock in [
            String::new(),
            package("core", None),
            package("core", None) + &package("core", None) + &package("device", None),
            package("core", Some("main")) + &package("device", None),
        ] {
            let error = core_source(&lock, PACKAGES, REPOSITORY, true).unwrap_err();
            assert!(
                error.contains("core") || error.contains("device"),
                "{error}"
            );
            assert!(error.contains("Cargo.lock"), "{error}");
        }
    }
}
