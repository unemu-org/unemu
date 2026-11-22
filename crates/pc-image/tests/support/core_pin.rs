// SPDX-License-Identifier: AGPL-3.0-or-later
//! Keep golden expectations tied to the exact core source recorded in the lockfile.

pub fn assert_core_revision(lock: &str, expected_rev: &str) {
    let source = format!(
        "source = \"git+https://github.com/unemu-org/unemupc?rev={expected_rev}#{expected_rev}\""
    );
    let mut packages = 0;
    for line in lock.lines().filter(|line| {
        let lower = line.to_ascii_lowercase();
        lower.trim_start().starts_with("source")
            && [
                "github.com/unemu-org/unemupc",
                "github.com:unemu-org/unemupc",
            ]
            .iter()
            .any(|repository| {
                lower.split_once(repository).is_some_and(|(_, suffix)| {
                    suffix.starts_with(['.', '/', '?', '#', '"']) || suffix.is_empty()
                })
            })
    }) {
        assert_eq!(
            line, source,
            "unemupc pin moved; re-check this file's golden before updating CORE_REV"
        );
        packages += 1;
    }
    assert!(
        packages > 0,
        "unemupc pin missing; re-check this file's golden before updating CORE_REV"
    );
}

// Only the import test runs this regression; the other includers check their own pin.
#[allow(dead_code)]
pub fn assert_alternate_sources_rejected(expected_rev: &str) {
    let canonical = format!(
        "source = \"git+https://github.com/unemu-org/unemupc?rev={expected_rev}#{expected_rev}\""
    );
    for spelling in [
        format!("git+https://github.com/unemu-org/unemupc.git?rev={expected_rev}#{expected_rev}"),
        format!("git+https://github.com/unemu-org/unemupc#{expected_rev}"),
        format!("git+https://github.com/unemu-org/unemupc/?rev={expected_rev}#{expected_rev}"),
        format!("git+ssh://git@github.com/unemu-org/unemupc?rev={expected_rev}#{expected_rev}"),
    ] {
        let lock = format!("{canonical}\nsource = \"{spelling}\"\n");
        assert!(
            std::panic::catch_unwind(|| assert_core_revision(&lock, expected_rev)).is_err(),
            "alternate core source was skipped: {spelling}"
        );
    }
}
