// SPDX-License-Identifier: AGPL-3.0-or-later
//! The emulator platforms compiled into the programs, named in one place so
//! the command line and the desktop list the same cores, accept the same
//! artifacts and print the same licence notice (core-contract.md §7.1,
//! §13.4), and push with the same guard. Two platforms: the IBM PC/AT
//! family, whose core is `unemu-pc-core` and whose image providers and
//! firmware catalogue live in `pc-image`, and the NES, whose core is
//! `unemu-nes-core` from the unemunes repository; it opens cartridge images
//! and has no image providers.

use image_registry::{push, Reference, RegistryEntry};
use image_store::{ConfigValidator, Digest, Manifest, Store};
use unemu_core_api::Core;

/// Every core compiled into the programs, in the order they are asked about
/// a file and listed for people. Each program makes one list of usable cores
/// from this (`core_launch::Cores`) and hands it down; no other crate keeps
/// a list of cores.
///
/// The scripted test core is in the list only with the `test-core` feature,
/// which no release build sets.
pub fn cores() -> &'static [&'static dyn Core] {
    #[cfg(not(feature = "test-core"))]
    static CORES: [&dyn Core; 2] = [&unemu_pc_core::PcCore, &unemu_nes_core::NesCore];
    #[cfg(feature = "test-core")]
    static CORES: [&dyn Core; 3] = [
        &unemu_pc_core::PcCore,
        &unemu_nes_core::NesCore,
        &unemu_test_core::TestCore,
    ];
    &CORES
}

/// Release domains supplied by the composing application and each selected core.
pub fn compatibility(frontend: &str) -> image_store::Result<image_store::compat::Compatibility> {
    compatibility_with_cores(
        frontend,
        cores()
            .iter()
            .copied()
            .filter(|core| unemu_core_api::CONTRACT.drives(core.description().implements)),
    )
}

pub fn compatibility_with_cores<'a>(
    frontend: &str,
    cores: impl IntoIterator<Item = &'a dyn Core>,
) -> image_store::Result<image_store::compat::Compatibility> {
    let mut compatibility = image_store::compat::Compatibility::new(
        frontend.parse()?,
        pc_image::compat::newest_frontend_row(),
    );
    for core in cores {
        let description = core.description();
        let version = description.version.parse()?;
        let newest = if description.id == "unemupc" {
            pc_image::compat::core_newest_row()
        } else {
            version
        };
        for platform in description.platforms {
            compatibility = compatibility.with_core(platform.id, version, newest);
        }
    }
    Ok(compatibility)
}

/// Run `job` with every platform provider a registry transfer may need: PC
/// machine images, PC media sets and PC firmware packages
/// ([`pc_image::with_providers`]). Push and pull pick the one whose artifact
/// type and platform match the manifest. The desktop shell takes the list
/// from here and the command line's PC platform takes the PC's list from
/// `pc-image` too, so both accept the same artifacts.
pub fn with_image_providers<T>(job: impl FnOnce(&[&dyn ConfigValidator]) -> T) -> T {
    pc_image::with_providers(job)
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
    use unemu_core_api::{
        ContentKind, ContractVersion, CoreDescription, CreateRequest, Error, ErrorKind, Machine,
        PlatformDescription,
    };

    /// A wrong pin of a core's contract fails here, not in a user's hands
    /// (core-contract.md §3.2): the programs make their list with the same
    /// check.
    #[test]
    fn every_compiled_in_core_states_a_contract_this_program_drives() {
        let checked = core_launch::Cores::new(cores());
        assert_eq!(checked.refused().len(), 0, "{:?}", checked.refused());
        assert_eq!(checked.usable().count(), cores().len());
    }

    #[test]
    fn the_ids_of_the_compiled_in_cores_are_unique() {
        assert_eq!(repeated_ids(cores()), Vec::<String>::new());
    }

    /// The PC core and the NES core ship, in that order; the test core
    /// joins with its feature alone.
    #[test]
    fn the_list_is_the_pc_and_nes_cores_and_the_test_core_only_with_its_feature() {
        let ids: Vec<_> = cores().iter().map(|core| core.description().id).collect();
        if cfg!(feature = "test-core") {
            assert_eq!(ids, ["unemupc", "unemunes", "unemu-test-core"]);
        } else {
            assert_eq!(ids, ["unemupc", "unemunes"]);
        }
    }

    /// Each id that `list` repeats: a core id, a platform id across all
    /// cores (an artifact names its platform, not its core), or a content
    /// kind's id within its platform.
    fn repeated_ids(list: &[&dyn Core]) -> Vec<String> {
        fn repeats<'a>(what: &str, ids: impl Iterator<Item = &'a str>) -> Vec<String> {
            let mut seen = std::collections::BTreeSet::new();
            ids.filter(|id| !seen.insert(*id))
                .map(|id| format!("{what} {id}"))
                .collect()
        }
        let descriptions = || list.iter().map(|core| core.description());
        let platforms = || descriptions().flat_map(|core| core.platforms);
        let mut found = repeats("core", descriptions().map(|core| core.id));
        found.extend(repeats("platform", platforms().map(|platform| platform.id)));
        for platform in platforms() {
            let what = format!("content kind of {}", platform.id);
            found.extend(repeats(&what, platform.content.iter().map(|kind| kind.id)));
        }
        found
    }

    /// A core that is its description and nothing else.
    struct Described(CoreDescription);

    impl Core for Described {
        fn description(&self) -> &CoreDescription {
            &self.0
        }

        fn create(&self, _request: &CreateRequest<'_>) -> Result<Box<dyn Machine>, Error> {
            Err(Error::new(ErrorKind::Unsupported, "a scripted description"))
        }
    }

    static KINDS: [ContentKind; 3] = [
        ContentKind::new("tape", "Tapes", &["tap"]),
        ContentKind::new("disk", "Disks", &["dsk"]),
        ContentKind::new("tape", "Tape images", &["tzx"]),
    ];
    static SECOND_PC: [PlatformDescription; 1] = [PlatformDescription::new("ibm-pc", "Another PC")];
    static HOME: [PlatformDescription; 2] = [
        PlatformDescription::new("home", "Home computer").with_content(&KINDS),
        PlatformDescription::new("console", "Console").with_content(&KINDS),
    ];
    static OTHER_PC: Described = Described(
        CoreDescription::new(ContractVersion::new(0, 3), "other-pc", "Other PC", "1.0.0")
            .with_platforms(&SECOND_PC),
    );
    static HOME_CORE: Described = Described(
        CoreDescription::new(ContractVersion::new(0, 3), "home", "Home", "1.0.0")
            .with_platforms(&HOME),
    );

    #[test]
    fn a_repeated_core_platform_or_content_kind_id_is_found() {
        let pc: &dyn Core = &unemu_pc_core::PcCore;
        assert_eq!(repeated_ids(&[pc, pc]), ["core unemupc", "platform ibm-pc"]);
        assert_eq!(repeated_ids(&[pc, &OTHER_PC]), ["platform ibm-pc"]);
        // A kind's id is unique within its platform; two platforms may each
        // have a kind of the same id.
        assert_eq!(
            repeated_ids(&[pc, &HOME_CORE]),
            ["content kind of home tape", "content kind of console tape"]
        );
    }

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
    #[test]
    fn compatibility_uses_the_core_description_and_injected_application_version() {
        use image_store::compat::AppVersion as V;
        for (core_version, frontend) in [("0.0.5", 9), ("0.0.9", 5)] {
            let core = Described(
                CoreDescription::new(ContractVersion::new(0, 3), "home", "Home", core_version)
                    .with_platforms(&HOME),
            );
            let runtime =
                compatibility_with_cores(&format!("0.0.{frontend}"), [&core as &dyn Core]).unwrap();
            let levels = runtime.levels("home");
            assert_eq!(levels.core, Some(core_version.parse().unwrap()));
            assert_eq!(levels.frontend, V::new(0, 0, frontend));
            assert_eq!(runtime.levels("unknown").core, None);
        }
    }
}
