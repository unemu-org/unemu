// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop-wide checks of the ui kit's icons and fonts: every string the
//! desktop and the kit draw has a glyph in the fonts the kit installs, and
//! the marks the desktop writes are the mock-ups' own characters. They read
//! this crate's sources and its menu marks, so they stay with the desktop;
//! the kit's own icon tests are in `ui_kit::shell_icons`. See
//! `backlog/docs/design/icons.md`.

mod inventory;

// The desktop glyph test reads each source file only up to its test module.
#[cfg(test)]
mod tests {
    use ui_kit::shell_icons::with_fonts;

    /// The characters inside string and character literals of a Rust source,
    /// outside comments.
    fn literal_chars(source: &str) -> Vec<char> {
        let chars: Vec<char> = source.chars().collect();
        let mut found = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if c == '/' && next == Some('/') {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            } else if c == 'r'
                && matches!(next, Some('"' | '#'))
                && !chars
                    .get(i.wrapping_sub(1))
                    .is_some_and(|p| p.is_alphanumeric() || *p == '_')
            {
                let mut hashes = 0;
                i += 1;
                while chars.get(i) == Some(&'#') {
                    hashes += 1;
                    i += 1;
                }
                i += 1;
                while i < chars.len() {
                    if chars[i] == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#')) {
                        i += hashes;
                        break;
                    }
                    found.push(chars[i]);
                    i += 1;
                }
                i += 1;
            } else if c == '"' {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    found.push(chars[i]);
                    i += 1;
                }
                i += 1;
            } else if c == '\'' {
                // A character literal, or a lifetime such as 'a or 'static.
                if next == Some('\\') {
                    let end = (i + 2..chars.len())
                        .find(|&k| chars[k] == '\'')
                        .unwrap_or(i);
                    i = end + 1;
                } else if chars.get(i + 2) == Some(&'\'') {
                    found.extend(next);
                    i += 3;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        found
    }

    #[test]
    fn literal_scan_skips_comments_and_reads_raw_and_char_literals() {
        let source = "// ★ not this\nlet a = \"✓ yes\"; let b = '✗'; fn f<'a>() {}\n\
                      /* ⓘ not this */ let c = r#\"⏏ \"raw\"\"#; let d = \"\\\"▶\";";
        let found: String = literal_chars(source)
            .into_iter()
            .filter(|c| !c.is_ascii())
            .collect();
        assert_eq!(found, "✓✗⏏▶");
    }

    /// Every source file of the desktop crate, of the model crates whose
    /// strings it draws and of the ui kit it draws with, by path.
    fn sources() -> Vec<(std::path::PathBuf, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            entries.sort();
            for path in entries {
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    out.push((path, text));
                }
            }
        }
        let mut out = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut out,
        );
        walk(&pc_media::source_dir(), &mut out);
        walk(&pc_library::source_dir(), &mut out);
        walk(&library_ui::source_dir(), &mut out);
        walk(&ui_kit::shell_icons::source_dir(), &mut out);
        assert!(out.len() > 20, "the desktop sources were not found");
        out
    }

    /// Every character any string of the desktop draws has a glyph in both
    /// font families it can be drawn in, so none renders as an empty box.
    #[test]
    fn every_string_the_desktop_draws_has_a_glyph() {
        let sources = sources();
        with_fonts(|fonts| {
            let mut missing = Vec::new();
            for (path, text) in &sources {
                // Test modules and the test-only mock-up inventory draw nothing.
                if path.ends_with("shell_icons/inventory.rs") {
                    continue;
                }
                let drawn = text.split("\n#[cfg(test)]\nmod tests").next().unwrap();
                for ch in literal_chars(drawn) {
                    if ch.is_ascii() {
                        continue;
                    }
                    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                        if !fonts.has_glyph(&egui::FontId::new(14.0, family.clone()), ch) {
                            missing.push(format!(
                                "U+{:04X} {ch} ({family:?}) in {}",
                                ch as u32,
                                path.file_name().unwrap().to_string_lossy()
                            ));
                        }
                    }
                }
            }
            missing.sort();
            missing.dedup();
            assert!(missing.is_empty(), "no glyph for:\n{}", missing.join("\n"));
        });
    }
}
