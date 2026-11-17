// SPDX-License-Identifier: AGPL-3.0-or-later
//! The file of recipes §6.1: UTF-8 without a byte-order mark, at most 1 MiB,
//! TOML 1.0.
//!
//! The `toml` crate the workspace uses reads TOML 1.1 and has no 1.0-only
//! mode, so the forms only TOML 1.1 accepts are refused here, in a pass over
//! the events of `toml_parser`, the parser inside the crate (recipes §4.6,
//! TOML 1.1): a line break or a comment while the innermost open container
//! is an inline table, a comma before an inline table's closing brace, the
//! escapes `\e` and `\xHH` in a basic string or a quoted key, and a bare time
//! with hours and minutes but no seconds. Everything else the crate parses
//! is TOML 1.0, which the TOML 1.0.0 specification's grammar defines.

use crate::finding::{Rule, Sink};
use std::ops::Range;
use toml::de::{DeTable, DeValue};
use toml::Spanned;
use toml_parser::decoder::Encoding;
use toml_parser::parser::EventReceiver;
use toml_parser::{ErrorSink, Span};

/// The largest recipe file, in bytes.
pub const MAX_FILE: usize = 1 << 20;

const TOML_1_1: &str = "is TOML 1.1 syntax; recipes are TOML 1.0";

/// The text of a file that may be parsed: UTF-8 and at most [`MAX_FILE`].
/// Otherwise the text before the fault, at which it is reported, and what
/// the fault is.
pub(crate) fn text(bytes: Vec<u8>) -> Result<String, (String, String)> {
    if bytes.len() > MAX_FILE {
        return Err((
            String::new(),
            format!(
                "the file is {} bytes; a recipe is at most {MAX_FILE} (1 MiB)",
                bytes.len()
            ),
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        let at = error.utf8_error().valid_up_to();
        let bytes = error.into_bytes();
        let before = String::from_utf8_lossy(&bytes[..at]).into_owned();
        (before, "the file is not UTF-8".to_owned())
    })
}

/// Parse `sink`'s source as TOML 1.0, reporting every syntax finding.
/// `None` when the `toml` crate cannot read it at all.
pub(crate) fn parse<'s>(sink: &mut Sink<'s>) -> Option<Spanned<DeTable<'s>>> {
    let text = sink.source.text.as_str();
    if text.starts_with('\u{feff}') {
        sink.report(
            Rule::Syntax,
            &(0..3),
            "",
            "the file starts with a byte-order mark; a recipe is UTF-8 without one",
        );
    }
    let (root, errors) = DeTable::parse_recoverable(text);
    if !errors.is_empty() {
        for error in errors {
            let span = error.span().unwrap_or(0..0);
            let path = path_at(root.get_ref(), span.start);
            sink.report(
                Rule::Syntax,
                &span,
                &path,
                format!("not TOML: {}", error.message()),
            );
        }
        return None;
    }
    for (span, message) in toml_1_1_forms(text) {
        let path = path_at(root.get_ref(), span.start);
        sink.report(Rule::Syntax, &span, &path, message);
    }
    Some(root)
}

/// The spans of every TOML 1.1 form in `text`, with what each one is.
pub(crate) fn toml_1_1_forms(text: &str) -> Vec<(Range<usize>, String)> {
    let source = toml_parser::Source::new(text);
    let tokens = source.lex().into_vec();
    let mut pass = Prepass {
        text,
        open: Vec::new(),
        after_comma: false,
        found: Vec::new(),
    };
    let mut ignore = |_: toml_parser::ParseError| {};
    toml_parser::parser::parse_document(&tokens, &mut pass, &mut ignore as &mut dyn ErrorSink);
    pass.found
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    InlineTable,
    Array,
}

struct Prepass<'t> {
    text: &'t str,
    open: Vec<Open>,
    /// The last event other than whitespace was a comma of an inline table.
    after_comma: bool,
    found: Vec<(Range<usize>, String)>,
}

impl Prepass<'_> {
    fn in_inline_table(&self) -> bool {
        self.open.last() == Some(&Open::InlineTable)
    }

    fn raw(&self, span: Span) -> &str {
        self.text.get(span.start()..span.end()).unwrap_or("")
    }

    fn escapes(&mut self, span: Span) {
        let bytes = self.raw(span).as_bytes();
        let mut at = 0;
        let mut found = Vec::new();
        while at + 1 < bytes.len() {
            if bytes[at] == b'\\' {
                if matches!(bytes[at + 1], b'e' | b'x') {
                    let start = span.start() + at;
                    found.push((
                        start..start + 2,
                        format!("the escape \\{} {TOML_1_1}", bytes[at + 1] as char),
                    ));
                }
                at += 2;
            } else {
                at += 1;
            }
        }
        self.found.extend(found);
    }

    /// A bare value with `HH:MM` not followed by `:SS`.
    fn time(&mut self, span: Span) {
        let raw = self.raw(span).as_bytes();
        let Some(colon) = raw.iter().position(|&b| b == b':') else {
            return;
        };
        let digits = |range: Range<usize>| {
            raw.get(range)
                .is_some_and(|d| d.iter().all(u8::is_ascii_digit))
        };
        if colon >= 2
            && digits(colon - 2..colon)
            && digits(colon + 1..colon + 3)
            && raw.get(colon + 3) != Some(&b':')
        {
            let start = span.start() + colon - 2;
            self.found.push((
                start..start + 5,
                format!("a time without seconds {TOML_1_1}"),
            ));
        }
    }

    fn other(&mut self) {
        self.after_comma = false;
    }
}

impl EventReceiver for Prepass<'_> {
    fn std_table_open(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.other();
    }
    fn array_table_open(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.other();
    }
    fn inline_table_open(&mut self, _span: Span, _error: &mut dyn ErrorSink) -> bool {
        self.other();
        self.open.push(Open::InlineTable);
        true
    }
    fn inline_table_close(&mut self, span: Span, _error: &mut dyn ErrorSink) {
        if self.in_inline_table() && self.after_comma {
            self.found.push((
                span.start()..span.end(),
                format!("a comma before the closing brace of an inline table {TOML_1_1}"),
            ));
        }
        self.open.pop();
        self.other();
    }
    fn array_open(&mut self, _span: Span, _error: &mut dyn ErrorSink) -> bool {
        self.other();
        self.open.push(Open::Array);
        true
    }
    fn array_close(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.open.pop();
        self.other();
    }
    fn simple_key(&mut self, span: Span, kind: Option<Encoding>, _error: &mut dyn ErrorSink) {
        self.other();
        if kind == Some(Encoding::BasicString) {
            self.escapes(span);
        }
    }
    fn key_sep(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.other();
    }
    fn key_val_sep(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.other();
    }
    fn scalar(&mut self, span: Span, kind: Option<Encoding>, _error: &mut dyn ErrorSink) {
        self.other();
        match kind {
            Some(Encoding::BasicString | Encoding::MlBasicString) => self.escapes(span),
            None => self.time(span),
            Some(_) => {}
        }
    }
    fn value_sep(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.after_comma = self.in_inline_table();
    }
    fn comment(&mut self, span: Span, _error: &mut dyn ErrorSink) {
        if self.in_inline_table() {
            self.found.push((
                span.start()..span.end(),
                format!("a comment inside an inline table {TOML_1_1}"),
            ));
        }
        self.other();
    }
    fn newline(&mut self, span: Span, _error: &mut dyn ErrorSink) {
        if self.in_inline_table() {
            self.found.push((
                span.start()..span.end(),
                format!("a line break inside an inline table {TOML_1_1}"),
            ));
        }
        self.other();
    }
    fn error(&mut self, _span: Span, _error: &mut dyn ErrorSink) {
        self.other();
    }
}

/// One step of a key path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Key(String),
    /// A 0-based array index, written 1-based.
    Index(usize),
}

/// Render a key path: `step[1].members[2].path`. A key that is not bare is
/// quoted.
pub(crate) fn render(path: &[Step]) -> String {
    let mut out = String::new();
    for step in path {
        match step {
            Step::Key(key) => {
                if !out.is_empty() {
                    out.push('.');
                }
                push_key(&mut out, key);
            }
            Step::Index(index) => out.push_str(&format!("[{}]", index + 1)),
        }
    }
    out
}

/// Append `key` to a path, quoted unless it is a bare key.
pub(crate) fn push_key(out: &mut String, key: &str) {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare {
        out.push_str(key);
    } else {
        out.push_str(&format!("{key:?}"));
    }
}

/// The key path of the innermost key or value whose span holds `offset`, as
/// far as the table was read.
pub(crate) fn path_at(root: &DeTable<'_>, offset: usize) -> String {
    let mut path = Vec::new();
    table_path(root, offset, &mut path);
    render(&path)
}

fn table_path(table: &DeTable<'_>, offset: usize, path: &mut Vec<Step>) -> bool {
    for (key, value) in table {
        path.push(Step::Key(key.get_ref().to_string()));
        if value_path(value, offset, path) || key.span().contains(&offset) {
            return true;
        }
        path.pop();
    }
    false
}

fn value_path(value: &Spanned<DeValue<'_>>, offset: usize, path: &mut Vec<Step>) -> bool {
    let inner = match value.get_ref() {
        DeValue::Table(table) => table_path(table, offset, path),
        DeValue::Array(items) => items.iter().enumerate().any(|(index, item)| {
            path.push(Step::Index(index));
            let found = value_path(item, offset, path);
            if !found {
                path.pop();
            }
            found
        }),
        _ => false,
    };
    inner || value.span().contains(&offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forms(text: &str) -> Vec<String> {
        toml_1_1_forms(text)
            .into_iter()
            .map(|(span, message)| format!("{}..{} {message}", span.start, span.end))
            .collect()
    }

    /// The TOML 1.1 cases of the corpus check (recipes §4.6): each is
    /// refused, and each TOML 1.0 control beside it passes.
    #[test]
    fn every_toml_1_1_form_is_refused_and_no_toml_1_0_control_is() {
        for (case, expected) in [
            (
                "a = { b = 1,\n c = 2 }\n",
                "12..13 a line break inside an inline table",
            ),
            (
                "a = { b = 1 # x\n}\n",
                "12..15 a comment inside an inline table",
            ),
            (
                "a = { b = 1, }\n",
                "13..14 a comma before the closing brace",
            ),
            (
                "a = { b = 1, c = 2, }\n",
                "20..21 a comma before the closing brace",
            ),
            ("a = \"\\e\"\n", "5..7 the escape \\e"),
            ("a = \"\\x41\"\n", "5..7 the escape \\x"),
            ("a = \"\"\"\\x41\"\"\"\n", "7..9 the escape \\x"),
            ("\"\\e\" = 1\n", "1..3 the escape \\e"),
            ("a = 07:32\n", "4..9 a time without seconds"),
            ("a = 1979-05-27T07:32\n", "15..20 a time without seconds"),
            ("a = 1979-05-27 07:32\n", "15..20 a time without seconds"),
        ] {
            // A comment in an inline table is followed by its line break.
            let found = forms(case);
            assert!(!found.is_empty(), "{case:?}");
            assert!(found[0].starts_with(expected), "{case:?}: {found:?}");
            assert!(
                found.iter().all(|f| f.ends_with(TOML_1_1)),
                "{case:?}: {found:?}"
            );
        }
        for control in [
            "a = { b = [\n 1,\n 2,\n] }\n",
            "a = \"\\\\e\"\n",
            "a = 'C:\\x'\n",
            "a = \"\"\"\\\n  x\"\"\"\n",
            "a = 07:32:00\n",
            "a = 1979-05-27T07:32:00\n",
            "a = [1, 2,]\n",
            "a = { b = 1, c = { d = 2 } }\n",
            "[x]\nb = 1 # a comment\n",
        ] {
            assert_eq!(forms(control), Vec::<String>::new(), "{control:?}");
        }
    }

    #[test]
    fn paths_name_the_innermost_key_or_array_item() {
        let text = "a = 1\n[[step]]\nid = \"x\"\n[[step]]\nmembers = [{ path = \"p\" }, { path = \"q\" }]\n";
        let (root, errors) = DeTable::parse_recoverable(text);
        assert!(errors.is_empty());
        let at = |needle: &str| path_at(root.get_ref(), text.find(needle).unwrap());
        assert_eq!(at("a ="), "a");
        assert_eq!(at("\"x\""), "step[1].id");
        assert_eq!(at("\"q\""), "step[2].members[2].path");
        assert_eq!(
            render(&[Step::Key("input".into()), Step::Key("a b".into())]),
            "input.\"a b\""
        );
    }
}
