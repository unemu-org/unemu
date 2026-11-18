// SPDX-License-Identifier: AGPL-3.0-or-later
//! The emulator platforms compiled into the programs, named in one place so
//! the command line and the desktop accept the same artifacts and print the
//! same licence notice (core-contract.md §7.1), and push with the same
//! guard. Today that is one platform, the IBM PC/AT family, whose image
//! providers and firmware catalogue live in `pc-image`.

use image_registry::{push, Reference, RegistryEntry};
use image_store::{firmware, media_set, ConfigValidator, Digest, Manifest, Store};
use pc_image::{
    compat::WithPcRows,
    firmware::{catalogue, PcFirmwareRules},
};

const MEDIA_SETS: media_set::Validator<'static> =
    media_set::Validator(&pc_image::media_set::PcMediaFormats);

/// Run `job` with every platform provider a registry transfer may need: PC
/// machine images, PC media sets and PC firmware packages. Push and pull pick
/// the one whose artifact type and platform match the manifest. The command
/// line and the desktop shell both take the list from here, so they accept
/// the same artifacts. Each rates an artifact with the PC rows of the
/// compatibility table, which the push guard of `unemu/` checks.
pub fn with_image_providers<T>(job: impl FnOnce(&[&dyn ConfigValidator]) -> T) -> T {
    let firmware_rules = PcFirmwareRules(catalogue());
    let firmware = firmware::Validator(&firmware_rules);
    job(&[
        &pc_image::PcProvider,
        &WithPcRows(&MEDIA_SETS),
        &WithPcRows(&firmware),
    ])
}

/// The push plan of `local_tag` that the desktop shell uses: every local
/// mark, plus a restriction for each blob [`restricted`] names
/// ([`push::plan_with_catalogue`]). Binding the platform-neutral plan to the
/// catalogue here keeps the desktop from planning a push without it; the
/// command line plans through its PC platform, which restricts with
/// [`restricted`] too.
pub fn push_plan(
    store: &Store,
    local_tag: &str,
    target: Reference,
    entry: Option<&RegistryEntry>,
    catalogue: &[assembly::FirmwareEntry],
) -> image_registry::Result<push::PushPlan> {
    push::plan_with_catalogue(store, local_tag, target, entry, |manifest| {
        restricted(manifest, catalogue)
    })
}

/// The blobs of `manifest` whose bytes, stored or decoded, the firmware
/// `catalogue` lists as a proprietary image or chip: what the push guard of
/// both programs restricts, whatever the image's own marks say.
pub fn restricted(
    manifest: &Manifest,
    catalogue: &[assembly::FirmwareEntry],
) -> image_store::Result<Vec<Digest>> {
    pc_image::firmware::restricted(manifest, catalogue)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_platform_artifact_type_has_a_provider() {
        let types = with_image_providers(|providers| {
            providers
                .iter()
                .map(|p| (p.artifact_type().to_owned(), p.platform().to_owned()))
                .collect::<Vec<_>>()
        });
        let expected = [
            image_store::media_type::MACHINE,
            image_store::media_type::MEDIA_SET,
            image_store::media_type::FIRMWARE,
        ];
        assert_eq!(
            types.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            expected
        );
        for (_, platform) in &types {
            assert!(pc_image::is_pc(platform), "{platform}");
        }
    }

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
