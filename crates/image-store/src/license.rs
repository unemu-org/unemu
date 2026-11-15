// SPDX-License-Identifier: AGPL-3.0-or-later
//! SPDX licence expressions and the redistribution marks on descriptors;
//! machine-images §5 and §5.3.
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
/// ROM descriptor annotation naming the decoded digest of its corresponding
/// source archive, which travels in the same closure.
pub const SOURCE: &str = "unemu.source";
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
    "GFDL-1.3-only",
    "GFDL-1.3-or-later",
    "GPL-1.0-only",
    "GPL-1.0-or-later",
    "GPL-2.0-only",
    "GPL-2.0-or-later",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
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

/// Push-guard rule 4 for one layer, for any frontend: an open ROM whose
/// licence obliges the corresponding source (under [`ACTIVE`]) and whose
/// linked source archive (`unemu.source`) is not a tar or zip layer of
/// `closure`. Such a ROM may stay only in a private registry outside `unemu/`.
pub fn source_missing(rom: &Descriptor, closure: &[Descriptor]) -> bool {
    if rom.decoded_media_type() != crate::media_type::FIRMWARE
        || !open_license(rom).is_some_and(|l| l.needs_source(ACTIVE))
    {
        return false;
    }
    let link = rom
        .annotations
        .get(SOURCE)
        .and_then(|s| s.parse::<crate::Digest>().ok());
    !link.is_some_and(|link| {
        closure.iter().any(|d| {
            d.names(&link)
                && matches!(
                    d.decoded_media_type(),
                    crate::media_type::TAR | crate::media_type::ZIP
                )
        })
    })
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
}
