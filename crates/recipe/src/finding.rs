// SPDX-License-Identifier: AGPL-3.0-or-later
//! What `recipe check` reports: a finding names the file, the line and
//! column, the rule of recipes §6.11 it breaks and the key path of the value
//! (recipes §4.6, error messages).

use std::fmt;
use std::ops::Range;

/// A rule of recipes §6.11. Rule 5 reports nothing: it prints the split of
/// required and optional inputs ([`crate::FileReport`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    /// The TOML of §6.1: syntax, keys, types and values.
    Syntax = 1,
    /// One identity table, and only the tables of its kind.
    Kind = 2,
    /// Names and references.
    Reference = 3,
    /// Inputs and steps no output uses.
    Unused = 4,
    /// The open statement of §6.6.
    License = 6,
    /// What the platform provider knows.
    Provider = 7,
    /// A digest and its lock.
    Lock = 8,
    /// Sizes and counts.
    Limit = 9,
    /// Sizes, formats, names and ROMs the provider refuses.
    Platform = 10,
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "6.11.{}", *self as u8)
    }
}

/// One refusal of `recipe check`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Finding {
    /// The file as it was named to the checker.
    pub file: String,
    /// 1-based line.
    pub line: usize,
    /// 1-based column, in characters.
    pub column: usize,
    pub rule: Rule,
    /// The key path of the value: `step[1].members[2].path`, with 1-based
    /// indices as a person counts the tables of a file. Empty for the file.
    pub path: String,
    pub message: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: error[{}] ",
            self.file, self.line, self.column, self.rule
        )?;
        if !self.path.is_empty() {
            write!(f, "{}: ", self.path)?;
        }
        f.write_str(&self.message)
    }
}

/// A recipe file's text and the start of each of its lines.
pub(crate) struct Source {
    pub name: String,
    pub text: String,
    line_starts: Vec<usize>,
}

impl Source {
    pub fn new(name: impl Into<String>, text: String) -> Self {
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Self {
            name: name.into(),
            text,
            line_starts,
        }
    }

    /// Line and column of a byte offset, both from 1; the column counts
    /// characters.
    pub fn position(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.text.len());
        let line = self.line_starts.partition_point(|&start| start <= offset);
        let start = self.line_starts[line.saturating_sub(1)];
        let column = self
            .text
            .get(start..offset)
            .map_or(offset - start, |text| text.chars().count());
        (line, column + 1)
    }
}

/// Findings of one file, collected while it is read and checked.
pub(crate) struct Sink<'s> {
    pub source: &'s Source,
    pub findings: Vec<Finding>,
}

impl<'s> Sink<'s> {
    pub fn new(source: &'s Source) -> Self {
        Self {
            source,
            findings: Vec::new(),
        }
    }

    pub fn report(
        &mut self,
        rule: Rule,
        span: &Range<usize>,
        path: &str,
        message: impl Into<String>,
    ) {
        let (line, column) = self.source.position(span.start);
        self.findings.push(Finding {
            file: self.source.name.clone(),
            line,
            column,
            rule,
            path: path.to_owned(),
            message: message.into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_lines_and_characters_from_one() {
        let source = Source::new("x", "ab\nčd\n\nx".into());
        assert_eq!(source.position(0), (1, 1));
        assert_eq!(source.position(1), (1, 2));
        assert_eq!(source.position(2), (1, 3));
        assert_eq!(source.position(3), (2, 1));
        // `č` is two bytes and one column.
        assert_eq!(source.position(5), (2, 2));
        assert_eq!(source.position(7), (3, 1));
        assert_eq!(source.position(8), (4, 1));
        assert_eq!(source.position(99), (4, 2));
    }

    #[test]
    fn a_finding_reads_file_line_column_rule_path_and_message() {
        let finding = Finding {
            file: "a.media.toml".into(),
            line: 15,
            column: 1,
            rule: Rule::Syntax,
            path: "media.revison".into(),
            message: "unknown key `revison`".into(),
        };
        assert_eq!(
            finding.to_string(),
            "a.media.toml:15:1: error[6.11.1] media.revison: unknown key `revison`"
        );
        assert_eq!(Rule::Platform.to_string(), "6.11.10");
    }
}
