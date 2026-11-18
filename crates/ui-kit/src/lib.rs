// SPDX-License-Identifier: AGPL-3.0-or-later
//! The desktop's skin and widget helpers, free of any emulated machine; see
//! `backlog/docs/design/spec.md` §9.4 and `backlog/docs/design/core-contract.md` §8.
//!
//! [`skin`] holds the two skins (classic and modern, each light or dark) as
//! egui visuals, the fonts every desktop window installs, the classic title
//! strips and bevels, the one-Escape-one-dialog rule and the text marks.
//! [`shell_icons`] paints the approved mock-ups' icons, [`pane`] draws the
//! library's details panes, [`logo`] decodes the embedded brand art,
//! [`file_dialog`] runs host file dialogs without a nested event loop and
//! [`text_clipboard`] gives cut, copy and paste back to text fields. Behind
//! the `test-support` feature, `test_support` drives egui frame by frame for
//! the tests of the crates that draw with the kit.
//!
//! The crate knows no platform, machine or image format, and the boundary
//! check keeps it so (layer `neutral`, `scripts/crate-boundaries.sh`).

/// Attribution and original licence texts for egui's bundled UI fonts.
pub const EGUI_FONT_NOTICE: &str = concat!(
    include_str!("../assets/fonts/egui/NOTICE.txt"),
    "\n",
    include_str!("../assets/fonts/egui/UFL.txt"),
    "\n",
    include_str!("../assets/fonts/egui/Hack-Regular.txt"),
    "\n",
    include_str!("../assets/fonts/egui/OFL.txt"),
    "\n",
    include_str!("../assets/fonts/egui/emoji-icon-font-mit-license.txt"),
);

/// The desktop's symbol font: five DejaVu Sans glyphs (✓ ✗ ✕ Ⅱ ⌫) that
/// egui's bundled fonts lack, cut from the DejaVu fonts 2.37 release.
pub const SYMBOL_FONT: &[u8] = include_bytes!("../assets/fonts/dejavu/DejaVuSans-UI-symbols.ttf");

/// Attribution and the original licence text of [`SYMBOL_FONT`].
pub const SYMBOL_FONT_NOTICE: &str = concat!(
    "DejaVu Sans 2.37, subset to five symbols by unemupc; outlines unmodified.\n",
    "Fonts are (c) Bitstream; Arev glyphs (c) Tavmjong Bah; DejaVu changes are public domain.\n",
    "Bitstream Vera and Arev Fonts licences:\n\n",
    include_str!("../assets/fonts/dejavu/LICENSE.txt"),
);

pub mod file_dialog;
pub mod logo;
pub mod pane;
pub mod shell_icons;
pub mod skin;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod text_clipboard;
