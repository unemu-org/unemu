// SPDX-License-Identifier: AGPL-3.0-or-later
//! The neutral command line carries no code of another crate
//! (`backlog/docs/design/core-contract.md` §7.1 to §7.4). Which crates
//! `cli-core` may use is its manifest's business, and
//! `scripts/crate-boundaries.sh` checks that manifest against the layers; the
//! compiler refuses a path to a crate the manifest does not declare. What
//! neither sees is a source file of another crate compiled in as one of this
//! crate's own modules. So, with comments and the contents of string and
//! character literals blanked out, no file under `src` may load another file
//! with `#[path]` or `include!` (`include_str!`, `include_bytes!`).

use std::path::{Path, PathBuf};

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `code` with comments and the contents of string and character literals
/// replaced by spaces, line breaks kept, so that what remains is code.
fn blank(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::with_capacity(code.len());
    let keep = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(keep(chars[i]));
                    i += 1;
                }
            }
        } else if c == 'r'
            && matches!(next, Some('"' | '#'))
            && !(i > 0 && ident_char(chars[i - 1]))
        {
            // A raw string, `r"…"` or `r#"…"#`, closed by as many hashes.
            let mut j = i + 1;
            while chars.get(j) == Some(&'#') {
                j += 1;
            }
            if chars.get(j) != Some(&'"') {
                out.push(c);
                i += 1;
                continue;
            }
            let hashes = j - i - 1;
            out.push('"');
            i = j + 1;
            while i < chars.len() {
                if chars[i] == '"' && (0..hashes).all(|k| chars.get(i + 1 + k) == Some(&'#')) {
                    out.push('"');
                    i += 1 + hashes;
                    break;
                }
                out.push(keep(chars[i]));
                i += 1;
            }
        } else if c == '"' {
            out.push('"');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    out.push(' ');
                    i += 1;
                }
                if i < chars.len() {
                    out.push(keep(chars[i]));
                    i += 1;
                }
            }
            out.push('"');
            i += 1;
        } else if c == '\'' && (next == Some('\\') || chars.get(i + 2) == Some(&'\'')) {
            // A character literal; a lifetime has no closing quote.
            out.push('\'');
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                if chars[i] == '\\' {
                    out.push(' ');
                    i += 1;
                }
                out.push(' ');
                i += 1;
            }
            out.push('\'');
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// One token of blanked code.
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    /// `::`
    Path,
    Punct(char),
}

/// The tokens of blanked code, each with its line.
fn tokens(code: &str) -> Vec<(Token, usize)> {
    let chars: Vec<char> = code.chars().collect();
    let (mut found, mut line, mut i) = (Vec::new(), 1, 0);
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if ident_char(c) {
            let start = i;
            while i < chars.len() && ident_char(chars[i]) {
                i += 1;
            }
            found.push((Token::Ident(chars[start..i].iter().collect()), line));
        } else if c == ':' && chars.get(i + 1) == Some(&':') {
            found.push((Token::Path, line));
            i += 2;
        } else {
            found.push((Token::Punct(c), line));
            i += 1;
        }
    }
    found
}

/// The lines of `code` that load another file into the crate.
fn violations(code: &str) -> Vec<String> {
    let lines: Vec<&str> = code.lines().collect();
    let tokens = tokens(&blank(code));
    let token = |i: usize| tokens.get(i).map(|(t, _)| t);
    let ident = |i: usize| match token(i) {
        Some(Token::Ident(name)) => Some(name.as_str()),
        _ => None,
    };
    let mut found: Vec<String> = Vec::new();
    for (i, (current, line)) in tokens.iter().enumerate() {
        let reach = match current {
            Token::Punct('#') => {
                let start = if token(i + 1) == Some(&Token::Punct('!')) {
                    2
                } else {
                    1
                };
                token(i + start) == Some(&Token::Punct('[')) && ident(i + start + 1) == Some("path")
            }
            Token::Ident(word) => {
                matches!(word.as_str(), "include" | "include_str" | "include_bytes")
                    && token(i + 1) == Some(&Token::Punct('!'))
            }
            _ => false,
        };
        let text = format!("{line}: {}", lines.get(line - 1).unwrap_or(&"").trim());
        if reach && !found.contains(&text) {
            found.push(text);
        }
    }
    found
}

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn the_neutral_command_line_names_no_emulator_crate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    files.sort();
    assert!(files.len() >= 7, "{files:?}");
    let mut found = Vec::new();
    for file in &files {
        let code = std::fs::read_to_string(file).unwrap();
        for violation in violations(&code) {
            found.push(format!("{}:{violation}", file.display()));
        }
    }
    assert!(
        found.is_empty(),
        "the neutral command line loads another file:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_check_finds_each_kind_of_reach() {
    for code in [
        "#[path = \"../../cli-pc/src/run.rs\"]\nmod run;",
        "#![path = \"elsewhere.rs\"]",
        "const USAGE: &str = include_str!(\"../usage.txt\");",
        "const ROM: &[u8] = include_bytes!(\"rom.bin\");",
        "include!(\"other.rs\");",
        "let s = \"// a comment marker in a string\"; include!(\"x.rs\");",
    ] {
        assert_eq!(violations(code).len(), 1, "{code}");
    }
    for code in [
        "use image_store::{media_type, Store};",
        "// #[path = \"../cli-pc/src/run.rs\"] in a comment",
        "/* include!(\"other.rs\") */ let y = 1;",
        "let s = \"#[path = x] and include!(y) and {\";",
        "let s = r#\"include_str!(\"quoted\") { \"#;",
        "#[derive(Debug)]\nstruct S;",
        "let path = include_path();",
        "fn f<'a>(x: &'a str) -> char {\n    '#'\n}",
    ] {
        assert!(
            violations(code).is_empty(),
            "{code}: {:?}",
            violations(code)
        );
    }
}
