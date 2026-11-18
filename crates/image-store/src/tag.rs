// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tags of installation media sets and firmware packages: a release and a
//! revision; recipes.md §8.2 and machine-images §5.2.
//!
//! ```text
//! tag      = [ release "-" ] "r" N          the immutable name of one packaging
//! alias    = release                         follows the newest rN of that release
//! release  = part ( "-" part )*
//! part     = [a-z0-9]+ ( [._] [a-z0-9]+ )*   and never "r" followed only by digits
//! ```
//!
//! The release names the software (`1.4`, `1.4-720k`, `usa-rev1`, `2x32k`);
//! `rN`, from 1, counts the packagings of one release. A tag that ends in
//! `rN` names one set or package forever, in the local store and on a
//! registry. The release alone is an alias that moves forward to the newest
//! revision of its release, and to nothing else. Sets and packages have no
//! `latest`: across releases and formats "the newest" has no single answer.
//! The parts of a release are a convention of the author, not fields: no
//! program reads a version, an edition or a format out of a tag, and the
//! orders here only arrange tags for display and compare revisions.
//!
//! Machine images keep their own rule: semantic versions never move and
//! every other tag, `latest` included, is an alias that moves when confirmed
//! (machine-images §5.5); [`precedence`] orders their version tags.
//!
//! Tags written by earlier releases still read: a set tag such as `1.4` is
//! an alias of release `1.4`, `1.4-r2` its second revision, and a firmware
//! tag `1` an alias of release `1`. Nothing rewrites them.

use crate::media_type;
use std::cmp::Ordering;

/// The tag sets and packages never take.
pub const LATEST: &str = "latest";

/// What a tag of a set or package names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind<'a> {
    /// `[<release>-]r<N>`: one packaging, named forever.
    Revision {
        release: Option<&'a str>,
        number: u64,
    },
    /// `<release>`: follows the newest revision of its release.
    Alias { release: &'a str },
}

/// Whether tags of `artifact_type` follow this scheme: installation media
/// sets and firmware packages. Machine images keep semantic versions.
pub fn governs(artifact_type: &str) -> bool {
    matches!(artifact_type, media_type::MEDIA_SET | media_type::FIRMWARE)
}

/// `N` of `rN`: decimal from 1 without a leading zero.
fn revision_number(text: &str) -> Option<u64> {
    let digits = text.strip_prefix('r')?;
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn valid_part(part: &str) -> bool {
    let segments_ok = part.split(['.', '_']).all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    });
    let looks_like_revision = part
        .strip_prefix('r')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
    segments_ok && !looks_like_revision
}

/// Whether `release` is a release: parts joined by `-`, none of the form
/// `r` plus digits.
pub fn valid_release(release: &str) -> bool {
    !release.is_empty() && release.split('-').all(valid_part)
}

/// What `tag` names, or `None` for a tag outside the scheme.
pub fn parse(tag: &str) -> Option<Kind<'_>> {
    if let Some(number) = revision_number(tag) {
        return Some(Kind::Revision {
            release: None,
            number,
        });
    }
    if let Some((release, last)) = tag.rsplit_once('-') {
        if let Some(number) = revision_number(last) {
            return valid_release(release).then_some(Kind::Revision {
                release: Some(release),
                number,
            });
        }
    }
    valid_release(tag).then_some(Kind::Alias { release: tag })
}

/// Whether `tag` ends in `rN`, so it never moves.
pub fn is_revision(tag: &str) -> bool {
    matches!(parse(tag), Some(Kind::Revision { .. }))
}

/// The alias a revision tag moves: its release; `None` for a revision
/// without a release, which has no alias, and for any other tag.
pub fn alias_of(tag: &str) -> Option<&str> {
    match parse(tag)? {
        Kind::Revision { release, .. } => release,
        Kind::Alias { .. } => None,
    }
}

/// The revision tag `number` of `release`.
pub fn revision(release: Option<&str>, number: u64) -> String {
    match release {
        Some(release) => format!("{release}-r{number}"),
        None => format!("r{number}"),
    }
}

/// The first revision tag of `release`: `<release>-r1`; for a firmware
/// package (`package`) whose release is a plain number `N`, `rN`, since an
/// earlier release tagged a package's revisions `1`, `2`.
pub fn first_revision(release: &str, package: bool) -> String {
    match release.parse::<u64>() {
        Ok(number) if package && number > 0 && !release.starts_with('0') => revision(None, number),
        _ => revision(Some(release), 1),
    }
}

/// Why the local tag `name` (`repository:tag`) cannot name a new set or
/// package, which no tag names yet: only a revision tag can, and its release
/// alias follows it. `None` for a revision tag. `package` is a firmware
/// package, whose plain-number release suggests `rN`.
pub fn new_image_problem(name: &str, package: bool) -> Option<String> {
    let (repository, tag) = name.rsplit_once(':').unwrap_or(("", name));
    match parse(tag) {
        Some(Kind::Revision { .. }) => None,
        _ if tag == LATEST => Some(format!(
            "installation media sets and firmware packages have no {LATEST} tag; tag a revision such as {repository}:1.0-r1"
        )),
        Some(Kind::Alias { release }) => Some(format!(
            "{name} is the alias of release {release}, which names its newest revision; tag the image {repository}:{}, and the alias {name} follows",
            first_revision(release, package)
        )),
        None => Some(format!(
            "{name} is neither a revision tag nor a release alias; installation media sets and firmware packages are tagged [RELEASE-]rN, such as {repository}:1.0-r1, and the release alone follows its newest revision"
        )),
    }
}

/// The revision after a revision tag: `1.4-r1` gives `1.4-r2`. `None` for
/// any other tag.
pub fn next_revision(tag: &str) -> Option<String> {
    match parse(tag)? {
        Kind::Revision { release, number } => Some(revision(release, number.checked_add(1)?)),
        Kind::Alias { .. } => None,
    }
}

/// How two tags compare as revisions: two revisions of the same release by
/// their number. Any other pair makes no claim, `None`: an alias, a tag
/// outside the scheme, or revisions of two releases, since a newer release
/// is a different piece of software, not a newer packaging.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    match (parse(a)?, parse(b)?) {
        (
            Kind::Revision {
                release: x,
                number: m,
            },
            Kind::Revision {
                release: y,
                number: n,
            },
        ) if x == y => Some(m.cmp(&n)),
        _ => None,
    }
}

/// Segments of a part, split at `.` and `_`.
fn segments(part: &str) -> impl Iterator<Item = &str> + '_ {
    part.split(['.', '_'])
}

/// Two segments in ascending order: numbers by value and before words,
/// words alphabetically.
fn ascending(a: &str, b: &str) -> Ordering {
    // Digits compare by value at any length: without leading zeros, a
    // longer number is the larger one.
    fn number(s: &str) -> Option<&str> {
        is_number(s).then(|| s.trim_start_matches('0'))
    }
    match (number(a), number(b)) {
        (Some(x), Some(y)) => x.len().cmp(&y.len()).then_with(|| x.cmp(y)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

fn is_number(segment: &str) -> bool {
    segment.bytes().all(|b| b.is_ascii_digit())
}

/// The first part, which by convention is the version, newest first: higher
/// numbers first, words alphabetically after numbers, `1.4.1` before `1.4`
/// and `1.4` before `1.4_beta`.
fn first_part_newest_first(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (segments(a), segments(b));
    loop {
        match (x.next(), y.next()) {
            (Some(s), Some(t)) => {
                // Numbers newest first; words stay alphabetical.
                let order = match ascending(s, t) {
                    order if is_number(s) && is_number(t) => order.reverse(),
                    order => order,
                };
                if order.is_ne() {
                    return order;
                }
            }
            // A further number is a newer version (`1.4.1` before `1.4`); a
            // further word is a variant, after its release (`1.4` before
            // `1.4_beta`).
            (Some(s), None) if is_number(s) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (None, Some(t)) if is_number(t) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        }
    }
}

/// The remaining parts, which by convention are the edition, region,
/// language and format: the release without them first, then in ascending
/// order part by part.
fn other_parts(a: &[&str], b: &[&str]) -> Ordering {
    for (s, t) in a.iter().zip(b) {
        let order = segments(s)
            .zip(segments(t))
            .map(|(x, y)| ascending(x, y))
            .find(|o| o.is_ne())
            .unwrap_or_else(|| segments(s).count().cmp(&segments(t).count()));
        if order.is_ne() {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

fn releases_newest_first(a: Option<&str>, b: Option<&str>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => {
            let a: Vec<&str> = a.split('-').collect();
            let b: Vec<&str> = b.split('-').collect();
            first_part_newest_first(a[0], b[0]).then_with(|| other_parts(&a[1..], &b[1..]))
        }
    }
}

/// Newest first, to list the tags of one set or package repository: by
/// release (the first part's numbers highest first, then the release
/// without further parts before its variants, which follow in ascending
/// order), and within a release the alias first, as the name that stands for
/// the newest revision, then the revisions highest first. A revision without
/// a release comes before every release. `latest` and tags outside the
/// scheme come last, `latest` first, then by name. A total order, whatever
/// order a registry lists the tags in.
pub fn newest_first(a: &str, b: &str) -> Ordering {
    let key = |tag| match parse(tag) {
        Some(_) if tag == LATEST => None,
        Some(Kind::Revision { release, number }) => Some((release, Some(number))),
        Some(Kind::Alias { release }) => Some((Some(release), None)),
        None => None,
    };
    match (key(a), key(b)) {
        (Some((x, m)), Some((y, n))) => releases_newest_first(x, y)
            .then_with(|| match (m, n) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
                (Some(m), Some(n)) => n.cmp(&m),
            })
            .then_with(|| a.cmp(b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => (b == LATEST).cmp(&(a == LATEST)).then_with(|| a.cmp(b)),
    }
}

/// A version tag's precedence, Semantic Versioning 2.0.0 §11: the numeric
/// core, then a release above its pre-releases, whose dot-separated
/// identifiers compare numerically when numeric and as text otherwise,
/// numeric ones first and a shorter list first. Build metadata is ignored.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Precedence {
    core: [u64; 3],
    release: bool,
    pre: Vec<Identifier>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Identifier {
    Numeric(u64),
    Text(String),
}

impl Precedence {
    /// A release, not a pre-release.
    pub fn is_release(&self) -> bool {
        self.release
    }
}

/// The precedence of a version-shaped tag; `None` for any other tag.
pub fn precedence(tag: &str) -> Option<Precedence> {
    if !crate::Store::is_version_tag(tag) {
        return None;
    }
    let tag = tag.split('+').next()?;
    let (core, pre) = match tag.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (tag, None),
    };
    let mut numbers = core.split('.').map(|n| n.parse::<u64>().ok());
    let core = [numbers.next()??, numbers.next()??, numbers.next()??];
    let pre = pre
        .map(|pre| {
            pre.split('.')
                .map(|id| match id.parse::<u64>() {
                    Ok(n) if id.bytes().all(|b| b.is_ascii_digit()) => Identifier::Numeric(n),
                    _ => Identifier::Text(id.to_owned()),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Precedence {
        core,
        release: tag.split_once('-').is_none(),
        pre,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_and_aliases_follow_the_grammar() {
        for (tag, release, number) in [
            ("r1", None, 1),
            ("r12", None, 12),
            ("1.4-r1", Some("1.4"), 1),
            ("1.4-720k-r2", Some("1.4-720k"), 2),
            ("usa-rev1-r1", Some("usa-rev1"), 1),
            ("2x32k-r1", Some("2x32k"), 1),
            ("3.0-1.2m-r10", Some("3.0-1.2m"), 10),
            ("1.4_beta.2-r3", Some("1.4_beta.2"), 3),
        ] {
            assert_eq!(
                parse(tag),
                Some(Kind::Revision { release, number }),
                "{tag}"
            );
            assert!(is_revision(tag), "{tag}");
            assert_eq!(alias_of(tag), release, "{tag}");
        }
        for tag in [
            "1.4", "1.4-720k", "usa", "usa-rev1", "2x32k", "2026", "latest", "1", "r", "r2d2",
            "rev1",
        ] {
            assert_eq!(parse(tag), Some(Kind::Alias { release: tag }), "{tag}");
            assert!(!is_revision(tag), "{tag}");
        }
        for tag in [
            "",
            "r0",
            "r01",
            "1.4-r0",
            "1.4-r01",
            "r1-r2",
            "1.4-r1-r2",
            "1.4-",
            "-r1",
            "1..4",
            "1.4.",
            "_1",
            "V1.4",
            "1.4-RC1",
            "1.4+b",
            "1.0.0-rc.1-",
            "a--b",
        ] {
            assert_eq!(parse(tag), None, "{tag}");
        }
        assert!(!valid_release("1.4-r2"));
        assert!(valid_release("1.4-rev2"));
    }

    #[test]
    fn the_next_revision_counts_up_within_its_release() {
        assert_eq!(next_revision("r1").as_deref(), Some("r2"));
        assert_eq!(next_revision("1.4-r1").as_deref(), Some("1.4-r2"));
        assert_eq!(
            next_revision("1.4-720k-r9").as_deref(),
            Some("1.4-720k-r10")
        );
        assert_eq!(next_revision("1.4"), None);
        assert_eq!(next_revision("Latest"), None);
        assert_eq!(revision(Some("usa"), 3), "usa-r3");
        assert_eq!(revision(None, 3), "r3");
        // A package an earlier release tagged `1` takes `r1`; a set's `1`
        // is a version.
        assert_eq!(first_revision("1", true), "r1");
        assert_eq!(first_revision("1", false), "1-r1");
        assert_eq!(first_revision("1.0", true), "1.0-r1");
        assert_eq!(first_revision("01", true), "01-r1");
    }

    #[test]
    fn a_new_set_or_package_takes_a_revision_tag() {
        assert_eq!(new_image_problem("x:1.0-r1", false), None);
        assert_eq!(new_image_problem("x:r1", true), None);
        let alias = new_image_problem("x:1.0", false).unwrap();
        assert!(
            alias.contains("tag the image x:1.0-r1, and the alias x:1.0 follows"),
            "{alias}"
        );
        let legacy = new_image_problem("firmware/x:1", true).unwrap();
        assert!(legacy.contains("tag the image firmware/x:r1"), "{legacy}");
        assert!(new_image_problem("x:Beta", false)
            .unwrap()
            .contains("neither a revision tag nor a release alias"));
        assert!(new_image_problem("x:latest", false)
            .unwrap()
            .contains("no latest"));
    }

    #[test]
    fn only_revisions_of_one_release_compare() {
        use Ordering::*;
        for (a, b, order) in [
            ("1.4-r2", "1.4-r1", Some(Greater)),
            ("r10", "r9", Some(Greater)),
            ("1.4-720k-r1", "1.4-720k-r1", Some(Equal)),
            ("usa-rev1-r2", "usa-rev1-r3", Some(Less)),
            ("1.4-r1", "1.3-r2", None),
            ("1.4-720k-r1", "1.4-r1", None),
            ("1.4", "1.4-r1", None),
            ("latest", "r1", None),
            ("r1", "2x32k-r1", None),
            ("1", "r1", None),
        ] {
            assert_eq!(compare(a, b), order, "{a} vs {b}");
            assert_eq!(compare(b, a), order.map(Ordering::reverse), "{b} vs {a}");
        }
    }

    #[test]
    fn tags_list_newest_first_with_the_alias_before_its_revisions() {
        fn order<'a>(tags: &[&'a str]) -> Vec<&'a str> {
            let mut tags = tags.to_vec();
            tags.sort_by(|a, b| newest_first(a, b));
            tags
        }
        let expected = [
            "1.10-r1",
            "1.5",
            "1.5-r1",
            "1.4",
            "1.4-r2",
            "1.4-r1",
            "1.4-1.2m",
            "1.4-1.2m-r1",
            "1.4-720k",
            "1.4-720k-r2",
            "1.4-720k-r1",
            "1.4-registered-720k-r1",
            "europe-r1",
            "usa",
            "usa-r1",
            "usa-rev1-r1",
            "latest",
            "Beta",
            "x+y",
        ];
        let mut listed = expected.to_vec();
        assert_eq!(order(&listed), expected);
        listed.reverse();
        assert_eq!(order(&listed), expected);
        assert_eq!(order(&["r1", "r10", "r2"]), ["r10", "r2", "r1"]);
        assert_eq!(
            order(&["4x16k-r1", "2x32k-r1", "r1"]),
            ["r1", "2x32k-r1", "4x16k-r1"]
        );
        assert_eq!(order(&["1.4", "1.4.1-r1"]), ["1.4.1-r1", "1.4"]);
        // A release before its own variants.
        assert_eq!(
            order(&["1.4_beta-r1", "1.4-r1", "1.4_beta", "1.4"]),
            ["1.4", "1.4-r1", "1.4_beta", "1.4_beta-r1"]
        );
        // Firmware tags of earlier releases list beside the new ones.
        assert_eq!(order(&["1", "r2", "2"]), ["r2", "2", "1"]);
    }

    #[test]
    fn only_sets_and_packages_follow_the_scheme() {
        assert!(governs(media_type::MEDIA_SET));
        assert!(governs(media_type::FIRMWARE));
        assert!(!governs(media_type::MACHINE));
        assert!(!governs("application/vnd.example.other.v1"));
    }
}
