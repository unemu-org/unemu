// SPDX-License-Identifier: AGPL-3.0-or-later
//! SPDX licence expressions and the redistribution marks on descriptors;
//! machine-images §5, §5.2 and §5.3.
//!
//! SPDX Specification 2.3, Annex D "SPDX License Expressions": licence
//! identifiers combine with `AND`, `OR`, `WITH` (an exception) and
//! parentheses; `LicenseRef-<idstring>` names a licence outside the SPDX
//! License List. Operators are upper case, identifiers keep their canonical
//! spelling, and the deprecated `+` suffix and deprecated identifiers (such as
//! `GPL-2.0`) are refused, so one licence has one stored spelling.
//!
//! A descriptor counts as `open` only when it carries both
//! `unemu.redistribution = "open"` and a valid expression in `unemu.license`.
//! A missing, unknown or unparsable licence means user-supplied: the safe
//! direction for packages written before licences were recorded.

use crate::{invalid, Descriptor, Result};

/// Descriptor annotation with the redistribution mark, `open` or `user-supplied`.
pub const REDISTRIBUTION: &str = "unemu.redistribution";
/// Descriptor annotation with the SPDX licence expression, next to the mark.
pub const LICENSE: &str = "unemu.license";
/// ROM, medium or installation-set extra annotation naming the decoded digest
/// of its corresponding source archive, which travels in the same closure.
pub const SOURCE: &str = "unemu.source";
/// Installation-media descriptor annotation naming the decoded digest of the
/// vendor's terms text the bytes are redistributed under. The text is an
/// extra of the same set and travels in the same closure.
pub const TERMS: &str = "unemu.terms";
/// Mark values.
pub const OPEN: &str = "open";
pub const USER_SUPPLIED: &str = "user-supplied";

/// Licence rules that are policy choices. [`ACTIVE`] selects the rules the
/// import and the push guard apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// A ROM the firmware catalogue knows as open copyleft firmware is open
    /// only together with its open source archive, as a user-marked ROM is;
    /// without one it is imported user-supplied.
    pub catalogue_copyleft_needs_source: bool,
    /// Any copyleft identifier asks for the source, even one alternative of
    /// an `OR` that also offers a permissive licence.
    pub any_copyleft_alternative: bool,
}
impl Policy {
    /// The first accepted rules: every copyleft ROM, catalogue-known or not,
    /// is open only with its source archive, and an expression needs the
    /// source only when every choice it offers is copyleft.
    pub const ACCEPTED: Self = Self {
        catalogue_copyleft_needs_source: true,
        any_copyleft_alternative: false,
    };
    /// The [`Self::ACCEPTED`] rules, and any copyleft identifier anywhere in
    /// an expression asks for the source, even after `OR`.
    pub const STRICT: Self = Self {
        catalogue_copyleft_needs_source: true,
        any_copyleft_alternative: true,
    };
}
/// The licence rules in force: the conservative reading the maintainer
/// approved with the Firmware collection design. Switch back to
/// [`Policy::ACCEPTED`] here to let a permissive `OR` alternative stand alone.
pub const ACTIVE: Policy = Policy::STRICT;

/// Identifiers from the SPDX License List that firmware, documents and
/// source archives commonly carry. Any other licence is written as
/// `LicenseRef-<idstring>`.
const LICENSES: &[&str] = &[
    "0BSD",
    "AFL-3.0",
    "AGPL-1.0-only",
    "AGPL-1.0-or-later",
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "Apache-1.1",
    "Apache-2.0",
    "Artistic-1.0",
    "Artistic-2.0",
    "BSD-1-Clause",
    "BSD-2-Clause",
    "BSD-2-Clause-Patent",
    "BSD-3-Clause",
    "BSD-3-Clause-Clear",
    "BSD-4-Clause",
    "BSL-1.0",
    "CC-BY-3.0",
    "CC-BY-4.0",
    "CC-BY-SA-3.0",
    "CC-BY-SA-4.0",
    "CC0-1.0",
    "CDDL-1.0",
    "CDDL-1.1",
    "EPL-1.0",
    "EPL-2.0",
    "EUPL-1.1",
    "EUPL-1.2",
    "GFDL-1.1-only",
    "GFDL-1.1-or-later",
    "GFDL-1.2-only",
    "GFDL-1.2-or-later",
    "GFDL-1.3-only",
    "GFDL-1.3-or-later",
    "GPL-1.0-only",
    "GPL-1.0-or-later",
    "GPL-2.0-only",
    "GPL-2.0-or-later",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "Info-ZIP",
    "ISC",
    "LGPL-2.0-only",
    "LGPL-2.0-or-later",
    "LGPL-2.1-only",
    "LGPL-2.1-or-later",
    "LGPL-3.0-only",
    "LGPL-3.0-or-later",
    "MIT",
    "MIT-0",
    "MPL-1.0",
    "MPL-1.1",
    "MPL-2.0",
    "MPL-2.0-no-copyleft-exception",
    "NCSA",
    "OFL-1.1",
    "Unlicense",
    "UPL-1.0",
    "Watcom-1.0",
    "WTFPL",
    "X11",
    "Zlib",
];
/// Exception identifiers from the SPDX License Exceptions list.
const EXCEPTIONS: &[&str] = &[
    "Autoconf-exception-2.0",
    "Autoconf-exception-3.0",
    "Bison-exception-2.2",
    "Classpath-exception-2.0",
    "eCos-exception-2.0",
    "Font-exception-2.0",
    "GCC-exception-2.0",
    "GCC-exception-3.1",
    "LLVM-exception",
    "Linux-syscall-note",
];
/// The copyleft families the firmware decision names: GPL, LGPL, AGPL, MPL.
const COPYLEFT_PREFIXES: &[&str] = &["GPL-", "LGPL-", "AGPL-", "MPL-"];
const MAX_LENGTH: usize = 256;

/// The licence identifiers the parser accepts besides `LicenseRef-…`, in
/// SPDX License List order, for a frontend that offers them as choices.
pub fn identifiers() -> &'static [&'static str] {
    LICENSES
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    License(String),
    /// A licence and its exception.
    With(String, String),
    And(Vec<Node>),
    Or(Vec<Node>),
}
impl Node {
    /// Canonical text: single spaces, and parentheses only around an `OR`
    /// inside an `AND`, the one place precedence needs them.
    fn render(&self, out: &mut String) {
        match self {
            Self::License(id) => out.push_str(id),
            Self::With(id, exception) => {
                out.push_str(id);
                out.push_str(" WITH ");
                out.push_str(exception);
            }
            Self::And(nodes) | Self::Or(nodes) => {
                let and = matches!(self, Self::And(_));
                for (i, node) in nodes.iter().enumerate() {
                    if i > 0 {
                        out.push_str(if and { " AND " } else { " OR " });
                    }
                    let group = and && matches!(node, Self::Or(_));
                    if group {
                        out.push('(');
                    }
                    node.render(out);
                    if group {
                        out.push(')');
                    }
                }
            }
        }
    }
    /// One operator's operands, with nested operands of the same operator
    /// lifted into it, so `(A AND B) AND C` and `A AND B AND C` are one tree.
    fn join(nodes: Vec<Self>, and: bool) -> Self {
        let mut flat = Vec::new();
        for node in nodes {
            match node {
                Self::And(inner) if and => flat.extend(inner),
                Self::Or(inner) if !and => flat.extend(inner),
                other => flat.push(other),
            }
        }
        match (flat.len(), and) {
            (1, _) => flat.remove(0),
            (_, true) => Self::And(flat),
            (_, false) => Self::Or(flat),
        }
    }
}

/// A validated expression with its canonical spelling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expression {
    canonical: String,
    node: Node,
}
impl Expression {
    /// Parse and validate; whitespace is normalized to single spaces. The
    /// canonical spelling of one statement is at most 256 bytes, so a stored
    /// licence always parses again.
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > 4 * MAX_LENGTH {
            return Err(invalid("SPDX licence expression is too long"));
        }
        let tokens = tokens(text);
        let mut parser = Parser {
            tokens: &tokens,
            at: 0,
        };
        let node = parser.or()?;
        if parser.at != tokens.len() {
            return Err(invalid(format!("invalid SPDX licence expression {text:?}")));
        }
        let expression = Self::from_node(node);
        if expression.canonical.len() > MAX_LENGTH {
            return Err(invalid("SPDX licence expression is too long"));
        }
        Ok(expression)
    }
    fn from_node(node: Node) -> Self {
        let mut canonical = String::new();
        node.render(&mut canonical);
        Self { canonical, node }
    }
    /// The conjunction of several expressions, each distinct operand once, in
    /// first-use order. Built from the parsed trees, so the result has no
    /// length limit: it describes a whole package, not one statement.
    pub fn all<'a>(expressions: impl IntoIterator<Item = &'a Expression>) -> Option<Self> {
        let mut operands: Vec<Node> = Vec::new();
        for expression in expressions {
            let parts = match &expression.node {
                Node::And(nodes) => nodes.clone(),
                node => vec![node.clone()],
            };
            for part in parts {
                if !operands.contains(&part) {
                    operands.push(part);
                }
            }
        }
        (!operands.is_empty()).then(|| Self::from_node(Node::join(operands, true)))
    }
    pub fn as_str(&self) -> &str {
        &self.canonical
    }
    /// Whether the expression is one `LicenseRef-<idstring>`: a licence named
    /// by its own text rather than by the SPDX License List.
    pub fn is_reference(&self) -> bool {
        matches!(&self.node, Node::License(id) if id.starts_with("LicenseRef-"))
    }
    /// Whether distributing under this expression obliges the corresponding
    /// source: under [`Policy::STRICT`] when any identifier is a GPL, LGPL,
    /// AGPL or MPL family licence, under [`Policy::ACCEPTED`] only when every
    /// choice it offers includes one.
    pub fn needs_source(&self, policy: Policy) -> bool {
        fn walk(node: &Node, any: bool) -> bool {
            match node {
                Node::License(id) | Node::With(id, _) => {
                    COPYLEFT_PREFIXES.iter().any(|p| id.starts_with(p))
                }
                Node::And(nodes) => nodes.iter().any(|n| walk(n, any)),
                Node::Or(nodes) if any => nodes.iter().any(|n| walk(n, any)),
                Node::Or(nodes) => nodes.iter().all(|n| walk(n, any)),
            }
        }
        walk(&self.node, policy.any_copyleft_alternative)
    }
}
impl std::fmt::Display for Expression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.canonical)
    }
}

fn tokens(text: &str) -> Vec<String> {
    text.replace('(', " ( ")
        .replace(')', " ) ")
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}
struct Parser<'a> {
    tokens: &'a [String],
    at: usize,
}
impl Parser<'_> {
    fn next(&mut self) -> Option<&str> {
        let token = self.tokens.get(self.at)?;
        self.at += 1;
        Some(token)
    }
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.at).map(String::as_str)
    }
    fn or(&mut self) -> Result<Node> {
        let mut nodes = vec![self.and()?];
        while self.peek() == Some("OR") {
            self.at += 1;
            nodes.push(self.and()?);
        }
        Ok(Node::join(nodes, false))
    }
    fn and(&mut self) -> Result<Node> {
        let mut nodes = vec![self.term()?];
        while self.peek() == Some("AND") {
            self.at += 1;
            nodes.push(self.term()?);
        }
        Ok(Node::join(nodes, true))
    }
    fn term(&mut self) -> Result<Node> {
        match self.next() {
            Some("(") => {
                let node = self.or()?;
                if self.next() != Some(")") {
                    return Err(invalid("unbalanced parentheses in SPDX licence expression"));
                }
                Ok(node)
            }
            Some(id) if valid_license(id) => {
                let id = id.to_owned();
                if self.peek() == Some("WITH") {
                    self.at += 1;
                    match self.next() {
                        Some(exception) if EXCEPTIONS.contains(&exception) => {
                            Ok(Node::With(id, exception.to_owned()))
                        }
                        _ => Err(invalid("unknown SPDX licence exception")),
                    }
                } else {
                    Ok(Node::License(id))
                }
            }
            Some(id) => Err(invalid(format!(
                "unknown SPDX licence identifier {id:?}; use the canonical SPDX id or LicenseRef-<name>"
            ))),
            None => Err(invalid("incomplete SPDX licence expression")),
        }
    }
}
fn valid_license(id: &str) -> bool {
    LICENSES.contains(&id)
        || id.strip_prefix("LicenseRef-").is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        })
}

/// The licence of a descriptor marked open. A user-supplied mark, a missing
/// or invalid licence, or an open mark without one all mean user-supplied.
pub fn open_license(descriptor: &Descriptor) -> Option<Expression> {
    if descriptor
        .annotations
        .get(REDISTRIBUTION)
        .map(String::as_str)
        != Some(OPEN)
    {
        return None;
    }
    Expression::parse(descriptor.annotations.get(LICENSE)?).ok()
}
/// Mark a descriptor open under `license`, or user-supplied without one.
/// A user-supplied descriptor keeps no licence annotation.
pub fn mark(descriptor: &mut Descriptor, license: Option<&Expression>) {
    match license {
        Some(license) => {
            descriptor
                .annotations
                .insert(REDISTRIBUTION.into(), OPEN.into());
            descriptor
                .annotations
                .insert(LICENSE.into(), license.as_str().into());
        }
        None => {
            descriptor
                .annotations
                .insert(REDISTRIBUTION.into(), USER_SUPPLIED.into());
            descriptor.annotations.remove(LICENSE);
        }
    }
}

/// Push-guard rule 4 for one layer, for any frontend: an open ROM, medium or
/// installation-set extra whose licence obliges the corresponding source
/// (under [`ACTIVE`]) and whose linked source archive (`unemu.source`) is not
/// in `closure`. A ROM's archive is a tar or zip layer, as a firmware package
/// stores it; a medium's or an extra's may also be an extra, the form an
/// installation set stores its source archive in. An extra is checked when
/// it links a source archive, or when `closure` holds an installation set's
/// config descriptor, which the set rule needs: pass the config together with
/// the layers. An unlinked extra elsewhere is a document that travels beside a
/// firmware package's ROMs or a machine's media and is not checked. Such a
/// layer may stay only in a private registry outside `unemu/`.
pub fn source_missing(layer: &Descriptor, closure: &[Descriptor]) -> bool {
    use crate::media_type as mt;
    let media_set = closure.iter().any(|d| d.media_type == mt::MEDIA_SET_CONFIG);
    let linked = layer.annotations.contains_key(SOURCE);
    let archives: &[&str] = match layer.decoded_media_type() {
        mt::FIRMWARE => &[mt::TAR, mt::ZIP],
        mt::FLOPPY | mt::OPTICAL => &[mt::TAR, mt::ZIP, mt::EXTRA],
        mt::EXTRA if media_set || linked => &[mt::TAR, mt::ZIP, mt::EXTRA],
        _ => return false,
    };
    if !open_license(layer).is_some_and(|l| l.needs_source(ACTIVE)) {
        return false;
    }
    let link = layer
        .annotations
        .get(SOURCE)
        .and_then(|s| s.parse::<crate::Digest>().ok());
    !link.is_some_and(|link| {
        closure
            .iter()
            .any(|d| d.names(&link) && archives.contains(&d.decoded_media_type()))
    })
}

/// Push-guard rule 5, for any frontend: the open layers of `closure` whose
/// vendor terms text does not travel with them. An open layer that links a
/// terms text (`unemu.terms`) needs it as an open extra layer of `closure`.
/// In an installation media set every open layer except the generated
/// metadata must link one, and all of them the same one: the set's terms
/// text, which the first linking layer in manifest order names. A missing or
/// unparsable link, a link to another digest, or a text that is absent, not
/// an extra or not open refuses the layer, which may then stay only in a
/// private registry outside `unemu/`.
pub fn terms_unlinked(closure: &[Descriptor]) -> Vec<Descriptor> {
    use crate::media_type as mt;
    let media_set = closure.iter().any(|d| d.media_type == mt::MEDIA_SET_CONFIG);
    // `None` without the annotation, `Some(None)` when it does not parse.
    let link = |d: &Descriptor| {
        d.annotations
            .get(TERMS)
            .map(|s| s.parse::<crate::Digest>().ok())
    };
    let bound = |d: &Descriptor| {
        media_set && d.media_type != mt::MEDIA_SET_CONFIG && d.decoded_media_type() != mt::METADATA
    };
    let open: Vec<&Descriptor> = closure
        .iter()
        .filter(|d| open_license(d).is_some())
        .collect();
    let set_terms = open
        .iter()
        .filter(|d| bound(d))
        .find_map(|d| link(d).flatten());
    let travels = |text: &crate::Digest| {
        closure.iter().any(|d| {
            d.names(text) && d.decoded_media_type() == mt::EXTRA && open_license(d).is_some()
        })
    };
    open.into_iter()
        .filter(|d| match link(d) {
            None => bound(d),
            Some(None) => true,
            Some(Some(text)) => !travels(&text) || (bound(d) && set_terms.as_ref() != Some(&text)),
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{media_type, Digest};

    #[test]
    fn spdx_expressions_parse_to_one_canonical_spelling() {
        for (text, canonical) in [
            ("MIT", "MIT"),
            ("GPL-3.0-or-later", "GPL-3.0-or-later"),
            ("( MIT  OR GPL-2.0-only )", "MIT OR GPL-2.0-only"),
            ("((MIT))", "MIT"),
            ("(MIT AND Zlib) AND ISC", "MIT AND Zlib AND ISC"),
            ("MIT OR (Zlib AND ISC)", "MIT OR Zlib AND ISC"),
            ("(MIT OR Zlib) OR ISC", "MIT OR Zlib OR ISC"),
            (
                "GPL-2.0-only WITH Classpath-exception-2.0",
                "GPL-2.0-only WITH Classpath-exception-2.0",
            ),
            (
                "LicenseRef-acme-1.0 AND CC0-1.0",
                "LicenseRef-acme-1.0 AND CC0-1.0",
            ),
            (
                "(LGPL-2.1-or-later OR MIT) AND Zlib",
                "(LGPL-2.1-or-later OR MIT) AND Zlib",
            ),
        ] {
            assert_eq!(
                Expression::parse(text).unwrap().as_str(),
                canonical,
                "{text}"
            );
        }
        for text in [
            "",
            "MIT OR",
            "Foo-1.0",
            "(MIT",
            "()",
            "MIT)",
            "mit",
            "MIT or Zlib",
            "GPL-2.0",
            "GPL-2.0+",
            "GPL-2.0-only WITH Unknown-exception",
            "LicenseRef-",
            "LicenseRef-a_b",
            &format!("{}MIT", "MIT AND ".repeat(40)),
        ] {
            assert!(Expression::parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn every_offered_identifier_parses_as_itself() {
        assert!(identifiers().len() > 40);
        for id in identifiers() {
            assert_eq!(Expression::parse(id).unwrap().as_str(), *id);
        }
        for copyleft in [
            "GPL-3.0-or-later",
            "LGPL-2.1-only",
            "AGPL-3.0-only",
            "MPL-2.0",
        ] {
            assert!(identifiers().contains(&copyleft), "{copyleft}");
        }
    }

    #[test]
    fn copyleft_families_need_the_source_and_the_strict_policy_counts_any_alternative() {
        let needs = |text: &str, policy| Expression::parse(text).unwrap().needs_source(policy);
        for text in [
            "GPL-2.0-only",
            "GPL-3.0-or-later",
            "LGPL-2.1-or-later",
            "AGPL-3.0-only",
            "MPL-2.0",
            "MIT AND GPL-2.0-only",
            "GPL-2.0-only WITH Classpath-exception-2.0",
            "GPL-2.0-only OR LGPL-2.1-only",
        ] {
            assert!(needs(text, Policy::ACCEPTED), "{text}");
            assert!(needs(text, Policy::STRICT), "{text}");
        }
        for text in ["MIT", "CC0-1.0", "LicenseRef-vendor", "EUPL-1.2"] {
            assert!(!needs(text, Policy::ACCEPTED), "{text}");
            assert!(!needs(text, Policy::STRICT), "{text}");
        }
        assert!(!needs("MIT OR GPL-2.0-or-later", Policy::ACCEPTED));
        assert!(needs("MIT OR GPL-2.0-or-later", Policy::STRICT));
        assert_eq!(ACTIVE, Policy::STRICT);
        const { assert!(ACTIVE.catalogue_copyleft_needs_source) };
    }

    #[test]
    fn a_conjunction_is_built_from_the_trees_without_the_statement_length_limit() {
        let long =
            |n: usize| Expression::parse(&format!("LicenseRef-{}-{n}", "x".repeat(200))).unwrap();
        let parts = [
            long(1),
            Expression::parse("MIT OR Zlib").unwrap(),
            long(2),
            long(1),
            Expression::parse("MIT AND CC0-1.0").unwrap(),
        ];
        let all = Expression::all(&parts).unwrap();
        assert!(all.as_str().len() > MAX_LENGTH);
        assert_eq!(
            all.as_str(),
            format!(
                "{} AND (MIT OR Zlib) AND {} AND MIT AND CC0-1.0",
                long(1),
                long(2)
            )
        );
        assert_eq!(
            Expression::all(&parts[1..2]).unwrap().as_str(),
            "MIT OR Zlib"
        );
        assert!(Expression::all(&[]).is_none());
    }

    #[test]
    fn rule_four_needs_the_linked_archive_in_the_closure() {
        let archive = Descriptor::new(media_type::TAR, Digest::of(b"src"), 3);
        let mut rom = Descriptor::new(media_type::FIRMWARE, Digest::of(b"rom"), 3);
        mark(&mut rom, Some(&Expression::parse("GPL-2.0-only").unwrap()));
        assert!(source_missing(&rom, std::slice::from_ref(&archive)));
        rom.annotations
            .insert(SOURCE.into(), archive.digest.to_string());
        assert!(!source_missing(&rom, std::slice::from_ref(&archive)));
        assert!(source_missing(&rom, &[]));
        mark(&mut rom, Some(&Expression::parse("MIT").unwrap()));
        assert!(!source_missing(&rom, &[]));
        mark(&mut rom, None);
        assert!(
            !source_missing(&rom, &[]),
            "a user-supplied ROM is rules 1 and 2"
        );
    }

    #[test]
    fn rule_four_needs_a_copyleft_set_medium_and_extra_to_link_a_source_that_travels() {
        let gpl = Expression::parse("GPL-2.0-only AND MIT").unwrap();
        let layer = |kind: &str, bytes: &[u8], link: Option<&[u8]>| {
            let mut d = Descriptor::new(kind, Digest::of(bytes), bytes.len() as u64);
            mark(&mut d, Some(&gpl));
            if let Some(link) = link {
                d.annotations
                    .insert(SOURCE.into(), Digest::of(link).to_string());
            }
            d
        };
        let config = Descriptor::new(media_type::MEDIA_SET_CONFIG, Digest::of(b"config"), 6);
        let disk = layer(media_type::FLOPPY, b"disk", Some(b"src"));
        let iso = layer(media_type::OPTICAL, b"iso", Some(b"src"));
        let manual = layer(media_type::EXTRA, b"manual", Some(b"src"));
        let source = layer(media_type::EXTRA, b"src", Some(b"src"));
        let set = vec![
            config.clone(),
            disk.clone(),
            iso.clone(),
            manual.clone(),
            source.clone(),
        ];
        for d in &set {
            assert!(!source_missing(d, &set), "{}", d.media_type);
        }
        // The source archive left out, or a link to bytes that are not in
        // the closure, refuses every linking medium and extra.
        let without: Vec<_> = set[..4].to_vec();
        for d in &without[1..] {
            assert!(source_missing(d, &without), "{}", d.media_type);
        }
        let elsewhere = layer(media_type::FLOPPY, b"disk", Some(b"other"));
        assert!(source_missing(&elsewhere, &set));
        // No link at all, or a link that does not parse.
        let mut unlinked = disk.clone();
        unlinked.annotations.remove(SOURCE);
        assert!(source_missing(&unlinked, &set));
        unlinked
            .annotations
            .insert(SOURCE.into(), "not a digest".into());
        assert!(source_missing(&unlinked, &set));
        // A tar or zip layer is a source archive for media as for ROMs.
        let tar = Descriptor::new(media_type::TAR, Digest::of(b"src"), 3);
        assert!(!source_missing(&disk, &[config.clone(), disk.clone(), tar]));
        // A licence without copyleft needs no source, and neither does a
        // user-supplied layer, which rules 1 and 2 keep private.
        let mut permissive = unlinked.clone();
        mark(&mut permissive, Some(&Expression::parse("MIT").unwrap()));
        assert!(!source_missing(&permissive, &without));
        mark(&mut permissive, None);
        assert!(!source_missing(&permissive, &without));
        // Outside an installation set a medium still needs its source, and
        // so does an extra that links one; an unlinked extra is a document
        // beside a firmware package's ROMs.
        let machine = vec![disk.clone(), manual.clone()];
        assert!(source_missing(&disk, &machine));
        assert!(source_missing(&manual, &machine), "a linked extra");
        assert!(!source_missing(&disk, &[disk.clone(), source.clone()]));
        assert!(!source_missing(&manual, &[manual.clone(), source.clone()]));
        let mut document = manual.clone();
        document.annotations.remove(SOURCE);
        assert!(!source_missing(&document, &machine));
        assert!(source_missing(
            &document,
            &[config.clone(), document.clone()]
        ));
        // A ROM's archive stays a tar or zip layer: an extra is not one.
        let rom = layer(media_type::FIRMWARE, b"rom", Some(b"src"));
        assert!(source_missing(&rom, &set));
        assert!(!source_missing(
            &rom,
            &[Descriptor::new(media_type::ZIP, Digest::of(b"src"), 3)]
        ));
    }

    #[test]
    fn only_an_open_mark_with_a_valid_licence_is_open() {
        let mut d = Descriptor::new(media_type::FIRMWARE, Digest::of(b"rom"), 3);
        assert!(open_license(&d).is_none());
        d.annotations.insert(REDISTRIBUTION.into(), OPEN.into());
        assert!(open_license(&d).is_none(), "open without a licence");
        d.annotations.insert(LICENSE.into(), "GPL-2.0".into());
        assert!(open_license(&d).is_none(), "deprecated identifier");
        let gpl = Expression::parse("GPL-2.0-only").unwrap();
        mark(&mut d, Some(&gpl));
        assert_eq!(open_license(&d), Some(gpl));
        mark(&mut d, None);
        assert_eq!(d.annotations[REDISTRIBUTION], USER_SUPPLIED);
        assert!(!d.annotations.contains_key(LICENSE));
        assert!(open_license(&d).is_none());
    }

    #[test]
    fn only_a_single_licence_reference_is_a_reference() {
        for text in ["LicenseRef-vendor-terms", "LicenseRef-a.1"] {
            assert!(Expression::parse(text).unwrap().is_reference(), "{text}");
        }
        for text in [
            "MIT",
            "CC0-1.0",
            "LicenseRef-a AND LicenseRef-b",
            "LicenseRef-a OR MIT",
        ] {
            assert!(!Expression::parse(text).unwrap().is_reference(), "{text}");
        }
    }

    /// A synthetic open media set: config, two disks, a manual and the terms
    /// text, every content layer linking `terms`, and its generated metadata.
    fn open_set(terms: &Digest) -> Vec<Descriptor> {
        let vendor = Expression::parse("LicenseRef-synthetic-terms").unwrap();
        let generated = Expression::parse("CC0-1.0").unwrap();
        let layer = |kind: &str, bytes: &[u8], license: &Expression, link: bool| {
            let mut d = Descriptor::new(kind, Digest::of(bytes), bytes.len() as u64);
            mark(&mut d, Some(license));
            if link {
                d.annotations.insert(TERMS.into(), terms.to_string());
            }
            d
        };
        vec![
            layer(media_type::MEDIA_SET_CONFIG, b"config", &generated, false),
            layer(media_type::FLOPPY, b"disk 1", &vendor, true),
            layer(media_type::FLOPPY, b"disk 2", &vendor, true),
            layer(media_type::EXTRA, b"manual", &vendor, true),
            layer(media_type::EXTRA, b"terms", &vendor, true),
            layer(media_type::METADATA, b"metadata", &generated, false),
        ]
    }

    #[test]
    fn rule_five_needs_one_open_terms_text_of_the_set_on_every_content_layer() {
        let terms = Digest::of(b"terms");
        let set = open_set(&terms);
        assert!(terms_unlinked(&set).is_empty());

        // The terms text left out of the closure refuses every linking layer.
        let mut absent = set.clone();
        absent.remove(4);
        assert_eq!(terms_unlinked(&absent).len(), 3);
        // A link to bytes that are not in the closure at all.
        let elsewhere = open_set(&Digest::of(b"other terms"));
        assert_eq!(terms_unlinked(&elsewhere).len(), 4);
        // The text present but user-supplied, or not an extra.
        let mut private = set.clone();
        mark(&mut private[4], None);
        assert_eq!(terms_unlinked(&private).len(), 3);
        let mut not_extra = set.clone();
        not_extra[4].media_type = media_type::FLOPPY.into();
        assert_eq!(terms_unlinked(&not_extra).len(), 4);

        // One content layer without the link, with an unparsable link, or
        // naming another extra of the set.
        let mut missing = set.clone();
        missing[2].annotations.remove(TERMS);
        assert_eq!(terms_unlinked(&missing), [missing[2].clone()]);
        let mut garbled = set.clone();
        garbled[3]
            .annotations
            .insert(TERMS.into(), "not a digest".into());
        assert_eq!(terms_unlinked(&garbled), [garbled[3].clone()]);
        let mut other = set.clone();
        other[2]
            .annotations
            .insert(TERMS.into(), Digest::of(b"manual").to_string());
        assert_eq!(terms_unlinked(&other), [other[2].clone()]);

        // Without any link the set has no terms text: every content layer.
        let mut unlinked = set.clone();
        for d in &mut unlinked {
            d.annotations.remove(TERMS);
        }
        assert_eq!(terms_unlinked(&unlinked).len(), 4);
        // User-supplied layers are rules 1 and 2, never rule 5.
        for d in &mut unlinked {
            mark(d, None);
        }
        assert!(terms_unlinked(&unlinked).is_empty());
    }

    #[test]
    fn rule_five_outside_a_media_set_checks_only_the_links_it_finds() {
        let terms = Digest::of(b"terms");
        let mut set = open_set(&terms);
        // A machine image embedding the set's disk: no media-set config, so
        // only layers that carry a link need their text.
        set[0].media_type = media_type::CONFIG.into();
        let disk = set[1].clone();
        let machine = vec![set[0].clone(), disk.clone()];
        assert_eq!(terms_unlinked(&machine), std::slice::from_ref(&disk));
        let with_text = vec![set[0].clone(), disk.clone(), set[4].clone()];
        assert!(terms_unlinked(&with_text).is_empty());
        let mut plain = disk;
        plain.annotations.remove(TERMS);
        assert!(terms_unlinked(&[set[0].clone(), plain]).is_empty());
    }
}
