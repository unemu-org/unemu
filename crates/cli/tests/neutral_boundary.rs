// SPDX-License-Identifier: AGPL-3.0-or-later
//! The neutral command line names no emulator crate
//! (`backlog/docs/design/core-contract.md` §7.1 to §7.3). Until it is a
//! crate of its own, whose manifest the crate-boundary check reads, this
//! test reads its source. With comments and the contents of string and
//! character literals blanked out, no file under `src/cli_core` may:
//!
//! - start a path, or a `use` tree, at a PC, emulation or application crate
//!   (`pc_image::…`, `::pc_image::…`, `use pc_image as …`, a crate named in a
//!   `use {…}` group, also one spread over several lines);
//! - name a `crate::` path outside `crate::cli_core`;
//! - climb out of `cli_core` with `super` (the depth counts the file's
//!   place in the module tree and every inline `mod` around the line);
//! - declare `extern crate`, load another file with `#[path]` or `include!`
//!   (`include_str!`, `include_bytes!`), or export a macro with
//!   `#[macro_export]`.
//!
//! What the neutral commands need of a platform comes through the
//! `CliPlatform` trait instead.

use std::path::{Path, PathBuf};

/// The crates of the `pc`, `emulation` and `application` layers the `cli`
/// package can reach, by the name a path starts with.
const FORBIDDEN: &[&str] = &[
    "assembly",
    "audio",
    "bus",
    "chipset",
    "cpu",
    "device",
    "devices",
    "machine",
    "machine_file",
    "pc_harness",
    "pc_image",
    "platforms",
    "testkit",
    "video",
];

/// Words after which a `::` starts a path at a crate.
const KEYWORDS: &[&str] = &[
    "use", "return", "in", "if", "else", "match", "while", "for", "let", "mut", "ref", "move",
    "dyn", "impl", "where", "break",
];

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

/// What an open brace opens.
#[derive(Clone, Copy, PartialEq)]
enum Brace {
    /// An inline `mod NAME {`, one module deeper.
    Module,
    /// A group of a `use` whose members each start at a crate:
    /// `use {std::io, …}`.
    RootGroup,
    Other,
}

/// The lines of `code` that reach outside the neutral command line, for a
/// file `depth` modules below `cli_core` (0 for `cli_core/mod.rs`).
fn violations(code: &str, depth: usize) -> Vec<String> {
    let lines: Vec<&str> = code.lines().collect();
    let tokens = tokens(&blank(code));
    let token = |i: Option<usize>| i.and_then(|i| tokens.get(i)).map(|(t, _)| t);
    let ident = |i: Option<usize>| match token(i) {
        Some(Token::Ident(name)) => Some(name.as_str()),
        _ => None,
    };
    let mut found: Vec<String> = Vec::new();
    let mut flag = |line: usize| {
        let text = format!("{line}: {}", lines.get(line - 1).unwrap_or(&"").trim());
        if !found.contains(&text) {
            found.push(text);
        }
    };
    let mut braces: Vec<Brace> = Vec::new();
    let mut in_use = false;
    for (i, (current, line)) in tokens.iter().enumerate() {
        let line = *line;
        let back = |n: usize| i.checked_sub(n);
        let ahead = |n: usize| Some(i + n);
        // A `::` with no path before it starts a path at a crate; a keyword
        // before it is no path.
        let leading_path = token(back(1)) == Some(&Token::Path)
            && match token(back(2)) {
                Some(Token::Ident(word)) => KEYWORDS.contains(&word.as_str()),
                Some(Token::Punct('>')) => false,
                _ => true,
            };
        // Where a member of a `use` tree starts at a crate.
        let use_root = in_use
            && match token(back(1)) {
                Some(Token::Ident(word)) => word == "use",
                Some(Token::Path) => leading_path,
                Some(Token::Punct(',' | '{')) => braces.last() == Some(&Brace::RootGroup),
                _ => false,
            };
        match current {
            Token::Punct('{') => {
                braces.push(if ident(back(2)) == Some("mod") {
                    Brace::Module
                } else if in_use
                    && (ident(back(1)) == Some("use")
                        || leading_path
                        || matches!(token(back(1)), Some(Token::Punct(',' | '{')))
                            && braces.last() == Some(&Brace::RootGroup))
                {
                    Brace::RootGroup
                } else {
                    Brace::Other
                });
            }
            Token::Punct('}') => {
                braces.pop();
            }
            Token::Punct(';') => in_use = false,
            Token::Punct('#') => {
                let start = if token(ahead(1)) == Some(&Token::Punct('!')) {
                    2
                } else {
                    1
                };
                if token(ahead(start)) == Some(&Token::Punct('['))
                    && matches!(ident(ahead(start + 1)), Some("path" | "macro_export"))
                {
                    flag(line);
                }
            }
            Token::Ident(word) => {
                let word = word.as_str();
                if word == "use" {
                    in_use = true;
                    continue;
                }
                if matches!(word, "include" | "include_str" | "include_bytes")
                    && token(ahead(1)) == Some(&Token::Punct('!'))
                {
                    flag(line);
                }
                if word == "extern" && ident(ahead(1)) == Some("crate") {
                    flag(line);
                }
                let path_root =
                    token(ahead(1)) == Some(&Token::Path) && token(back(1)) != Some(&Token::Path);
                // Inside a `use` only a tree's first segment names a crate.
                let root = if in_use {
                    use_root
                } else {
                    path_root || leading_path
                };
                if !root {
                    continue;
                }
                if FORBIDDEN.contains(&word) {
                    flag(line);
                }
                if word == "crate"
                    && token(ahead(1)) == Some(&Token::Path)
                    && ident(ahead(2)) != Some("cli_core")
                {
                    flag(line);
                }
                if word == "super" {
                    let mut climbed = 1;
                    while token(ahead(climbed * 2 - 1)) == Some(&Token::Path)
                        && ident(ahead(climbed * 2)) == Some("super")
                    {
                        climbed += 1;
                    }
                    let inline = braces.iter().filter(|b| **b == Brace::Module).count();
                    if climbed > depth + inline {
                        flag(line);
                    }
                }
            }
            _ => {}
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

/// How many modules below `cli_core` a file is: `mod.rs` is `cli_core`
/// itself, `journal.rs` one below it, `image/media.rs` two.
fn depth(root: &Path, file: &Path) -> usize {
    let relative = file.strip_prefix(root).unwrap();
    let parts = relative.components().count();
    if relative.file_name().is_some_and(|n| n == "mod.rs") {
        parts - 1
    } else {
        parts
    }
}

#[test]
fn the_neutral_command_line_names_no_emulator_crate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli_core");
    let mut files = Vec::new();
    sources(&root, &mut files);
    files.sort();
    assert!(files.len() >= 7, "{files:?}");
    let mut found = Vec::new();
    for file in &files {
        let code = std::fs::read_to_string(file).unwrap();
        for violation in violations(&code, depth(&root, file)) {
            found.push(format!("{}:{violation}", file.display()));
        }
    }
    assert!(
        found.is_empty(),
        "the neutral command line reaches outside itself:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_check_finds_each_kind_of_reach() {
    for (code, depth) in [
        ("use pc_image::firmware;", 1),
        ("    let x = assembly::AssemblyOptions::default();", 1),
        ("use crate::cli_pc::PcPlatform;", 1),
        ("    crate::create::parse(args)", 1),
        ("use {std::io, machine_file::load};", 1),
        ("use {\n    std::io,\n    pc_harness::run_boot,\n};", 1),
        ("use ::{std::io, devices::OPL3_NOTICE};", 1),
        ("use platforms;", 1),
        ("use pc_image as pc;", 1),
        ("extern crate devices;", 1),
        ("fn f() -> video::Frame { todo!() }", 1),
        ("let x = ::pc_image::PLATFORM;", 1),
        ("use ::assembly::FirmwareEntry;", 1),
        (
            "let s = \"// a comment marker in a string\"; let p = pc_image::PLATFORM;",
            1,
        ),
        ("use super::super::about;", 1),
        ("use super::command;", 0),
        ("mod tests {\n    use super::super::about;\n}", 0),
        ("#[path = \"../cli_pc/run.rs\"]\nmod run;", 1),
        ("const USAGE: &str = include_str!(\"../usage.txt\");", 1),
        ("include!(\"other.rs\");", 1),
        ("#[macro_export]\nmacro_rules! m {\n    () => {};\n}", 1),
    ] {
        assert_eq!(violations(code, depth).len(), 1, "depth {depth}: {code}");
    }
    for (code, depth) in [
        ("use crate::cli_core::CliPlatform;", 1),
        ("use image_store::{media_type, Store};", 1),
        ("// pc_image::recipe::PcRecipes is the PC's provider", 1),
        ("/* assembly::AssemblyOptions */ let y = 1;", 1),
        ("let s = \"pc_image::x and // and {\";", 1),
        ("let s = r#\"crate::cli_pc \"quoted\" { \"#;", 1),
        ("let machine = store.read_machines()?;", 1),
        ("use image_registry::guard;", 1),
        ("let platforms = &[];", 1),
        ("fn platform_of(platforms: &[&dyn CliPlatform]) {}", 1),
        ("image_store::firmware::Config::decode(bytes)", 1),
        (
            "use image_registry::{\n    device::Thing,\n    machine,\n};",
            1,
        ),
        ("use super::Io;", 1),
        ("use super::super::CliPlatform;", 2),
        ("mod tests {\n    use super::*;\n}", 0),
        (
            "fn f<'a>(x: &'a str) -> char {\n    '{'\n}\nuse pc_harness_free::x;",
            1,
        ),
        ("let v = <Vec<u8> as Default>::default();", 1),
    ] {
        assert!(
            violations(code, depth).is_empty(),
            "depth {depth}: {code}: {:?}",
            violations(code, depth)
        );
    }
}
