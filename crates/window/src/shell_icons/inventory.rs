// SPDX-License-Identifier: AGPL-3.0-or-later
//! The marks the desktop writes, checked against the ui kit's inventory of
//! the approved mock-ups' icons.

#[cfg(test)]
mod tests {
    use ui_kit::shell_icons::inventory::{Art, Place, INVENTORY};

    /// The marks the app draws as text are the mock-ups' own characters.
    #[test]
    fn text_marks_are_the_mockup_characters() {
        use crate::shell_model::Command;
        let marks = [
            Command::Start,
            Command::Pause,
            Command::Stop,
            Command::Reset,
        ]
        .map(crate::shell::machine_menu_mark);
        assert_eq!(marks, ["▶", "Ⅱ", "■", ""]);
        assert_eq!(ui_kit::skin::EJECT_MARK, "⏏");
        assert_eq!(crate::catalogue_menu::IN_DRIVE_MARK, "●");
        assert_eq!(ui_kit::skin::audio_mark(true), "🔊");
        assert_eq!(ui_kit::skin::audio_mark(false), "🔇");
        let listed = |ch: char| {
            INVENTORY.iter().any(|e| {
                matches!(e.art, Art::Glyph(g) if g == ch) && matches!(e.place, Place::Text(_))
            })
        };
        for mark in ["▶", "Ⅱ", "■", "⏏", "🔊", "🔇", "★", "⚠", "✓", "✗", "●"]
        {
            assert!(listed(mark.chars().next().unwrap()), "{mark}");
        }
    }
}
