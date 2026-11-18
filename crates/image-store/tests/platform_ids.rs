// SPDX-License-Identifier: AGPL-3.0-or-later
//! Platform ids are compared through `image_store::platform::same` (or the PC
//! provider's `is_pc`, which calls it), never exactly, because a retired id
//! names the same platform as its replacement; machine-images §1, Platform
//! neutrality. The retired PC id `ibm-pc-at` appears only where it is meant:
//! the alias table, the compatibility rows and the tests that build
//! artifacts naming it; the current id `ibm-pc` is spelled out only where it
//! is defined and in tests.
//!
//! A scan of the Rust source of every crate in the workspace, outside
//! comments, with a line that continues on the next one joined to it first.
//! It flags the comparison forms this code base writes when one side is a
//! platform operand: a `platform` field or accessor or a `PLATFORM` constant
//! (through `as_str()`, `as_ref()`, `ok()` and the like), or a PC platform
//! literal. The forms are `==` and `!=`; `matches!` and `match`; `.eq`,
//! `.ne`, `.cmp`, `.starts_with`, `.ends_with` and `.contains`; map lookups
//! with `.get`, `.contains_key` or an index; and match arms on a PC literal.
//! It catches these forms, not every conceivable one: an id copied into a
//! variable of another name and compared there passes. Software platforms of
//! title records (`DOS`) share the field name, so their few comparisons are
//! listed by hand below.

use std::path::{Path, PathBuf};

/// The files that may name the retired id, relative to `crates/`.
const RETIRED_ID: &[&str] = &[
    "image-store/src/platform.rs",
    "image-store/src/compat.rs",
    "cli/tests/platform_alias.rs",
];
/// The production files that may spell out the current PC id: its
/// definition, the alias table, the compatibility rows, the metadata of a
/// new machine and the PC core's description of its platform.
const PC_LITERAL: &[&str] = &[
    "machine-file/src/firmware_package.rs",
    "image-store/src/platform.rs",
    "image-store/src/compat.rs",
    "pc-image/src/lib.rs",
    "pc-core/src/core.rs",
];
/// Comparisons of the software platform of a title record ("DOS"), which is
/// no machine platform id: the file and code its logical line contains.
const SOFTWARE_PLATFORM: &[(&str, &str)] = &[
    (
        "image-store/src/summary.rs",
        ".all(|p| t.platform.as_ref() == Some(p))",
    ),
    (
        "pc-image/src/media_import.rs",
        "ensure(SOFTWARE_PLATFORMS.contains(&platform.as_str()), || {",
    ),
];
/// This file, whose detector tests quote the forms it flags.
const SELF: &str = "image-store/tests/platform_ids.rs";

/// Methods that do not change which id an operand names.
const TRANSPARENT: &[&str] = &[
    "as_str",
    "as_ref",
    "as_deref",
    "to_string",
    "to_owned",
    "clone",
    "unwrap",
    "ok",
    "unwrap_or_default",
    "trim",
    "into",
    "borrow",
];
/// Methods that compare their receiver with their first argument.
const COMPARING: &[&str] = &[
    "eq",
    "ne",
    "cmp",
    "eq_ignore_ascii_case",
    "starts_with",
    "ends_with",
    "contains",
];
/// Methods that look their first argument up.
const LOOKUP: &[&str] = &["get", "get_mut", "contains_key", "remove"];

fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "target") {
                rust_files(&path, files);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The text names the current PC id as a whole literal, not as the start of
/// a longer id such as `ibm-pc-at`.
fn names_pc_literal(code: &str) -> bool {
    code.match_indices("ibm-pc").any(|(index, id)| {
        let after = &code[index + id.len()..];
        !after.starts_with(|c: char| is_word(c) || c == '-') && code[..index].ends_with(['"', '\''])
    })
}

/// An operand that names a platform id: a platform field, accessor or
/// constant, seen through methods that keep the id, or a PC literal.
fn platform_operand(operand: &str) -> bool {
    let operand = operand
        .trim()
        .trim_start_matches(['&', '*', '(', '!'])
        .trim();
    if ["\"ibm-pc", "Some(\"ibm-pc", "Some(&\"ibm-pc"]
        .iter()
        .any(|literal| operand.starts_with(literal))
    {
        return true;
    }
    let mut rest = operand;
    loop {
        rest = rest.trim_end();
        if let Some(before) = rest.strip_suffix('?') {
            rest = before;
            continue;
        }
        if let Some(call) = rest.strip_suffix("()") {
            match call.rsplit_once('.') {
                Some((receiver, method)) if TRANSPARENT.contains(&method) => {
                    rest = receiver;
                    continue;
                }
                _ => rest = call,
            }
        }
        break;
    }
    ["platform", "PLATFORM"].iter().any(|name| {
        rest.strip_suffix(name)
            .is_some_and(|before| !before.ends_with(is_word) && !before.ends_with('"'))
    })
}

/// The text before `end` back to the nearest delimiter outside parentheses:
/// the left operand of a comparison, or the receiver of a method call.
fn left_operand(code: &str, end: usize) -> &str {
    const STOP: &[char] = &['(', ',', '{', '|', ';', '!', '=', '[', '>', '<'];
    let left = &code[..end];
    let mut depth = 0;
    for (index, c) in left.char_indices().rev() {
        match c {
            ')' | ']' => depth += 1,
            '(' | '[' if depth > 0 => depth -= 1,
            c if depth == 0 && STOP.contains(&c) => return left[index + c.len_utf8()..].trim(),
            '&' if depth == 0 && left[index..].starts_with("&&") => {
                return left[index + 2..].trim();
            }
            _ => {}
        }
    }
    left.trim()
}

/// The text from `start` to the nearest delimiter outside parentheses: the
/// right operand of a comparison.
fn right_operand(code: &str, start: usize) -> &str {
    let right = &code[start..];
    let mut depth = 0;
    for (index, c) in right.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 0 => depth -= 1,
            ')' | ']' | ',' | '{' | ';' | '|' => return right[..index].trim(),
            '&' if right[index..].starts_with("&&") => return right[..index].trim(),
            '=' if right[index..].starts_with("=>") => return right[..index].trim(),
            _ => {}
        }
    }
    right.trim()
}

/// The first argument of the call whose `(` is at `open`.
fn first_argument(code: &str, open: usize) -> &str {
    let inner = &code[open + 1..];
    let mut depth = 0;
    for (index, c) in inner.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' if depth == 0 => return inner[..index].trim(),
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => return inner[..index].trim(),
            _ => {}
        }
    }
    inner.trim()
}

/// What is wrong with one logical line of code, if anything.
fn problem(code: &str, may_name_retired: bool, may_name_pc: bool) -> Option<&'static str> {
    if code.trim().is_empty() {
        return None;
    }
    if !may_name_retired && code.contains("ibm-pc-at") {
        return Some("names the retired platform id");
    }
    if !may_name_pc && names_pc_literal(code) {
        return Some("spells out the PC platform id");
    }
    // A match arm or an or-pattern on a PC literal.
    for (index, _) in code.match_indices("\"ibm-pc") {
        let Some(close) = code[index + 1..].find('"') else {
            continue;
        };
        let after = code[index + close + 2..]
            .trim_start()
            .trim_start_matches(')');
        let after = after.trim_start();
        if after.starts_with("=>") || after.starts_with('|') && !after.starts_with("||") {
            return Some("matches on a platform literal");
        }
    }
    for (index, _) in code.match_indices("==").chain(code.match_indices("!=")) {
        if code[..index].ends_with(['=', '!', '<', '>']) || code[index + 2..].starts_with('=') {
            continue;
        }
        if platform_operand(left_operand(code, index))
            || platform_operand(right_operand(code, index + 2))
        {
            return Some("compares platform ids exactly");
        }
    }
    for (index, _) in code.match_indices("matches!(") {
        if platform_operand(first_argument(code, index + "matches!".len())) {
            return Some("matches a platform id exactly");
        }
    }
    for (index, _) in code.match_indices("match ") {
        if index > 0 && code[..index].ends_with(is_word) {
            continue;
        }
        let rest = &code[index + "match ".len()..];
        if let Some(open) = rest.find('{') {
            if platform_operand(&rest[..open]) {
                return Some("matches a platform id exactly");
            }
        }
    }
    for (index, _) in code.match_indices('.') {
        let rest = &code[index + 1..];
        let name: String = rest.chars().take_while(|c| is_word(*c)).collect();
        if !rest[name.len()..].starts_with('(') {
            continue;
        }
        let open = index + 1 + name.len();
        let argument = first_argument(code, open);
        if COMPARING.contains(&name.as_str())
            && (platform_operand(left_operand(code, index)) || platform_operand(argument))
        {
            return Some("compares platform ids exactly");
        }
        if LOOKUP.contains(&name.as_str()) && platform_operand(argument) {
            return Some("looks a platform id up exactly");
        }
    }
    for (index, _) in code.match_indices('[') {
        if !code[..index].ends_with(|c: char| is_word(c) || c == ')' || c == ']') {
            continue;
        }
        if platform_operand(first_argument(code, index)) {
            return Some("looks a platform id up exactly");
        }
    }
    None
}

/// A line of code without its comment; a `//` inside a string literal stays.
fn code_of(line: &str) -> &str {
    match line.find("//") {
        Some(comment) if !line[..comment].contains('"') => &line[..comment],
        _ => line,
    }
}

/// The logical lines of `text`, each with the number of its first line: a
/// line that ends in an open call, a comma, an operator or an assignment, or
/// whose next line starts with a method call, a closing bracket or an
/// operator, continues on the next line.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut lines = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (number, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let code = code_of(line).trim();
        if code.is_empty() {
            continue;
        }
        current = match current.take() {
            Some((first, mut joined))
                if ["(", ",", "==", "!=", "&&", "||", "=", "|"]
                    .iter()
                    .any(|end| joined.ends_with(end))
                    || [".", "==", "!=", "&&", "||", "?", ")", "]", "|"]
                        .iter()
                        .any(|start| code.starts_with(start)) =>
            {
                joined.push(' ');
                joined.push_str(code);
                Some((first, joined))
            }
            previous => {
                lines.extend(previous);
                Some((number + 1, code.to_owned()))
            }
        };
    }
    lines.extend(current);
    lines
}

/// Test code: an integration test, a test-support file, or a test module.
fn test_regions(relative: &str, text: &str) -> usize {
    let name = relative.rsplit('/').next().unwrap_or_default();
    if relative.contains("/tests/") || name.ends_with("tests.rs") || name.contains("test_support") {
        return 0;
    }
    let lines: Vec<&str> = text.lines().collect();
    lines
        .windows(2)
        .position(|pair| {
            let module = pair[1].trim();
            let module = module
                .strip_prefix("pub")
                .map_or(module, |rest| rest.split_once(' ').map_or(rest, |(_, m)| m));
            pair[0].trim() == "#[cfg(test)]" && module.starts_with("mod ") && module.ends_with('{')
        })
        .map_or(usize::MAX, |index| index + 1)
}

#[test]
fn the_detector_flags_exact_platform_comparisons_and_nothing_else() {
    let flagged = |code: &str| problem(code, true, true);
    // The forms this repository used before platform ids had an alias, and
    // the other forms a comparison takes.
    for bad in [
        "if config.platform != self.platform() {",
        "|| platform != manifest.platform()?",
        "|| platform != validator.platform()",
        "if decoded.platform != platform {",
        "Ok(decoded) if decoded.platform == platform => decoded,",
        "if metadata.platform != manifest.platform()? {",
        "if source_metadata.platform != resolved.manifest.platform()? {",
        ".find(|p| p.artifact_type() == manifest.artifact_type() && p.platform() == platform)",
        "|| manifest.platform()? != validator.platform()",
        "if manifest.artifact_type() != mt::MACHINE || manifest.platform().ok() != Some(\"ibm-pc\") {",
        "if summary.platform != PLATFORM {",
        "Ok(Some(g)) if summary.platform == PLATFORM => format!(",
        ".is_some_and(|s| s.kind == ArtifactKind::Firmware && s.platform == \"ibm-pc\")",
        "if config.schema != 1 || config.platform != \"ibm-pc\" {",
        "if platform != \"ibm-pc\" {",
        "if config.platform.as_str() == manifest.platform()? {",
        "if matches!(config.platform.as_str(), \"ibm-pc\" | \"ibm-pc-at\") {",
        "if config.platform.eq(PLATFORM) {",
        "if !manifest.platform()?.ne(\"ibm-pc\") {",
        "if summary.platform.starts_with(\"ibm-pc\") {",
        "if KNOWN.contains(&summary.platform.as_str()) {",
        "let provider = providers.get(manifest.platform()?);",
        "let provider = &providers[&config.platform];",
        "match manifest.platform()? {",
        "\"ibm-pc\" => true,",
        "\"ibm-pc\" | \"ibm-pc-at\" => true,",
        "Some(\"ibm-pc\") => true,",
    ] {
        assert!(flagged(bad).is_some(), "{bad}");
    }
    // A comparison over two lines is one logical line.
    let lines = logical_lines("    if config.platform\n        != manifest.platform()?\n    {\n");
    assert_eq!(lines[0].0, 1);
    assert!(flagged(&lines[0].1).is_some(), "{lines:?}");
    let lines = logical_lines(
        "    let same = matches!(\n        config.platform.as_str(),\n        \"x\"\n    );\n",
    );
    assert!(flagged(&lines[0].1).is_some(), "{lines:?}");
    // The literals, outside the files that may name them.
    assert_eq!(
        problem("platform: \"ibm-pc-at\".into(),", false, true),
        Some("names the retired platform id")
    );
    for literal in [
        "let manifest = Manifest::new(\"ibm-pc\", config, layers)?;",
        "import(&b\"schema=1\\nplatform='ibm-pc'\\n\"[..], None)",
    ] {
        assert_eq!(
            problem(literal, true, false),
            Some("spells out the PC platform id"),
            "{literal}"
        );
    }
    for good in [
        "if !crate::platform::same(&config.platform, self.platform()) {",
        "|| !crate::platform::same(platform, manifest.platform()?)",
        "if !pc_image::is_pc(&summary.platform) {",
        "result.platform = descriptor.platform.clone();",
        "let platform = manifest.platform()?;",
        "if self.software_platform == other {",
        "assert!(a.platform_output == b);",
        "if matches!(kind, Kind::Firmware) {",
        "if values.contains(&value) {",
        "let entry = map.get(&digest);",
        "match kind {",
        "let id = ALIASES[0].0;",
        "platform: \"ibm-pc-at\".into(),",
        "// if config.platform != \"ibm-pc\" {",
    ] {
        assert_eq!(flagged(code_of(good)), None, "{good}");
    }
    assert!(!names_pc_literal("\"ibm-pc-at\""));
    assert!(!names_pc_literal("IBM PC compatible"));
}

#[test]
fn platform_ids_are_never_compared_exactly() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    assert!(files.len() > 100, "the scan found {} files", files.len());
    let mut problems = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(&crates)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if relative == SELF {
            continue;
        }
        let text = std::fs::read_to_string(file).unwrap();
        let tests_from = test_regions(&relative, &text);
        let may_name_retired = RETIRED_ID.contains(&relative.as_str());
        for (number, code) in logical_lines(&text) {
            if SOFTWARE_PLATFORM
                .iter()
                .any(|(path, line)| *path == relative && code.contains(line))
            {
                continue;
            }
            let may_name_pc = number > tests_from || PC_LITERAL.contains(&relative.as_str());
            if let Some(what) = problem(&code, may_name_retired, may_name_pc) {
                problems.push(format!("{relative}:{number}: {what}: {code}"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
