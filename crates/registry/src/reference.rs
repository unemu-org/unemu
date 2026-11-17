// SPDX-License-Identifier: AGPL-3.0-or-later
//! Image references with Docker-style resolution against a default registry.
//!
//! OCI Distribution Specification 1.1, "Pulling manifests": repository path
//! components match `[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*`, tags match
//! `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`. Host inference follows Docker's
//! reference grammar: a first component containing `.` or `:`, or equal to
//! `localhost`, names the registry. See `backlog/docs/design/machine-images.md`
//! §5.1 and the default-registry rules it cites.

use crate::{Error, Result};
use image_store::Digest;
use std::fmt;

/// The unemu family's default registry, in the role `docker.io` plays for Docker.
pub const DEFAULT_REGISTRY: &str = "registry.unemu.dev";
/// The official namespace that an unqualified single-component name expands into.
pub const OFFICIAL_NAMESPACE: &str = "unemu";

/// A fully qualified repository reference. A digest pins content and ignores the tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub host: String,
    pub repository: String,
    pub tag: Option<String>,
    pub digest: Option<Digest>,
}

impl Reference {
    /// Resolve `name[:tag][@digest]`, `owner/name...` or `host/path...` against
    /// `default_host`. A reference without tag or digest means `:latest`.
    pub fn parse(input: &str, default_host: &str) -> Result<Self> {
        let (rest, digest) = match input.split_once('@') {
            Some((rest, digest)) => (rest, Some(digest.parse::<Digest>().map_err(bad)?)),
            None => (input, None),
        };
        let (host, path) = split_host(rest, default_host)?;
        let (path, tag) = match path.rsplit_once(':') {
            Some((path, tag)) if !tag.contains('/') => (path, Some(tag.to_owned())),
            _ => (path, None),
        };
        let repository = if path.contains('/') || host.explicit {
            path.to_owned()
        } else {
            format!("{OFFICIAL_NAMESPACE}/{path}")
        };
        validate_repository(&repository)?;
        if let Some(tag) = &tag {
            validate_tag(tag)?;
        }
        let tag = match (&tag, &digest) {
            (None, None) => Some("latest".to_owned()),
            _ => tag,
        };
        Ok(Self {
            host: host.name,
            repository,
            tag,
            digest,
        })
    }

    /// Parse a repository without a tag or digest, for listing its tags.
    pub fn parse_repository(input: &str, default_host: &str) -> Result<(String, String)> {
        if input.contains('@') {
            return Err(bad("a repository name cannot carry a digest"));
        }
        let (host, path) = split_host(input, default_host)?;
        if path.contains(':') {
            return Err(bad("a repository name cannot carry a tag"));
        }
        let repository = if path.contains('/') || host.explicit {
            path.to_owned()
        } else {
            format!("{OFFICIAL_NAMESPACE}/{path}")
        };
        validate_repository(&repository)?;
        Ok((host.name, repository))
    }

    /// The manifest reference sent to the registry: the digest when pinned.
    pub fn manifest_reference(&self) -> String {
        match (&self.digest, &self.tag) {
            (Some(digest), _) => digest.to_string(),
            (None, Some(tag)) => tag.clone(),
            (None, None) => "latest".into(),
        }
    }

    /// The local tag a pull writes. From the family default registry
    /// ([`DEFAULT_REGISTRY`]) it keeps the short form: no host, and no
    /// official namespace when the remaining name is a single component
    /// (`open-at:1.2.0`). From any other registry, a configured default one
    /// included, it is the full reference with its host and port, Docker-style
    /// (`nas.example:5000/private/x:1`), so one path on two registries never
    /// shares a local name. A short form whose first component would read as
    /// a host (`a.b/c`) keeps the host too. Digest-only references have no
    /// local name.
    pub fn local_tag(&self) -> Option<String> {
        let tag = self.tag.as_ref()?;
        let short = format!("{}:{tag}", without_official_namespace(&self.repository));
        if self.host == DEFAULT_REGISTRY && image_store::Store::tag_host(&short).is_none() {
            return Some(short);
        }
        Some(format!("{}/{}:{tag}", self.host, self.repository))
    }

    /// The first path component, which owns access policy on shared registries.
    pub fn namespace(&self) -> &str {
        self.repository
            .split('/')
            .next()
            .unwrap_or(&self.repository)
    }
}

/// The family's short form of a local tag or repository: without its
/// registry host, and without the official namespace when one component
/// remains (`nas.example:5000/unemu/open-at:1.0.0` gives `open-at:1.0.0`).
/// It is the name a pull of the same path from the family default registry
/// writes, which is how official templates and the names suggested for
/// images made here are recognised whatever registry a tag came from.
pub fn short_name(name: &str) -> &str {
    without_official_namespace(image_store::Store::without_host(name))
}

/// `unemu/open-at` gives `open-at`; `unemu/media/freedos` keeps its namespace.
fn without_official_namespace(path: &str) -> &str {
    match path
        .strip_prefix(OFFICIAL_NAMESPACE)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        Some(rest) if !rest.contains('/') => rest,
        _ => path,
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.host, self.repository)?;
        if let Some(tag) = &self.tag {
            write!(f, ":{tag}")?;
        }
        if let Some(digest) = &self.digest {
            write!(f, "@{digest}")?;
        }
        Ok(())
    }
}

struct Host {
    name: String,
    explicit: bool,
}

fn split_host<'a>(input: &'a str, default_host: &str) -> Result<(Host, &'a str)> {
    if input.is_empty() {
        return Err(bad("empty reference"));
    }
    match input.split_once('/') {
        Some((first, rest)) if is_host(first) => {
            validate_host(first)?;
            Ok((
                Host {
                    name: first.to_ascii_lowercase(),
                    explicit: true,
                },
                rest,
            ))
        }
        _ => {
            validate_host(default_host)?;
            Ok((
                Host {
                    name: default_host.to_ascii_lowercase(),
                    explicit: false,
                },
                input,
            ))
        }
    }
}

fn is_host(component: &str) -> bool {
    component.contains('.') || component.contains(':') || component == "localhost"
}

/// Accept `name[:port]` or a bracketed IPv6 literal with an optional port,
/// in any case: the store's host rules after lowercasing
/// ([`image_store::Store::validate_registry_host`]), so a host that parses
/// here has exactly one local spelling.
pub(crate) fn validate_host(host: &str) -> Result<()> {
    image_store::Store::validate_registry_host(&host.to_ascii_lowercase()).map_err(|error| {
        bad(match error {
            image_store::Error::Invalid(why) => why,
            other => other.to_string(),
        })
    })
}

fn validate_repository(repository: &str) -> Result<()> {
    fn component(mut bytes: &[u8]) -> bool {
        let alnum = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
        loop {
            let count = bytes.iter().take_while(|b| alnum(b)).count();
            if count == 0 {
                return false;
            }
            bytes = &bytes[count..];
            if bytes.is_empty() {
                return true;
            }
            let separator = match bytes[0] {
                b'.' => 1,
                b'_' if bytes.get(1) == Some(&b'_') => 2,
                b'_' => 1,
                b'-' => bytes.iter().take_while(|&&b| b == b'-').count(),
                _ => return false,
            };
            bytes = &bytes[separator..];
        }
    }
    if repository.is_empty()
        || repository.len() > 255
        || !repository.split('/').all(|part| component(part.as_bytes()))
    {
        return Err(bad(format!("invalid repository name: {repository}")));
    }
    Ok(())
}

/// A namespace is one repository path component, the first one of the
/// repositories it holds.
pub(crate) fn validate_namespace(name: &str) -> Result<()> {
    if name.contains('/') {
        return Err(bad(format!("invalid namespace name: {name}")));
    }
    validate_repository(name)
}

pub(crate) fn validate_tag(tag: &str) -> Result<()> {
    if tag.is_empty()
        || tag.len() > 128
        || !tag.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_alphanumeric() || b == b'_' || (i > 0 && matches!(b, b'.' | b'-'))
        })
    {
        return Err(bad(format!("invalid tag: {tag}")));
    }
    Ok(())
}

/// Semantic Versioning 2.0.0 §§2, 9–10: tags of this shape are immutable once
/// they name an image. The store owns the rule; see [`image_store::Store::is_version_tag`].
pub fn is_version_tag(tag: &str) -> bool {
    image_store::Store::is_version_tag(tag)
}

fn bad(message: impl fmt::Display) -> Error {
    Error::Reference(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Reference {
        Reference::parse(input, DEFAULT_REGISTRY).unwrap()
    }

    #[test]
    fn short_names_resolve_into_the_official_namespace_of_the_default_registry() {
        let r = parse("freedos:1.0.0");
        assert_eq!(r.to_string(), "registry.unemu.dev/unemu/freedos:1.0.0");
        assert_eq!(r.local_tag().as_deref(), Some("freedos:1.0.0"));
        assert_eq!(parse("freedos").tag.as_deref(), Some("latest"));
        let owned = parse("private/games:1");
        assert_eq!(owned.to_string(), "registry.unemu.dev/private/games:1");
        assert_eq!(owned.local_tag().as_deref(), Some("private/games:1"));
        assert_eq!(owned.namespace(), "private");
    }

    #[test]
    fn a_first_component_with_a_dot_colon_or_localhost_is_a_host() {
        let r = parse("nas.example:5000/private/games:1");
        assert_eq!(r.host, "nas.example:5000");
        assert_eq!(r.repository, "private/games");
        assert_eq!(parse("localhost/lab").repository, "lab");
        assert_eq!(parse("127.0.0.1:5000/lab:2").host, "127.0.0.1:5000");
        let other_default = Reference::parse("lab:1", "nas.example").unwrap();
        assert_eq!(other_default.to_string(), "nas.example/unemu/lab:1");
    }

    #[test]
    fn local_tags_are_short_only_for_the_family_default_registry() {
        for (reference, local) in [
            ("open-at:1.2.0", "open-at:1.2.0"),
            ("unemu/media/freedos:1.4", "unemu/media/freedos:1.4"),
            ("private/games/dune2:1.0.0", "private/games/dune2:1.0.0"),
            ("registry.unemu.dev/private/x:1", "private/x:1"),
            (
                "registry.lan.example/private/games/dune2:1.0.0",
                "registry.lan.example/private/games/dune2:1.0.0",
            ),
            (
                "nas.example:5000/private/media/x:1.1",
                "nas.example:5000/private/media/x:1.1",
            ),
            ("localhost:5000/lab:2", "localhost:5000/lab:2"),
            ("127.0.0.1:5000/lab:2", "127.0.0.1:5000/lab:2"),
            ("NAS.example:5000/lab:2", "nas.example:5000/lab:2"),
        ] {
            let tag = parse(reference).local_tag().unwrap();
            assert_eq!(tag, local, "{reference}");
            image_store::Store::validate_tag_name(&tag).unwrap();
        }
        // A short form whose first component would read as a host keeps it.
        assert_eq!(
            parse("registry.unemu.dev/a.b/c:1").local_tag().as_deref(),
            Some("registry.unemu.dev/a.b/c:1")
        );
        // A configured default is a registry like any other; only the family
        // default keeps the short names.
        let other_default = Reference::parse("lab:1", "nas.example:5000").unwrap();
        assert_eq!(
            other_default.local_tag().as_deref(),
            Some("nas.example:5000/unemu/lab:1")
        );
    }

    #[test]
    fn short_names_drop_the_host_then_the_official_namespace() {
        for (name, short) in [
            ("nas.example:5000/unemu/open-at:1.0.0", "open-at:1.0.0"),
            ("nas.example:5000/unemu/open-at", "open-at"),
            (
                "registry.unemu.dev/unemu/media/freedos:1.4",
                "unemu/media/freedos:1.4",
            ),
            ("nas.example:5000/private/x:1", "private/x:1"),
            ("unemu/open-at:1.2.0", "open-at:1.2.0"),
            ("open-at:1.2.0", "open-at:1.2.0"),
            ("private/x", "private/x"),
        ] {
            assert_eq!(short_name(name), short, "{name}");
        }
    }

    /// Hosts are lowercased, and every other spelling of one registry is
    /// refused, so a reference names a registry one way.
    #[test]
    fn a_registry_host_parses_one_way() {
        assert_eq!(parse("NAS.Example:5000/x:1").host, "nas.example:5000");
        assert_eq!(parse("[FE80::1]:5000/x:1").host, "[fe80::1]:5000");
        let label = "a".repeat(64);
        for input in [
            "nas.example:05000/x:1",
            "nas.example:0/x:1",
            &format!("{label}.example/x:1"),
            "[:]/x:1",
            "[::1::2]:5000/x:1",
        ] {
            assert!(
                Reference::parse(input, DEFAULT_REGISTRY).is_err(),
                "{input}"
            );
        }
        assert!(Reference::parse("lab:1", "nas.example:05000").is_err());
    }

    #[test]
    fn digest_references_pin_content_and_have_no_local_name() {
        let digest = Digest::of(b"manifest");
        let r = parse(&format!("lab@{digest}"));
        assert_eq!(r.tag, None);
        assert_eq!(r.manifest_reference(), digest.to_string());
        assert_eq!(r.local_tag(), None);
        let both = parse(&format!("lab:1@{digest}"));
        assert_eq!(both.manifest_reference(), digest.to_string());
    }

    #[test]
    fn invalid_names_tags_hosts_and_digests_are_rejected() {
        for input in [
            "",
            "Lab:1",
            "lab:-1",
            "lab:",
            "a//b",
            "lab@sha256:short",
            "bad_host.:1/x",
            "host.example:99999/x",
            "lab:has space",
        ] {
            assert!(
                Reference::parse(input, DEFAULT_REGISTRY).is_err(),
                "{input}"
            );
        }
        assert!(Reference::parse("lab", "not a host").is_err());
    }

    #[test]
    fn repository_listing_names_follow_the_same_resolution() {
        assert_eq!(
            Reference::parse_repository("freedos", DEFAULT_REGISTRY).unwrap(),
            ("registry.unemu.dev".into(), "unemu/freedos".into())
        );
        assert!(Reference::parse_repository("freedos:1", DEFAULT_REGISTRY).is_err());
    }

    #[test]
    fn version_tags_have_the_shape_of_a_semantic_version() {
        for tag in [
            "1.0.0",
            "0.1.2",
            "10.20.30",
            "1.0.0-rc.1",
            "1.0.0-alpha-2",
            "2.0.0+b1",
        ] {
            assert!(is_version_tag(tag), "{tag}");
        }
        for tag in [
            "latest",
            "1.0",
            "1",
            "01.0.0",
            "1.0.0-",
            "1.0.0-01",
            "open-at:8mhz",
            "8mhz-1mb-2mbems",
            "v1.0.0",
        ] {
            assert!(!is_version_tag(tag), "{tag}");
        }
    }
}
