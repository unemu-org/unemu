// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the platform-neutral registry and Remotes code asks a platform, and
//! what the views read of the platforms without calling them
//! (`backlog/docs/design/core-contract.md` §8.1.4).
//!
//! The caller hands the platforms in, as the command line hands its
//! platforms to the neutral commands (§7.2); no neutral crate names one. A
//! question about one artifact goes to the first platform of the list.

use image_store::{ConfigValidator, Digest, Manifest, Store};
use unemu_core_api::PlatformDescription;

/// One platform's part of the desktop library: what the platform-neutral
/// registry and Remotes code asks about artifacts it cannot read itself.
///
/// The desktop's counterpart of the command line's `CliPlatform`
/// (core-contract.md §7.2): `validators`, `restricted` and `medium_format`
/// mean what they mean there. The caller hands the platforms in; no
/// neutral crate names one.
pub trait DesktopPlatform {
    /// The platform as its core describes it: the id artifacts name, in its
    /// current spelling, and the name people read. Ids are compared with
    /// `image_store::platform::same` only.
    fn description(&self) -> &'static PlatformDescription;

    /// Validators for every artifact type of the platform. Push and pull
    /// pick the one whose artifact type and platform match the manifest.
    fn validators(&self) -> Vec<&dyn ConfigValidator>;

    /// Digests of a manifest's blobs the platform knows must not be
    /// redistributed, whatever the image's own marks say: the input of the
    /// push guard, for the dry run and for the push.
    fn restricted(&self, store: &Store, manifest: &Manifest) -> image_store::Result<Vec<Digest>>;

    /// The name of a medium's format, from its table in a media set's
    /// config, as Remotes lists a set's media; `None` for a table the
    /// platform does not read.
    fn medium_format(&self, format: &toml::Table) -> Option<&'static str>;

    /// The platform's own payload media types with a name for each, as a
    /// transfer lists the blobs of an image. A type no platform names is
    /// shown by its media type.
    fn payload_labels(&self) -> &'static [(&'static str, &'static str)];
}

/// Every validator of `platforms`, in the order of the list.
pub fn validators<'p>(platforms: &[&'p dyn DesktopPlatform]) -> Vec<&'p dyn ConfigValidator> {
    platforms
        .iter()
        .flat_map(|platform| platform.validators())
        .collect()
}

/// The digests of `manifest` that some platform restricts, joined in
/// platform order: what the push guard counts as user-supplied whatever
/// the image's own marks say.
pub fn restricted(
    platforms: &[&dyn DesktopPlatform],
    store: &Store,
    manifest: &Manifest,
) -> image_store::Result<Vec<Digest>> {
    let mut digests = Vec::new();
    for platform in platforms {
        digests.extend(platform.restricted(store, manifest)?);
    }
    Ok(digests)
}

/// What the views and the registry models read of the compiled-in
/// platforms without calling them. Plain data: the frame makes it once and
/// the dialogs clone it, so nothing on the UI thread borrows a platform.
#[derive(Clone, Debug, Default)]
pub struct PlatformFacts {
    platforms: Vec<&'static PlatformDescription>,
    payload_labels: Vec<(&'static str, &'static str)>,
}

impl PlatformFacts {
    /// The facts of `platforms`, in list order: their descriptions and every
    /// payload label they state.
    pub fn of(platforms: &[&dyn DesktopPlatform]) -> Self {
        Self {
            platforms: platforms
                .iter()
                .map(|platform| platform.description())
                .collect(),
            payload_labels: platforms
                .iter()
                .flat_map(|platform| platform.payload_labels().iter().copied())
                .collect(),
        }
    }

    /// Whether a compiled-in platform runs artifacts that name `id`; a
    /// retired spelling of a platform's id counts as the platform.
    pub fn runs(&self, id: &str) -> bool {
        self.platforms
            .iter()
            .any(|platform| image_store::platform::same(id, platform.id))
    }

    /// The name of the machines this program runs, as "this release runs
    /// …s" says it: the first platform's name.
    pub fn family(&self) -> &str {
        self.platforms.first().map_or("", |platform| platform.name)
    }

    /// The name of a payload media type, when a platform states one.
    pub fn payload_label(&self, media_type: &str) -> Option<&'static str> {
        self.payload_labels
            .iter()
            .find(|(kind, _)| *kind == media_type)
            .map(|(_, label)| *label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A platform that states only its description, a payload label and the
    /// digests it restricts.
    struct Toy {
        description: &'static PlatformDescription,
        labels: &'static [(&'static str, &'static str)],
        restricts: &'static [u8],
    }

    impl DesktopPlatform for Toy {
        fn description(&self) -> &'static PlatformDescription {
            self.description
        }
        fn validators(&self) -> Vec<&dyn ConfigValidator> {
            Vec::new()
        }
        fn restricted(&self, _: &Store, _: &Manifest) -> image_store::Result<Vec<Digest>> {
            Ok(vec![Digest::of(self.restricts)])
        }
        fn medium_format(&self, _: &toml::Table) -> Option<&'static str> {
            None
        }
        fn payload_labels(&self) -> &'static [(&'static str, &'static str)] {
            self.labels
        }
    }

    static FIRST: PlatformDescription = PlatformDescription::new("toy-first", "Toy computer");
    static SECOND: PlatformDescription = PlatformDescription::new("toy-second", "Other toy");

    fn toys() -> [Toy; 2] {
        [
            Toy {
                description: &FIRST,
                labels: &[("application/vnd.example.first", "first payload")],
                restricts: b"first",
            },
            Toy {
                description: &SECOND,
                labels: &[("application/vnd.example.second", "second payload")],
                restricts: b"second",
            },
        ]
    }

    /// The facts answer for every platform handed in; the family is the
    /// first platform's name, and no platform means none of either.
    #[test]
    fn facts_read_every_platform_and_name_the_first() {
        let [first, second] = toys();
        let facts = PlatformFacts::of(&[&first, &second]);
        assert!(facts.runs("toy-first") && facts.runs("toy-second"));
        assert!(!facts.runs("toy-third"));
        assert_eq!(facts.family(), "Toy computer");
        assert_eq!(
            facts.payload_label("application/vnd.example.second"),
            Some("second payload")
        );
        assert_eq!(facts.payload_label("application/vnd.example.third"), None);

        let none = PlatformFacts::default();
        assert!(!none.runs("toy-first"));
        assert_eq!(none.family(), "");
    }

    /// Every platform's restrictions count, in the order of the list.
    #[test]
    fn restrictions_of_every_platform_are_joined_in_order() {
        let temp = image_store::test_support::TempDir::new("platform-restricted");
        let store = Store::open(temp.path().join("store")).unwrap();
        let blob = |media_type: &str, bytes: &[u8]| {
            image_store::Descriptor::new(media_type, Digest::of(bytes), bytes.len() as u64)
        };
        let manifest = Manifest::new(
            "toy-first",
            blob(image_store::media_type::CONFIG, b"config"),
            vec![blob(image_store::media_type::METADATA, b"metadata")],
        )
        .unwrap();
        let [first, second] = toys();
        assert_eq!(
            restricted(&[&first, &second], &store, &manifest).unwrap(),
            [Digest::of(b"first"), Digest::of(b"second")]
        );
        assert!(restricted(&[], &store, &manifest).unwrap().is_empty());
    }
}
