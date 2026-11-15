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
//! 1. a closure with any user-supplied blob may go only to a namespace the
//!    registry entry marks private, or to an entry without namespaces that is
//!    marked private as a whole, and never into the official `unemu/`
//!    namespace, which is never private on any host;
//! 2. a push into `unemu/` requires every blob to be open;
//! 3. a restricted push additionally needs the target repository to refuse
//!    anonymous reads (checked against the registry before the first upload);
//! 4. an open ROM under a copyleft licence (GPL, LGPL, AGPL or MPL family)
//!    goes to a namespace or registry not marked private, or into `unemu/`,
//!    only with its linked source archive (`unemu.source`) in the same closure;
//! 5. open installation media go to a namespace or registry not marked
//!    private, or into `unemu/`, only with the vendor's terms text they link
//!    (`unemu.terms`) as an open extra in the same closure; in a media set
//!    every medium and extra links the one terms text of the set
//!    (machine-images §5.2).
//!
//! An entry that lists namespaces takes pushes only into them: a push to a
//! repository outside every listed namespace is refused whatever it holds
//! ([`crate::config::Scope`]). There is no override.

use crate::{
    config::{RegistryEntry, Scope},
    reference::OFFICIAL_NAMESPACE,
    Reference,
};
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
    #[error("{repository} is in the official namespace and every blob must be open; user-supplied: {}{hint}", list(.blobs))]
    OfficialNamespaceNeedsOpen {
        repository: String,
        blobs: Vec<Descriptor>,
        /// What clears the restriction, when the closure shows it; empty
        /// otherwise.
        hint: &'static str,
    },
    #[error("{host} is not marked private, so it cannot receive user-supplied blobs: {}{hint}", list(.blobs))]
    RegistryNotPrivate {
        host: String,
        blobs: Vec<Descriptor>,
        /// As for [`Refusal::OfficialNamespaceNeedsOpen`].
        hint: &'static str,
    },
    #[error("{host}/{namespace}/ is not marked private, so it cannot receive user-supplied blobs: {}{hint}", list(.blobs))]
    NamespaceNotPrivate {
        host: String,
        namespace: String,
        blobs: Vec<Descriptor>,
        /// As for [`Refusal::OfficialNamespaceNeedsOpen`].
        hint: &'static str,
    },
    #[error("{}", unlisted(.host, .repository, .namespace.as_deref()))]
    UnlistedNamespace {
        host: String,
        repository: String,
        /// The repository's first path component; `None` when its name has one.
        namespace: Option<String>,
    },
    #[error("{host} is not a configured registry; add it to the registries list first")]
    UnknownRegistry { host: String },
    #[error("{reference} can be read anonymously, so it cannot receive user-supplied blobs")]
    AnonymousReadable { reference: String },
    #[error("{reference} answers anonymous reads with a token challenge, which does not show whether it is private; Bearer authentication is not supported yet, so it cannot receive user-supplied blobs")]
    AnonymousAccessUnverified { reference: String },
    #[error("{target} is in unemu/ or in a namespace or registry not marked private, and these open copyleft ROMs have no source archive in the image ({hint}): {}", list(.blobs))]
    CopyleftWithoutSource {
        target: String,
        /// What adds the source to this kind of artifact.
        hint: &'static str,
        blobs: Vec<Descriptor>,
    },
    #[error("{target} is in unemu/ or on a registry not marked private, and these open installation media do not link one open terms text of their set in the image (unemu.terms; {hint}): {}", list(.blobs))]
    TermsWithoutText {
        target: String,
        /// What records the link for this kind of artifact.
        hint: &'static str,
        blobs: Vec<Descriptor>,
    },
}

fn unlisted(host: &str, repository: &str, namespace: Option<&str>) -> String {
    match namespace {
        Some(namespace) => {
            format!("{host}/{namespace}/ is not a configured namespace on this registry")
        }
        None => {
            format!("{host}/{repository} is in none of the namespaces configured on this registry")
        }
    }
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

/// Rules 1, 2, 4 and 5, decided from configuration and the closure's
/// effective classes alone; rule 3 needs the registry. `entry` is `None` when
/// the target host is neither configured nor the family default.
pub fn check(
    entry: Option<&RegistryEntry>,
    target: &Reference,
    closure: &[ClosureEntry],
) -> Result<(), Refusal> {
    let entry = entry.ok_or_else(|| Refusal::UnknownRegistry {
        host: target.host.clone(),
    })?;
    let scope = entry.scope(&target.repository);
    if let Scope::Unlisted { namespace } = scope {
        return Err(Refusal::UnlistedNamespace {
            host: entry.host.clone(),
            repository: target.repository.clone(),
            namespace,
        });
    }
    let restricted: Vec<_> = closure
        .iter()
        .filter(|c| c.effective == Redistribution::UserSupplied)
        .map(|c| c.descriptor.clone())
        .collect();
    let official = target.namespace() == OFFICIAL_NAMESPACE;
    if !restricted.is_empty() && official {
        return Err(Refusal::OfficialNamespaceNeedsOpen {
            repository: target.repository.clone(),
            blobs: restricted,
            hint: restriction_hint(closure),
        });
    }
    if !restricted.is_empty() && !scope.private() {
        return Err(match scope {
            Scope::Namespace { name, .. } => Refusal::NamespaceNotPrivate {
                host: entry.host.clone(),
                namespace: name,
                blobs: restricted,
                hint: restriction_hint(closure),
            },
            _ => Refusal::RegistryNotPrivate {
                host: entry.host.clone(),
                blobs: restricted,
                hint: restriction_hint(closure),
            },
        });
    }
    if scope.private() && !official {
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
    let unlinked = terms_without_text(closure);
    if !unlinked.is_empty() {
        let set = closure
            .iter()
            .any(|c| c.descriptor.media_type == media_type::MEDIA_SET_CONFIG);
        return Err(Refusal::TermsWithoutText {
            target: target.to_string(),
            hint: if set {
                "import the set again under a new tag with --open --license LicenseRef-NAME --license-text EXTRA"
            } else {
                "the image must carry the terms text of the set its media come from"
            },
            blobs: unlinked,
        });
    }
    Ok(())
}

/// What clears a user-supplied restriction, when the closure shows it: a
/// record in another local tag, which only deleting that tag (the library's
/// Delete tag) or pushing from a store without it removes; or a media set
/// imported without the open statement, which only a new import under a new
/// tag can open, because the marks are part of what the tag names.
fn restriction_hint(closure: &[ClosureEntry]) -> &'static str {
    let restricted = |c: &&ClosureEntry| c.effective == Redistribution::UserSupplied;
    if closure
        .iter()
        .filter(restricted)
        .any(|c| matches!(c.source, MarkSource::LocalTag(_)))
    {
        "; another local tag records some of these bytes user-supplied (image push --dry-run names it): delete that tag in the library, or push from a store without it"
    } else if closure
        .iter()
        .any(|c| c.descriptor.media_type == media_type::MEDIA_SET_CONFIG)
    {
        "; a media set is open only when imported under a new tag with --open --license LicenseRef-NAME --license-text EXTRA"
    } else {
        ""
    }
}

/// Rule 5: effectively open layers whose vendor terms text does not travel
/// with them ([`license::terms_unlinked`]).
pub fn terms_without_text(closure: &[ClosureEntry]) -> Vec<Descriptor> {
    let layers: Vec<Descriptor> = closure.iter().map(|c| c.descriptor.clone()).collect();
    let unlinked = license::terms_unlinked(&layers);
    closure
        .iter()
        .filter(|c| c.effective == Redistribution::Open && unlinked.contains(&c.descriptor))
        .map(|c| c.descriptor.clone())
        .collect()
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
    /// Private namespaces or registries outside `unemu/` only: a
    /// user-supplied blob.
    UserSupplied,
    /// Private namespaces or registries outside `unemu/` only: an open
    /// copyleft ROM without its linked source archive.
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

    /// A layer of an open shareware set: the vendor's terms by reference and,
    /// when given, the link to the terms text's bytes.
    fn shareware(kind: &str, bytes: &[u8], terms: Option<&[u8]>) -> Descriptor {
        let mut d = Descriptor::new(kind, Digest::of(bytes), bytes.len() as u64);
        let vendor = license::Expression::parse("LicenseRef-synthetic-shareware").unwrap();
        license::mark(&mut d, Some(&vendor));
        if let Some(terms) = terms {
            d.annotations
                .insert(license::TERMS.into(), Digest::of(terms).to_string());
        }
        d
    }
    /// A complete open media set: two disks and the terms text, all linking
    /// it, and the generated config and metadata open under their own licence.
    fn open_set() -> Vec<ClosureEntry> {
        vec![
            entry(Some("open"), mt::MEDIA_SET_CONFIG, b"set config"),
            classified(shareware(mt::FLOPPY, b"disk 1", Some(b"terms"))),
            classified(shareware(mt::FLOPPY, b"disk 2", Some(b"terms"))),
            classified(shareware(mt::EXTRA, b"terms", Some(b"terms"))),
            entry(Some("open"), mt::METADATA, b"set metadata"),
        ]
    }
    fn verdict(private: bool, reference: &str, closure: &[ClosureEntry]) -> Result<(), Refusal> {
        check(Some(&registry(private)), &target(reference), closure)
    }

    #[test]
    fn a_complete_open_media_set_may_enter_the_official_namespace() {
        let set = open_set();
        for (private, reference) in [
            (false, "shareware:1"),
            (true, "shareware:1"),
            (false, "owner/shareware:1"),
            (true, "private/shareware:1"),
        ] {
            assert_eq!(verdict(private, reference, &set), Ok(()), "{reference}");
        }
        assert!(terms_without_text(&set).is_empty());
        assert!(!needs_private_probe(&set));
    }

    #[test]
    fn an_open_media_set_without_its_linked_terms_text_stays_private() {
        let disk = |n: &[u8]| shareware(mt::FLOPPY, n, Some(b"terms"));
        // The terms text is not in the closure.
        let mut absent = open_set();
        absent.remove(3);
        // One disk's link is missing, or names another extra of the set.
        let mut missing = open_set();
        missing[2] = classified(shareware(mt::FLOPPY, b"disk 2", None));
        let mut other = open_set();
        other.insert(
            3,
            classified(shareware(mt::EXTRA, b"manual", Some(b"terms"))),
        );
        other[2] = classified(shareware(mt::FLOPPY, b"disk 2", Some(b"manual")));
        // The terms text is restricted by another local tag.
        let mut local = LocalMarks::default();
        local.restrict(Digest::of(b"terms"), "elsewhere:1");
        let manifest = Manifest::new_media_set(
            "synthetic",
            open_set()[0].descriptor.clone(),
            open_set()[1..]
                .iter()
                .map(|c| c.descriptor.clone())
                .collect(),
        )
        .unwrap();
        let restricted = closure(&manifest, &local);
        for (case, set, blobs) in [
            ("absent", &absent, vec![disk(b"disk 1"), disk(b"disk 2")]),
            (
                "missing",
                &missing,
                vec![shareware(mt::FLOPPY, b"disk 2", None)],
            ),
            ("other", &other, vec![other[2].descriptor.clone()]),
        ] {
            for (private, reference) in [
                (false, "shareware:1"),
                (true, "shareware:1"),
                (false, "owner/shareware:1"),
            ] {
                let refusal = verdict(private, reference, set).unwrap_err();
                assert!(
                    matches!(&refusal, Refusal::TermsWithoutText { blobs: b, hint, .. }
                        if *b == blobs && hint.contains("under a new tag with --open")),
                    "{case} {reference}: {refusal}"
                );
                assert!(refusal.to_string().contains("terms text"), "{refusal}");
            }
            // The user's own copy may still go to a private registry.
            assert_eq!(verdict(true, "private/shareware:1", set), Ok(()), "{case}");
        }
        let refusal = verdict(true, "shareware:1", &restricted).unwrap_err();
        assert!(matches!(
            &refusal,
            Refusal::OfficialNamespaceNeedsOpen { blobs, .. }
                if blobs.len() == 1 && blobs[0].digest == Digest::of(b"terms")
        ));
        // Only removing the other tag's record clears it, not a new import.
        assert!(
            refusal.to_string().ends_with(
                "; another local tag records some of these bytes user-supplied \
                 (image push --dry-run names it): delete that tag in the library, \
                 or push from a store without it"
            ),
            "{refusal}"
        );
    }

    #[test]
    fn a_user_supplied_media_set_never_enters_the_official_namespace() {
        let set = [
            entry(Some("user-supplied"), mt::MEDIA_SET_CONFIG, b"set config"),
            entry(Some("user-supplied"), mt::FLOPPY, b"disk 1"),
            entry(Some("user-supplied"), mt::EXTRA, b"manual"),
            entry(Some("user-supplied"), mt::METADATA, b"set metadata"),
        ];
        for private in [false, true] {
            assert!(matches!(
                verdict(private, "shareware:1", &set),
                Err(Refusal::OfficialNamespaceNeedsOpen { ref blobs, hint, .. })
                    if blobs.len() == 4 && hint.contains("under a new tag with --open")
            ));
        }
        assert!(matches!(
            verdict(false, "owner/shareware:1", &set),
            Err(Refusal::RegistryNotPrivate { hint, .. }) if hint.contains("--license-text")
        ));
        assert_eq!(verdict(true, "private/shareware:1", &set), Ok(()));
    }

    #[test]
    fn a_machine_image_embedding_open_set_media_is_open_only_when_every_other_layer_is() {
        // Rights follow the bytes: the set's disk and terms text stay open in
        // the machine image, but its user-supplied ROM keeps the image private.
        let mixed = [
            entry(Some("open"), mt::CONFIG, b"machine config"),
            entry(Some("user-supplied"), mt::FIRMWARE, b"vendor rom"),
            classified(shareware(mt::FLOPPY, b"disk 1", Some(b"terms"))),
            classified(shareware(mt::EXTRA, b"terms", Some(b"terms"))),
            entry(Some("open"), mt::METADATA, b"machine metadata"),
        ];
        for private in [false, true] {
            assert!(matches!(
                verdict(private, "lab:1", &mixed),
                Err(Refusal::OfficialNamespaceNeedsOpen { ref blobs, hint: "", .. })
                    if blobs.len() == 1 && blobs[0].digest == Digest::of(b"vendor rom")
            ));
        }
        assert!(matches!(
            verdict(false, "owner/lab:1", &mixed),
            Err(Refusal::RegistryNotPrivate { .. })
        ));
        assert_eq!(verdict(true, "private/lab:1", &mixed), Ok(()));

        // With an open ROM the image is open, but only with the terms text.
        let mut open = mixed.clone();
        open[1] = entry(Some("open"), mt::FIRMWARE, b"open rom");
        assert_eq!(verdict(false, "lab:1", &open), Ok(()));
        let without: Vec<_> = open
            .iter()
            .filter(|c| c.descriptor.media_type != mt::EXTRA)
            .cloned()
            .collect();
        assert!(matches!(
            verdict(false, "lab:1", &without),
            Err(Refusal::TermsWithoutText { ref blobs, hint, .. })
                if blobs.len() == 1 && blobs[0].digest == Digest::of(b"disk 1")
                    && hint.contains("terms text of the set")
        ));
    }

    #[test]
    fn an_unconfigured_registry_is_refused() {
        let open = [entry(Some("open"), mt::CONFIG, b"c")];
        assert!(matches!(
            check(None, &target("other.example/lab:1"), &open),
            Err(Refusal::UnknownRegistry { .. })
        ));
    }

    /// The default registry with a public `unemu/`, a private `private/`
    /// and, when `games` is set, a public `games/`.
    fn namespaced(games: bool) -> RegistryEntry {
        let namespace = |name: &str, private| crate::config::Namespace {
            name: name.into(),
            private,
        };
        let mut namespaces = vec![namespace("unemu", false), namespace("private", true)];
        if games {
            namespaces.push(namespace("games", false));
        }
        RegistryEntry {
            namespaces,
            ..RegistryEntry::builtin("registry.unemu.dev")
        }
    }

    #[test]
    fn user_supplied_blobs_go_only_into_a_namespace_marked_private() {
        let mixed = [
            entry(Some("open"), mt::CONFIG, b"c"),
            entry(Some("user-supplied"), mt::FIRMWARE, b"rom"),
        ];
        let entry = namespaced(true);
        assert_eq!(
            check(Some(&entry), &target("private/lab:1"), &mixed),
            Ok(())
        );
        assert!(needs_private_probe(&mixed));
        assert!(matches!(
            check(Some(&entry), &target("lab:1"), &mixed),
            Err(Refusal::OfficialNamespaceNeedsOpen { .. })
        ));
        let public = check(Some(&entry), &target("games/lab:1"), &mixed).unwrap_err();
        assert!(
            matches!(public, Refusal::NamespaceNotPrivate { ref namespace, ref blobs, .. }
                if namespace == "games" && blobs.len() == 1),
            "{public:?}"
        );
        assert!(public
            .to_string()
            .starts_with("registry.unemu.dev/games/ is not marked private"));
    }

    #[test]
    fn an_entry_with_namespaces_refuses_every_push_outside_them() {
        let open = [entry(Some("open"), mt::CONFIG, b"c")];
        let mixed = [
            entry(Some("open"), mt::CONFIG, b"c"),
            entry(None, mt::FIRMWARE, b"rom"),
        ];
        let entry = namespaced(false);
        for closure in [&open[..], &mixed[..]] {
            let refusal = check(Some(&entry), &target("owner/lab:1"), closure).unwrap_err();
            assert_eq!(
                refusal.to_string(),
                "registry.unemu.dev/owner/ is not a configured namespace on this registry"
            );
            assert!(matches!(
                check(
                    Some(&entry),
                    &target("registry.unemu.dev/private:1"),
                    closure
                ),
                Err(Refusal::UnlistedNamespace {
                    namespace: None,
                    ..
                })
            ));
        }
        // Open content still goes to the listed namespaces.
        for reference in ["lab:1", "private/lab:1"] {
            assert_eq!(check(Some(&entry), &target(reference), &open), Ok(()));
        }
    }

    #[test]
    fn the_official_namespace_takes_open_content_only_whatever_the_entry_says() {
        // Preferences refuse this entry; one built in code still gets no exception.
        let entry = RegistryEntry {
            namespaces: vec![crate::config::Namespace {
                name: "unemu".into(),
                private: true,
            }],
            ..RegistryEntry::builtin("registry.unemu.dev")
        };
        assert!(!entry.scope("unemu/lab").private());
        let mixed = [entry_with_rom()];
        assert!(matches!(
            check(Some(&entry), &target("lab:1"), &mixed),
            Err(Refusal::OfficialNamespaceNeedsOpen { .. })
        ));
        let without = package("GPL-3.0-or-later", None);
        assert!(matches!(
            check(Some(&entry), &target("lab:1"), &without),
            Err(Refusal::CopyleftWithoutSource { .. })
        ));
    }

    fn entry_with_rom() -> ClosureEntry {
        entry(Some("user-supplied"), mt::FIRMWARE, b"rom")
    }

    #[test]
    fn a_copyleft_rom_without_source_stays_in_a_private_namespace() {
        let without = package("GPL-3.0-or-later", None);
        let entry = namespaced(true);
        assert_eq!(
            check(Some(&entry), &target("private/bios:1"), &without),
            Ok(())
        );
        for reference in ["bios:1", "games/bios:1"] {
            assert!(
                matches!(
                    check(Some(&entry), &target(reference), &without),
                    Err(Refusal::CopyleftWithoutSource { .. })
                ),
                "{reference}"
            );
        }
    }
}
