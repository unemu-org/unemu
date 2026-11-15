// SPDX-License-Identifier: AGPL-3.0-or-later
//! Client-side push guard: proprietary content never reaches a public registry.
//!
//! Every descriptor carries an `unemu.redistribution` mark, `open` or
//! `user-supplied`, and an open one an SPDX licence in `unemu.license`; a
//! missing mark, or an open mark without a valid licence, means user-supplied
//! (machine-images §2, §5 and §5.3). A blob's effective class is the most restrictive one recorded for its
//! digest in any local manifest, so an image that labels a vendor ROM `open`
//! cannot carry it out when another local image records it as user-supplied.
//! A `+zstd` payload is also matched by its decoded digest, so the same ROM
//! stored raw in one image and compressed in another shares one class.
//! The guard runs on the manifest's complete closure before any upload:
//!
//! 1. a closure with any user-supplied blob may go only to a registry entry
//!    marked private, and never into the official `unemu/` namespace;
//! 2. a push into `unemu/` requires every blob to be open;
//! 3. a restricted push additionally needs the target repository to refuse
//!    anonymous reads (checked against the registry before the first upload);
//! 4. an open ROM under a copyleft licence (GPL, LGPL, AGPL or MPL family)
//!    goes to a registry not marked private, or into `unemu/`, only with its
//!    linked source archive (`unemu.source`) in the same closure.
//!
//! There is no override.

use crate::{config::RegistryEntry, reference::OFFICIAL_NAMESPACE, Reference};
use image_store::{
    license, media_type, BlobSource, Descriptor, Digest, Manifest, Store, MAX_SMALL_BLOB,
};
use std::{collections::BTreeMap, fmt};

/// Redistribution class of one blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redistribution {
    Open,
    UserSupplied,
}

impl Redistribution {
    /// Absent or unknown evidence defaults to user-supplied: an open mark
    /// counts only together with a valid SPDX licence.
    pub fn of(descriptor: &Descriptor) -> Self {
        if license::open_license(descriptor).is_some() {
            Self::Open
        } else {
            Self::UserSupplied
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::UserSupplied => "user-supplied",
        }
    }
}

/// Where a closure member's effective class was recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkSource {
    /// The pushed manifest's own descriptor.
    ThisImage,
    /// A more restrictive mark in the manifest of another local tag.
    LocalTag(String),
    /// The bytes are a ROM or chip the firmware catalogue lists as
    /// proprietary, whatever any manifest says about them.
    Catalogue,
}

/// One closure member as shown by a dry run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosureEntry {
    pub role: &'static str,
    pub descriptor: Descriptor,
    /// The mark on the pushed manifest's descriptor.
    pub mark: Redistribution,
    /// The most restrictive class any local manifest records for the digest;
    /// the guard decides on this one.
    pub effective: Redistribution,
    pub source: MarkSource,
}

/// Digests that some tagged local manifest records as user-supplied, each
/// with the first such tag in name order, plus digests a caller restricts
/// because the firmware catalogue lists their bytes as proprietary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalMarks {
    restricted: BTreeMap<Digest, MarkSource>,
}

impl LocalMarks {
    /// Read every tagged manifest in the store: machine images, media sets
    /// and any other artifact type. A manifest that cannot be read fails the
    /// scan, because its marks might restrict the closure being pushed.
    pub fn scan(store: &Store) -> crate::Result<Self> {
        let mut restricted = BTreeMap::new();
        let mut after: Option<String> = None;
        loop {
            let page = store.list_tags(after.as_deref(), 1000)?;
            for tag in &page.tags {
                let bytes = store.read_verified(&tag.manifest, MAX_SMALL_BLOB)?;
                let manifest = Manifest::decode_for_summary(&bytes)?;
                for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
                    if Redistribution::of(descriptor) == Redistribution::UserSupplied {
                        for digest in identities(descriptor) {
                            restricted
                                .entry(digest)
                                .or_insert_with(|| MarkSource::LocalTag(tag.name.clone()));
                        }
                    }
                }
            }
            match page.next_after {
                Some(next) => after = Some(next),
                None => return Ok(Self { restricted }),
            }
        }
    }

    /// Record that the manifest of local tag `tag` marks `digest` user-supplied.
    pub fn restrict(&mut self, digest: Digest, tag: impl Into<String>) {
        self.restricted
            .entry(digest)
            .or_insert_with(|| MarkSource::LocalTag(tag.into()));
    }

    /// Record that `digest`, stored or decoded, holds bytes the firmware
    /// catalogue lists as proprietary. The platform-neutral guard has no
    /// catalogue; the frontend that has one restricts the closure with it, so
    /// a pulled package marking such a ROM open still cannot leave.
    pub fn restrict_catalogue(&mut self, digest: Digest) {
        self.restricted.insert(digest, MarkSource::Catalogue);
    }

    fn restricted_by(&self, descriptor: &Descriptor) -> Option<&MarkSource> {
        identities(descriptor)
            .iter()
            .find_map(|digest| self.restricted.get(digest))
    }
}

/// The stored digest and, for a compressed payload, the decoded content digest.
fn identities(descriptor: &Descriptor) -> Vec<Digest> {
    let mut digests = vec![descriptor.digest.clone()];
    if descriptor.is_compressed() {
        digests.extend(descriptor.decoded_digest().ok());
    }
    digests
}

/// The config followed by every layer, in manifest order, with each blob's
/// effective class.
pub fn closure(manifest: &Manifest, local: &LocalMarks) -> Vec<ClosureEntry> {
    std::iter::once(("config", &manifest.config))
        .chain(manifest.layers.iter().map(|d| ("layer", d)))
        .map(|(role, descriptor)| {
            let mark = Redistribution::of(descriptor);
            let (effective, source) = match (mark, local.restricted_by(descriptor)) {
                (Redistribution::Open, Some(source)) => {
                    (Redistribution::UserSupplied, source.clone())
                }
                (mark, _) => (mark, MarkSource::ThisImage),
            };
            ClosureEntry {
                role,
                descriptor: descriptor.clone(),
                mark,
                effective,
                source,
            }
        })
        .collect()
}

/// Why a push was refused. Messages name digests and media types only.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("{repository} is in the official namespace and every blob must be open; user-supplied: {}", list(.blobs))]
    OfficialNamespaceNeedsOpen {
        repository: String,
        blobs: Vec<Descriptor>,
    },
    #[error("{host} is not marked private, so it cannot receive user-supplied blobs: {}", list(.blobs))]
    RegistryNotPrivate {
        host: String,
        blobs: Vec<Descriptor>,
    },
    #[error("{host} is not a configured registry; add it to the registries list first")]
    UnknownRegistry { host: String },
    #[error("{reference} can be read anonymously, so it cannot receive user-supplied blobs")]
    AnonymousReadable { reference: String },
    #[error("{reference} answers anonymous reads with a token challenge, which does not show whether it is private; Bearer authentication is not supported yet, so it cannot receive user-supplied blobs")]
    AnonymousAccessUnverified { reference: String },
    #[error("{target} is in unemu/ or on a registry not marked private, and these open copyleft ROMs have no source archive in the image ({hint}): {}", list(.blobs))]
    CopyleftWithoutSource {
        target: String,
        /// What adds the source to this kind of artifact.
        hint: &'static str,
        blobs: Vec<Descriptor>,
    },
}

fn list(blobs: &[Descriptor]) -> String {
    let shown = blobs
        .iter()
        .take(8)
        .map(|d| format!("{} ({})", d.digest, d.media_type))
        .collect::<Vec<_>>()
        .join(", ");
    match blobs.len() {
        n if n > 8 => format!("{shown} and {} more", n - 8),
        _ => shown,
    }
}

impl fmt::Display for MarkSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ThisImage => f.write_str("this image"),
            Self::LocalTag(tag) => write!(f, "local tag {tag}"),
            Self::Catalogue => f.write_str("firmware catalogue"),
        }
    }
}

/// `role digest size media-type effective-class source`, tab separated.
impl fmt::Display for ClosureEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\t{}\t{}\t{}\t{}\t{}",
            self.role,
            self.descriptor.digest,
            self.descriptor.size,
            self.descriptor.media_type,
            self.effective.as_str(),
            self.source
        )
    }
}

/// Rules 1, 2 and 4, decided from configuration and the closure's effective
/// classes alone; rule 3 needs the registry. `entry` is `None` when the
/// target host is neither configured nor the family default.
pub fn check(
    entry: Option<&RegistryEntry>,
    target: &Reference,
    closure: &[ClosureEntry],
) -> Result<(), Refusal> {
    let entry = entry.ok_or_else(|| Refusal::UnknownRegistry {
        host: target.host.clone(),
    })?;
    let restricted: Vec<_> = closure
        .iter()
        .filter(|c| c.effective == Redistribution::UserSupplied)
        .map(|c| c.descriptor.clone())
        .collect();
    if !restricted.is_empty() && target.namespace() == OFFICIAL_NAMESPACE {
        return Err(Refusal::OfficialNamespaceNeedsOpen {
            repository: target.repository.clone(),
            blobs: restricted,
        });
    }
    if !restricted.is_empty() && !entry.private {
        return Err(Refusal::RegistryNotPrivate {
            host: entry.host.clone(),
            blobs: restricted,
        });
    }
    if entry.private && target.namespace() != OFFICIAL_NAMESPACE {
        return Ok(());
    }
    let missing = copyleft_without_source(closure);
    if !missing.is_empty() {
        let package = closure
            .iter()
            .any(|c| c.descriptor.media_type == media_type::FIRMWARE_CONFIG);
        return Err(Refusal::CopyleftWithoutSource {
            target: target.to_string(),
            hint: if package {
                "image firmware edit --source adds one"
            } else {
                "import the machine again once its firmware package carries the source archive"
            },
            blobs: missing,
        });
    }
    Ok(())
}

/// Rule 4: effectively open ROM layers whose licence obliges the
/// corresponding source but whose linked source archive is absent from the
/// closure ([`license::source_missing`]). Only ROM layers are checked.
pub fn copyleft_without_source(closure: &[ClosureEntry]) -> Vec<Descriptor> {
    let layers: Vec<Descriptor> = closure.iter().map(|c| c.descriptor.clone()).collect();
    closure
        .iter()
        .filter(|c| {
            c.effective == Redistribution::Open && license::source_missing(&c.descriptor, &layers)
        })
        .map(|c| c.descriptor.clone())
        .collect()
}

/// Where rules 1, 2 and 4 let a closure go, judged by its effective classes
/// alone, for a frontend that shows it before any push.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Any configured registry, including `unemu/`.
    Anywhere,
    /// Private registries outside `unemu/` only: a user-supplied blob.
    UserSupplied,
    /// Private registries outside `unemu/` only: an open copyleft ROM
    /// without its linked source archive.
    CopyleftWithoutSource,
}
/// The [`Reach`] of a closure; a user-supplied blob is named before a
/// missing source archive.
pub fn reach(closure: &[ClosureEntry]) -> Reach {
    if closure
        .iter()
        .any(|c| c.effective == Redistribution::UserSupplied)
    {
        Reach::UserSupplied
    } else if !copyleft_without_source(closure).is_empty() {
        Reach::CopyleftWithoutSource
    } else {
        Reach::Anywhere
    }
}

/// Whether rule 3's anonymous-read probe applies to this closure.
pub fn needs_private_probe(closure: &[ClosureEntry]) -> bool {
    closure
        .iter()
        .any(|c| c.effective == Redistribution::UserSupplied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_store::{media_type as mt, Digest};

    /// An open mark comes with a permissive licence, as an open mark requires.
    fn entry(mark: Option<&str>, kind: &str, label: &[u8]) -> ClosureEntry {
        let mut descriptor = Descriptor::new(kind, Digest::of(label), label.len() as u64);
        if let Some(mark) = mark {
            descriptor
                .annotations
                .insert("unemu.redistribution".into(), mark.into());
            if mark == "open" {
                descriptor
                    .annotations
                    .insert("unemu.license".into(), "MIT".into());
            }
        }
        classified(descriptor)
    }
    fn classified(descriptor: Descriptor) -> ClosureEntry {
        let mark = Redistribution::of(&descriptor);
        ClosureEntry {
            role: "layer",
            mark,
            effective: mark,
            source: MarkSource::ThisImage,
            descriptor,
        }
    }

    fn target(input: &str) -> Reference {
        Reference::parse(input, crate::reference::DEFAULT_REGISTRY).unwrap()
    }

    fn registry(private: bool) -> RegistryEntry {
        RegistryEntry {
            private,
            ..RegistryEntry::builtin("registry.unemu.dev")
        }
    }

    #[test]
    fn a_missing_or_unknown_mark_counts_as_user_supplied() {
        assert_eq!(
            entry(None, mt::FIRMWARE, b"a").mark,
            Redistribution::UserSupplied
        );
        assert_eq!(
            entry(Some("open"), mt::FIRMWARE, b"a").mark,
            Redistribution::Open
        );
    }

    #[test]
    fn an_all_open_closure_may_go_anywhere_configured() {
        let open = [
            entry(Some("open"), mt::CONFIG, b"c"),
            entry(Some("open"), mt::FIRMWARE, b"f"),
        ];
        for (private, reference) in [
            (false, "lab:1.0.0"),
            (true, "private/lab:1"),
            (false, "owner/lab:1"),
        ] {
            assert_eq!(
                check(Some(&registry(private)), &target(reference), &open),
                Ok(())
            );
        }
        assert!(!needs_private_probe(&open));
    }

    #[test]
    fn the_official_namespace_refuses_any_user_supplied_blob_even_on_a_private_registry() {
        let mixed = [
            entry(Some("open"), mt::CONFIG, b"c"),
            entry(Some("user-supplied"), mt::FIRMWARE, b"rom"),
        ];
        for private in [false, true] {
            let refusal =
                check(Some(&registry(private)), &target("lab:1.0.0"), &mixed).unwrap_err();
            assert!(
                matches!(refusal, Refusal::OfficialNamespaceNeedsOpen { ref blobs, .. } if blobs.len() == 1)
            );
            assert!(refusal
                .to_string()
                .contains(&Digest::of(b"rom").to_string()));
        }
    }

    #[test]
    fn user_supplied_blobs_need_a_private_registry_outside_the_official_namespace() {
        let unmarked = [entry(None, mt::CONFIG, b"c")];
        assert!(matches!(
            check(Some(&registry(false)), &target("private/lab:1"), &unmarked),
            Err(Refusal::RegistryNotPrivate { .. })
        ));
        assert_eq!(
            check(Some(&registry(true)), &target("private/lab:1"), &unmarked),
            Ok(())
        );
        assert!(needs_private_probe(&unmarked));
    }

    #[test]
    fn another_local_manifest_restricting_a_digest_makes_it_user_supplied() {
        let open = |kind: &str, bytes: &[u8]| {
            let mut descriptor = Descriptor::new(kind, Digest::of(bytes), bytes.len() as u64);
            descriptor
                .annotations
                .insert("unemu.redistribution".into(), "open".into());
            descriptor
                .annotations
                .insert("unemu.license".into(), "CC0-1.0".into());
            descriptor
        };
        let manifest = Manifest::new(
            "synthetic",
            open(mt::CONFIG, b"c"),
            vec![open(mt::FIRMWARE, b"rom"), open(mt::METADATA, b"m")],
        )
        .unwrap();
        let unrestricted = closure(&manifest, &LocalMarks::default());
        assert!(unrestricted
            .iter()
            .all(|c| c.effective == Redistribution::Open && c.source == MarkSource::ThisImage));
        assert_eq!(
            check(
                Some(&registry(false)),
                &target("owner/game:1"),
                &unrestricted
            ),
            Ok(())
        );

        let mut local = LocalMarks::default();
        local.restrict(Digest::of(b"rom"), "rom:1.0.0");
        let restricted = closure(&manifest, &local);
        let rom = &restricted[1];
        assert_eq!(
            (rom.mark, rom.effective),
            (Redistribution::Open, Redistribution::UserSupplied)
        );
        assert_eq!(rom.source, MarkSource::LocalTag("rom:1.0.0".into()));
        assert!(rom
            .to_string()
            .ends_with("\tuser-supplied\tlocal tag rom:1.0.0"));
        assert!(restricted[0].to_string().ends_with("\topen\tthis image"));
        assert!(matches!(
            check(Some(&registry(false)), &target("owner/game:1"), &restricted),
            Err(Refusal::RegistryNotPrivate { ref blobs, .. }) if blobs.len() == 1
        ));
        assert!(needs_private_probe(&restricted));
    }

    #[test]
    fn an_open_mark_without_a_valid_licence_counts_as_user_supplied() {
        // Packages written before licences were recorded mark ROMs open with
        // no licence; the guard reads them in the safe direction.
        for licence in [None, Some("GPL-2.0"), Some("not a licence")] {
            let mut d = Descriptor::new(mt::FIRMWARE, Digest::of(b"old"), 3);
            d.annotations
                .insert("unemu.redistribution".into(), "open".into());
            if let Some(licence) = licence {
                d.annotations.insert("unemu.license".into(), licence.into());
            }
            let old = classified(d);
            assert_eq!(old.effective, Redistribution::UserSupplied, "{licence:?}");
            assert!(matches!(
                check(Some(&registry(false)), &target("owner/rom:1"), &[old]),
                Err(Refusal::RegistryNotPrivate { .. })
            ));
        }
    }

    /// An open ROM under `licence`, linked to `source` when given, and the
    /// config and README a package carries.
    fn package(licence: &str, source: Option<&[u8]>) -> Vec<ClosureEntry> {
        let mut rom = Descriptor::new(mt::FIRMWARE, Digest::of(b"rom"), 3);
        rom.annotations
            .insert("unemu.redistribution".into(), "open".into());
        rom.annotations
            .insert("unemu.license".into(), licence.into());
        if let Some(source) = source {
            rom.annotations
                .insert("unemu.source".into(), Digest::of(source).to_string());
        }
        vec![
            entry(Some("open"), mt::FIRMWARE_CONFIG, b"config"),
            classified(rom),
            entry(Some("open"), mt::MARKDOWN, b"readme"),
        ]
    }
    fn with_archive(mut closure: Vec<ClosureEntry>, kind: &str, bytes: &[u8]) -> Vec<ClosureEntry> {
        closure.push(entry(Some("open"), kind, bytes));
        closure
    }

    #[test]
    fn an_open_copyleft_rom_needs_its_linked_source_outside_private_registries() {
        let without = package("GPL-3.0-or-later", None);
        for (private, reference) in [(false, "owner/bios:1"), (true, "bios:1"), (false, "bios:1")] {
            let refusal =
                check(Some(&registry(private)), &target(reference), &without).unwrap_err();
            assert!(
                matches!(refusal, Refusal::CopyleftWithoutSource { ref blobs, .. }
                    if blobs.len() == 1 && blobs[0].digest == Digest::of(b"rom")),
                "{reference}: {refusal}"
            );
            assert!(refusal
                .to_string()
                .contains(&Digest::of(b"rom").to_string()));
        }
        // A private registry outside the official namespace keeps the user's own copy.
        assert_eq!(
            check(Some(&registry(true)), &target("private/bios:1"), &without),
            Ok(())
        );
        let linked = with_archive(package("GPL-3.0-or-later", Some(b"src")), mt::TAR, b"src");
        for (private, reference) in [(false, "owner/bios:1"), (false, "bios:1"), (true, "bios:1")] {
            assert_eq!(
                check(Some(&registry(private)), &target(reference), &linked),
                Ok(()),
                "{reference}"
            );
        }
        let zip = with_archive(package("LGPL-2.1-or-later", Some(b"src")), mt::ZIP, b"src");
        assert_eq!(
            check(Some(&registry(false)), &target("bios:1"), &zip),
            Ok(())
        );
        // An archive that is not the linked one, or a link without its layer, is no source.
        let unlinked = with_archive(package("GPL-2.0-only", None), mt::TAR, b"src");
        let dangling = package("GPL-2.0-only", Some(b"src"));
        let wrong_kind = with_archive(package("MPL-2.0", Some(b"src")), mt::EXTRA, b"src");
        for closure in [unlinked, dangling, wrong_kind] {
            assert!(matches!(
                check(Some(&registry(false)), &target("owner/bios:1"), &closure),
                Err(Refusal::CopyleftWithoutSource { .. })
            ));
        }
        // Under the active rules any copyleft identifier, even after OR, needs
        // the source.
        assert!(matches!(
            check(
                Some(&registry(false)),
                &target("owner/bios:1"),
                &package("MIT OR GPL-2.0-or-later", None)
            ),
            Err(Refusal::CopyleftWithoutSource { .. })
        ));
        // Permissive licences need no source.
        for licence in ["MIT", "LicenseRef-vendor-grant", "MIT OR Zlib"] {
            assert_eq!(
                check(
                    Some(&registry(false)),
                    &target("bios:1"),
                    &package(licence, None)
                ),
                Ok(()),
                "{licence}"
            );
        }
    }

    #[test]
    fn reach_names_a_user_supplied_blob_before_a_missing_source() {
        assert_eq!(reach(&package("MIT", None)), Reach::Anywhere);
        assert_eq!(
            reach(&package("GPL-2.0-only", None)),
            Reach::CopyleftWithoutSource
        );
        let mut both = package("GPL-2.0-only", None);
        both.push(entry(None, mt::EXTRA, b"manual"));
        assert_eq!(reach(&both), Reach::UserSupplied);
        let linked = with_archive(package("GPL-2.0-only", Some(b"src")), mt::TAR, b"src");
        assert_eq!(reach(&linked), Reach::Anywhere);
    }

    #[test]
    fn a_catalogue_restriction_overrides_an_open_mark_with_a_licence() {
        let config = entry(Some("open"), mt::FIRMWARE_CONFIG, b"config").descriptor;
        let rom = entry(Some("open"), mt::FIRMWARE, b"vendor rom").descriptor;
        let manifest = Manifest::new_firmware("synthetic", config, vec![rom.clone()]).unwrap();
        let mut local = LocalMarks::default();
        local.restrict_catalogue(rom.digest.clone());
        let restricted = closure(&manifest, &local);
        assert_eq!(restricted[1].mark, Redistribution::Open);
        assert_eq!(restricted[1].effective, Redistribution::UserSupplied);
        assert_eq!(restricted[1].source, MarkSource::Catalogue);
        assert!(restricted[1]
            .to_string()
            .ends_with("\tuser-supplied\tfirmware catalogue"));
        assert!(matches!(
            check(Some(&registry(false)), &target("owner/rom:1"), &restricted),
            Err(Refusal::RegistryNotPrivate { ref blobs, .. }) if blobs[0].digest == rom.digest
        ));
        assert!(matches!(
            check(Some(&registry(true)), &target("rom:1"), &restricted),
            Err(Refusal::OfficialNamespaceNeedsOpen { .. })
        ));
    }

    #[test]
    fn the_copyleft_refusal_names_the_way_to_add_the_source_for_the_artifact() {
        let hint = |closure: &[ClosureEntry]| match check(
            Some(&registry(false)),
            &target("owner/bios:1"),
            closure,
        ) {
            Err(Refusal::CopyleftWithoutSource { hint, .. }) => hint,
            other => panic!("{other:?}"),
        };
        let mut machine = package("GPL-2.0-only", None);
        assert!(hint(&machine).contains("image firmware edit --source"));
        machine[0] = entry(Some("open"), mt::CONFIG, b"machine config");
        assert!(hint(&machine).contains("import the machine again"));
    }

    #[test]
    fn an_unconfigured_registry_is_refused() {
        let open = [entry(Some("open"), mt::CONFIG, b"c")];
        assert!(matches!(
            check(None, &target("other.example/lab:1"), &open),
            Err(Refusal::UnknownRegistry { .. })
        ));
    }
}
