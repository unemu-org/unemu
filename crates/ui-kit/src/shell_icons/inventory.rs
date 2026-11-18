// SPDX-License-Identifier: AGPL-3.0-or-later
//! Every icon of the approved desktop mock-ups and where the app draws it.
//!
//! The tests read the mock-ups themselves, so an icon added to one of them
//! without an entry here, or an entry whose icon left its mock-up, fails the
//! build. `backlog/docs/design/icons.md` summarises the same list. The
//! `test-support` feature lends the list to the tests of the crates that
//! draw the marks.

use super::Icon;

/// One icon of one mock-up.
pub struct Entry {
    /// The mock-up file in `backlog/docs/design/mockups/`.
    pub mockup: &'static str,
    pub art: Art,
    /// What the mock-up marks with it.
    pub meaning: &'static str,
    pub place: Place,
}

pub enum Art {
    /// An inline SVG, found by a fragment of its markup that only it has.
    Svg(&'static str),
    /// A character the mock-up writes as text.
    Glyph(char),
}

pub enum Place {
    /// Painted with egui shapes from the SVG's coordinates.
    Painted(Icon, &'static str),
    /// Drawn as the same character in the app's text.
    Text(&'static str),
    /// egui's own widget draws the mark there.
    Egui(&'static str),
    /// Not drawn: the reason.
    Missing(&'static str),
}

/// Characters the mock-ups use as typography, not as icons: dashes, quotes,
/// separators, prose arrows and the no-break space of grouped numbers. They
/// need glyphs too.
pub const TYPOGRAPHY: &str = "·—–’“”…›‹→≈′µ\u{a0}";

const MAIN: &str = "main-window.html";
const LIBRARY: &str = "library.html";
const MACHINES: &str = "machines.html";
const FIRMWARE: &str = "firmware.html";
const MEDIA: &str = "install-media.html";
const EXTRAS: &str = "install-media-extras.html";
const NEW_MACHINE: &str = "new-machine.html";
const SAVE: &str = "save-as-image.html";
const ACCOUNTS: &str = "registry-accounts.html";
const TEMPLATES: &str = "get-templates.html";
const PICKER: &str = "media-picker.html";
const FIRMWARE_SETTINGS: &str = "firmware-settings.html";
const REMOTES: &str = "remotes.html";
const PLAYER: &str = "player.html";

const TOOLBAR_ONLY_MACHINE: &str =
    "the app has one toolbar, the main-window mock-up's machine toolbar; \
     this command is not on it (it is in the menus and the details pane)";
const SYSTEM_TITLE_BAR: &str =
    "a window button; the system title bar draws it (the classic title strip has none)";
const PLAYER_LATER: &str =
    "the player window is built in a later step, in crates/player-ui; nothing draws it yet";
const PLAYER_LATER_SAME_ART: &str =
    "the player window is built in a later step, in crates/player-ui; the desktop already \
     paints the same art for its own toolbar";
const CLOSE: &str =
    "dialog close buttons where the app's dialogs have one (Settings, Preferences, New machine, Edit metadata)";

const fn svg(
    mockup: &'static str,
    key: &'static str,
    meaning: &'static str,
    place: Place,
) -> Entry {
    Entry {
        mockup,
        art: Art::Svg(key),
        meaning,
        place,
    }
}

const fn glyph(mockup: &'static str, ch: char, meaning: &'static str, place: Place) -> Entry {
    Entry {
        mockup,
        art: Art::Glyph(ch),
        meaning,
        place,
    }
}

pub const INVENTORY: &[Entry] = &[
    // main-window.html, revision 2.
    svg(MAIN, "class=\"appicon\"", "application", Place::Painted(Icon::App, "classic title strip")),
    svg(MAIN, "M4 2l10 6-10 6z", "Start", Place::Painted(Icon::Play, "toolbar Start, and Resume")),
    svg(MAIN, "x=\"9.5\" y=\"2\" width=\"3.5\"", "Pause", Place::Painted(Icon::Pause, "toolbar Pause")),
    svg(MAIN, "width=\"10\" height=\"10\" fill=\"#b23a2b\"", "Stop", Place::Painted(Icon::Stop, "toolbar Stop")),
    svg(MAIN, "fill=\"#3a3f4a\"", "insert into A:", Place::Painted(Icon::Floppy, "toolbar A:")),
    svg(MAIN, "M8 1v3M8 12v3", "Settings", Place::Painted(Icon::Settings, "toolbar Settings")),
    svg(MAIN, "M8 2l6 3-6 3-6-3z\" fill=\"#4b79c1\"", "Disk layers", Place::Painted(Icon::Layers, "toolbar Layers")),
    svg(
        MAIN,
        "stroke=\"#1c2b44\"",
        "a machine in the welcome list",
        Place::Missing("the welcome list holds images and machine files; they take the machines mock-up's image and machine-file icons"),
    ),
    glyph(MAIN, '⏏', "Eject", Place::Painted(Icon::Eject, "toolbar Eject (the same symbol, drawn)")),
    glyph(MAIN, '⏏', "Eject in a menu", Place::Text("mark column of Media › Eject A:/B: and the floppy drive menus")),
    glyph(MAIN, '▶', "Start in the Machine menu", Place::Text("Machine menu mark column")),
    glyph(MAIN, 'Ⅱ', "Pause in the Machine menu", Place::Text("Machine menu mark column")),
    glyph(MAIN, '■', "Stop in the Machine menu", Place::Text("Machine menu mark column")),
    glyph(MAIN, '✓', "View › Toolbar, Status bar and Mute audio are on", Place::Egui("check boxes of View › Toolbar and Status bar; Mute audio says Unmute audio instead")),
    glyph(MAIN, '●', "the chosen scale, the chosen write mode", Place::Egui("View's scale choices show the selection; a drive menu names its write mode as text")),
    glyph(MAIN, '⚠', "a machine that cannot start", Place::Text("after the name of an unreadable image or a machine file with a problem")),
    glyph(MAIN, '🔊', "audio playing", Place::Text("status bar audio cell")),
    glyph(MAIN, '🔇', "audio muted", Place::Text("status bar audio cell, while muted or unavailable")),
    glyph(MAIN, '└', "layer chain", Place::Text("Disk layers tree")),
    glyph(MAIN, '─', "layer chain", Place::Text("Disk layers tree")),
    glyph(MAIN, '×', "close", Place::Text(CLOSE)),
    glyph(MAIN, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // library.html.
    svg(LIBRARY, "M4 2l10 6-10 6z", "Run", Place::Painted(Icon::Play, "toolbar Start, which runs the selected image")),
    svg(LIBRARY, "M8 5v6M5 8h6", "New", Place::Missing(TOOLBAR_ONLY_MACHINE)),
    svg(LIBRARY, "M4 6h8M4 9h8M4 12h5", "Properties", Place::Missing(TOOLBAR_ONLY_MACHINE)),
    svg(LIBRARY, "M2 8h12M8 2c3 3 3 9 0 12", "Browse registry", Place::Missing(TOOLBAR_ONLY_MACHINE)),
    svg(LIBRARY, "M8 13V3M4 7l4-4 4 4", "Push", Place::Missing(TOOLBAR_ONLY_MACHINE)),
    svg(LIBRARY, "M4 4l8 8M12 4l-8 8", "Delete", Place::Missing(TOOLBAR_ONLY_MACHINE)),
    glyph(LIBRARY, '★', "your rating", Place::Missing("the app has no rating field; favourites carry the star instead")),
    glyph(LIBRARY, '−', "an expanded group", Place::Text("repository group rows; facet groups are egui collapsing headers")),
    glyph(LIBRARY, '▲', "sorted ascending", Place::Text("sortable column headers")),
    glyph(LIBRARY, '▼', "sorted descending", Place::Text("sortable column headers")),
    glyph(LIBRARY, '▸', "the expand control, named in the mock-up's note", Place::Missing("only in the mock-up's explanatory note")),
    glyph(LIBRARY, '×', "close", Place::Text(CLOSE)),
    glyph(LIBRARY, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // machines.html.
    svg(MACHINES, "fill=\"#c9d6e8\" stroke=\"#0b1a33\"", "application", Place::Painted(Icon::App, "classic title strip")),
    svg(MACHINES, "M4 2l10 6-10 6z", "Start", Place::Painted(Icon::Play, "toolbar Start")),
    svg(MACHINES, "rx=\"3\"", "Gaming mode", Place::Missing(TOOLBAR_ONLY_MACHINE)),
    svg(MACHINES, "width=\"10\" height=\"10\" fill=\"#b23a2b\"", "Stop", Place::Painted(Icon::Stop, "toolbar Stop")),
    svg(MACHINES, "M8 5v6M5 8h6", "New machine from image", Place::Missing(TOOLBAR_ONLY_MACHINE)),
    svg(MACHINES, "M8 1v3M8 12v3", "Settings", Place::Painted(Icon::Settings, "toolbar Settings")),
    svg(MACHINES, "M8 1.5l6 3-6 3-6-3z\" fill=\"#4b79c1\"", "Save as new image", Place::Missing(TOOLBAR_ONLY_MACHINE)),
    svg(MACHINES, "M5 14h6M8 11v3", "a machine", Place::Painted(Icon::Machine, "tree Machines, machine rows, the machine details heading")),
    svg(MACHINES, "M8 1.5l6 3-6 3-6-3z\" fill=\"none\"", "an image", Place::Painted(Icon::Image, "tree Images, image rows, the image details heading")),
    svg(MACHINES, "x=\"2.5\" y=\"1.5\" width=\"11\" height=\"13\"", "an installation media set", Place::Painted(Icon::Media, "tree Installation media, media set rows")),
    svg(MACHINES, "M6 1v2.5M10 1v2.5", "a firmware package", Place::Painted(Icon::Firmware, "tree Firmware, firmware package rows")),
    svg(MACHINES, "M3 1.5h7l3 3v10H3z", "a machine file", Place::Painted(Icon::MachineFile, "tree Machine files, machine file rows")),
    glyph(MACHINES, '★', "a favourite", Place::Text("after a favourite machine's or image's name in its row and its details heading")),
    glyph(MACHINES, '⚠', "an unfinished gaming session", Place::Text("before the library's Unfinished sessions heading")),
    glyph(MACHINES, 'ⓘ', "machine settings and migration notes", Place::Painted(Icon::Info, "the migration notice and the banner of Settings… on a machine")),
    glyph(MACHINES, '⚑', "a machine whose tag moved", Place::Text("Source image badge in a machine's row and details")),
    glyph(MACHINES, '●', "a setting that differs from the image", Place::Text("Settings… on a machine: the differing field's hint and its tab label")),
    glyph(MACHINES, '✓', "a finished wizard step", Place::Text("New machine wizard step list")),
    glyph(MACHINES, '−', "an expanded repository group", Place::Text("repository group rows")),
    glyph(MACHINES, '▲', "sorted ascending", Place::Text("sortable column headers")),
    glyph(MACHINES, '▼', "sorted descending", Place::Text("sortable column headers")),
    glyph(MACHINES, '⌘', "the macOS Command key", Place::Text("macOS menu shortcuts")),
    glyph(MACHINES, '⇧', "the macOS Shift key", Place::Text("macOS menu shortcuts")),
    glyph(MACHINES, '⌫', "the macOS Delete key", Place::Text("macOS shortcut of Delete machine… and Delete tag…")),
    glyph(MACHINES, '×', "close", Place::Text(CLOSE)),
    glyph(MACHINES, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // firmware.html.
    glyph(FIRMWARE, 'ⓘ', "Edit metadata creates a new revision", Place::Painted(Icon::Info, "Edit metadata… note")),
    glyph(FIRMWARE, '⚠', "no source: private registries only", Place::Text("firmware rows and details, the push guard line")),
    glyph(FIRMWARE, '✓', "a check that passed", Place::Text("Edit metadata… source line")),
    glyph(FIRMWARE, '×', "close", Place::Text(CLOSE)),
    glyph(FIRMWARE, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // install-media.html and install-media-extras.html.
    glyph(MEDIA, '↑', "move a disk up", Place::Text("Import installation media… disk order")),
    glyph(MEDIA, '↓', "move a disk down", Place::Text("Import installation media… disk order")),
    glyph(MEDIA, '▸', "a submenu", Place::Egui("Media › Insert into A:/B: submenus")),
    glyph(MEDIA, '×', "close", Place::Text(CLOSE)),
    glyph(MEDIA, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    glyph(EXTRAS, '↑', "move a disk up", Place::Text("Import installation media… disk order")),
    glyph(EXTRAS, '↓', "move a disk down", Place::Text("Import installation media… disk order")),
    glyph(EXTRAS, '▸', "a submenu", Place::Egui("Media › Insert into A:/B: submenus")),
    glyph(EXTRAS, '×', "close", Place::Text(CLOSE)),
    glyph(EXTRAS, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // new-machine.html.
    glyph(NEW_MACHINE, '✓', "a free tag, a finished step", Place::Text("Name and tag check, step list")),
    glyph(NEW_MACHINE, '×', "close", Place::Text(CLOSE)),
    glyph(NEW_MACHINE, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // save-as-image.html.
    glyph(SAVE, 'ⓘ', "Settings on an image creates a new revision", Place::Painted(Icon::Info, "image Settings… note")),
    glyph(SAVE, '✕', "a refused version tag", Place::Painted(Icon::Refusal, "Version tag already exists dialog")),
    glyph(SAVE, '●', "a tab with changes", Place::Text("image Settings… tab labels")),
    glyph(SAVE, '✓', "a free tag, a valid draft", Place::Text("Save as new image… tag check, publish review validator line")),
    glyph(SAVE, '└', "layer chain", Place::Text("Disk layers tree")),
    glyph(SAVE, '─', "layer chain", Place::Text("Disk layers tree")),
    glyph(SAVE, '×', "close", Place::Text(CLOSE)),
    glyph(SAVE, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // registry-accounts.html.
    glyph(ACCOUNTS, '●', "signed in", Place::Text("Preferences › Registries table")),
    glyph(ACCOUNTS, '○', "signed out", Place::Text("Preferences › Registries table")),
    glyph(ACCOUNTS, '✓', "a check that passed", Place::Text("Preferences › Registries checks, Push… checks")),
    glyph(ACCOUNTS, '✗', "a check that failed", Place::Text("Preferences › Registries checks, Push… checks")),
    glyph(ACCOUNTS, '×', "close", Place::Text(CLOSE)),
    glyph(ACCOUNTS, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // get-templates.html.
    svg(TEMPLATES, "viewBox=\"0 0 48 48\"", "no machine images yet", Place::Painted(Icon::EmptyLibrary, "the empty library card")),
    glyph(TEMPLATES, '✓', "pulled", Place::Text("Pull… results of Remotes, which replaced the Get templates dialog")),
    glyph(TEMPLATES, '✗', "failed", Place::Text("Pull… results of Remotes, which replaced the Get templates dialog")),
    glyph(TEMPLATES, '×', "close", Place::Text(CLOSE)),
    glyph(TEMPLATES, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // media-picker.html.
    svg(PICKER, "fill=\"#c9d6e8\" stroke=\"#0b1a33\"", "application", Place::Painted(Icon::App, "classic title strip")),
    svg(PICKER, "M4 2l10 6-10 6z", "Start", Place::Painted(Icon::Play, "toolbar Start")),
    svg(PICKER, "width=\"10\" height=\"10\" fill=\"#b23a2b\"", "Stop", Place::Painted(Icon::Stop, "toolbar Stop")),
    svg(
        PICKER,
        "fill=\"#3a3f4a\"",
        "insert into A: or B:",
        Place::Painted(Icon::Floppy, "toolbar A: and B:, which open the media picker"),
    ),
    svg(PICKER, "M5 14h6M8 11v3", "a machine", Place::Painted(Icon::Machine, "tree Machines and machine rows, as in the machines mock-up")),
    svg(PICKER, "x=\"2.5\" y=\"1.5\" width=\"11\" height=\"13\"", "an installation media set", Place::Painted(Icon::Media, "tree Installation media, media set rows")),
    glyph(PICKER, '▾', "the Insert into… drive menu", Place::Text("Installation media › Insert into… ▾ menu button")),
    glyph(PICKER, '↑', "the picker's arrow keys, named in the mock-up's note", Place::Text("the media picker's key note")),
    glyph(PICKER, '↓', "the picker's arrow keys, named in the mock-up's note", Place::Text("the media picker's key note")),
    glyph(PICKER, '⌘', "the macOS Command key", Place::Text("macOS menu shortcuts")),
    glyph(PICKER, '⇧', "the macOS Shift key", Place::Text("macOS menu shortcuts")),
    glyph(PICKER, '▸', "a submenu: Catalogue under each floppy drive", Place::Egui("Catalogue under each floppy drive of the Media menu and a drive chip's menu: egui's submenu arrow")),
    glyph(PICKER, '●', "the disk in the drive, in a Catalogue submenu", Place::Text("the Catalogue submenu's mark column, before the disk in the drive")),
    glyph(PICKER, '←', "the Left arrow key, named in the Catalogue notes", Place::Missing("only in the mock-up's notes")),
    glyph(PICKER, '×', "close", Place::Text(CLOSE)),
    glyph(PICKER, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // firmware-settings.html.
    svg(FIRMWARE_SETTINGS, "fill=\"#c9d6e8\" stroke=\"#0b1a33\"", "application", Place::Painted(Icon::App, "classic title strip")),
    svg(FIRMWARE_SETTINGS, "M4 2l10 6-10 6z", "Start", Place::Painted(Icon::Play, "toolbar Start")),
    svg(FIRMWARE_SETTINGS, "M8 1v3M8 12v3", "Settings", Place::Painted(Icon::Settings, "toolbar Settings")),
    svg(FIRMWARE_SETTINGS, "M8 1.5l6 3-6 3-6-3z\" fill=\"#4b79c1\"", "Save as new image", Place::Missing(TOOLBAR_ONLY_MACHINE)),
    svg(FIRMWARE_SETTINGS, "M8 7v5M5.5 9.5h5", "Create image", Place::Missing(TOOLBAR_ONLY_MACHINE)),
    svg(FIRMWARE_SETTINGS, "M5 14h6M8 11v3", "a machine", Place::Painted(Icon::Machine, "tree Machines, machine rows, the machine details heading")),
    svg(FIRMWARE_SETTINGS, "M8 1.5l6 3-6 3-6-3z\" fill=\"none\"", "an image", Place::Painted(Icon::Image, "tree Images, image rows, the image details heading")),
    svg(FIRMWARE_SETTINGS, "x=\"2.5\" y=\"1.5\" width=\"11\" height=\"13\"", "an installation media set", Place::Painted(Icon::Media, "tree Installation media, media set rows")),
    svg(FIRMWARE_SETTINGS, "M6 1v2.5M10 1v2.5", "a firmware package", Place::Painted(Icon::Firmware, "tree Firmware, firmware package rows")),
    svg(FIRMWARE_SETTINGS, "M5 7h6M5 9.5h6M5 12h4", "a machine file", Place::Painted(Icon::MachineFile, "tree Machine files, machine file rows")),
    glyph(FIRMWARE_SETTINGS, '↑', "a newer version of the firmware", Place::Text("the Newer version available hint and the quiet newer BIOS badge of machine and image rows")),
    glyph(FIRMWARE_SETTINGS, 'ⓘ', "settings of a machine or a new image revision", Place::Painted(Icon::Info, "the banner of Settings… on a machine or an image")),
    glyph(FIRMWARE_SETTINGS, '⚠', "CMOS settings are reset", Place::Text("the Firmware tab's CMOS line for another CMOS layout")),
    glyph(FIRMWARE_SETTINGS, '✓', "the same CMOS layout, a free tag", Place::Text("the Firmware tab's CMOS line, the revision's tag check")),
    glyph(FIRMWARE_SETTINGS, '●', "a setting that differs from the image", Place::Text("Settings… tab labels and the differing field's hint")),
    glyph(FIRMWARE_SETTINGS, '★', "a favourite", Place::Text("after a favourite machine's or image's name in its row and its details heading")),
    glyph(FIRMWARE_SETTINGS, '−', "an expanded repository group", Place::Text("repository group rows")),
    glyph(FIRMWARE_SETTINGS, '×', "close", Place::Text(CLOSE)),
    glyph(FIRMWARE_SETTINGS, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // The Remotes mock-up: browsing configured registries from the library.
    svg(REMOTES, "fill=\"#c9d6e8\" stroke=\"#0b1a33\"", "application", Place::Painted(Icon::App, "classic title strip")),
    svg(REMOTES, "M4 2l10 6-10 6z", "Start", Place::Painted(Icon::Play, "toolbar Start")),
    svg(REMOTES, "M8 3v10M4 9l4 4 4-4", "Pull", Place::Painted(Icon::Pull, "toolbar Pull…, while the library shows Remotes: the tags selected there")),
    svg(REMOTES, "M13 8a5 5 0 1 1-1.5-3.6", "Refresh a registry listing", Place::Painted(Icon::Refresh, "toolbar Refresh, which lists the shown registry again")),
    svg(REMOTES, "M5 4.5h1M5 11.5h1", "a registry", Place::Painted(Icon::Registry, "Remotes tree: each registry; toolbar Registries…")),
    svg(REMOTES, "M5 14h6M8 11v3", "a machine", Place::Painted(Icon::Machine, "tree Machines, machine rows, the machine details heading")),
    svg(REMOTES, "M8 1.5l6 3-6 3-6-3z\" fill=\"none\"", "an image", Place::Painted(Icon::Image, "tree Images, image rows, the image details heading")),
    svg(REMOTES, "x=\"2.5\" y=\"1.5\" width=\"11\" height=\"13\"", "an installation media set", Place::Painted(Icon::Media, "tree Installation media, media set rows")),
    svg(REMOTES, "M6 1v2.5M10 1v2.5", "a firmware package", Place::Painted(Icon::Firmware, "tree Firmware, firmware package rows")),
    svg(REMOTES, "M3 1.5h7l3 3v10H3z", "a machine file", Place::Painted(Icon::MachineFile, "tree Machine files, machine file rows")),
    svg(REMOTES, "M1.5 3.5h5l1.5 1.5h6.5v8.5h-13z", "a registry namespace", Place::Painted(Icon::Namespace, "Remotes tree: each namespace")),
    svg(REMOTES, "class=\"ico lock\"", "a private namespace before login", Place::Painted(Icon::Lock, "Remotes tree: after a private namespace until a login lists it")),
    svg(REMOTES, "M13 13l14 14M27 13L13 27", "a registry that cannot be reached", Place::Painted(Icon::Unreachable, "Remotes list: the message of an unreachable registry or a server error")),
    svg(REMOTES, "M14 18v-5a6 6 0 0 1 12 0v5", "a registry that asks for a login", Place::Painted(Icon::LoginRequired, "Remotes list: the login required message")),
    svg(REMOTES, "M4 12h12l3 3h17v19H4z", "an empty registry namespace", Place::Painted(Icon::EmptyRegistry, "Remotes list: the message of a registry with nothing published")),
    svg(REMOTES, "M20 10v10l7 4", "a registry listing in progress", Place::Painted(Icon::Listing, "Remotes list: the message of a listing before its first row")),
    glyph(REMOTES, '⚠', "a registry or tag that needs attention", Place::Text("Remotes strips of a stopped or stale listing, and the Local state of a tag whose local name holds another image")),
    glyph(REMOTES, '✓', "a pulled tag", Place::Text("Remotes Pull… results")),
    glyph(REMOTES, '−', "an expanded repository group", Place::Text("repository group rows")),
    glyph(REMOTES, '▲', "sorted ascending", Place::Text("sortable column headers")),
    glyph(REMOTES, '▼', "sorted descending", Place::Text("sortable column headers")),
    glyph(REMOTES, '⌘', "the macOS Command key", Place::Text("macOS menu shortcuts")),
    glyph(REMOTES, '×', "close", Place::Text(CLOSE)),
    glyph(REMOTES, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
    // player.html: the window of a machine opened from a file.
    svg(PLAYER, "fill=\"#c9d6e8\" stroke=\"#0b1a33\"", "application", Place::Missing(PLAYER_LATER_SAME_ART)),
    svg(
        PLAYER,
        "aria-label=\"Test pattern drawn for this mock-up\"",
        "the emulated screen, a test pattern the mock-up's script draws",
        Place::Missing("a stand-in for the machine's own picture, which the player shows in its place"),
    ),
    svg(PLAYER, "M4 2l10 6-10 6z", "Start, and Resume", Place::Missing(PLAYER_LATER_SAME_ART)),
    svg(PLAYER, "x=\"9.5\" y=\"2\" width=\"3.5\"", "Pause", Place::Missing(PLAYER_LATER_SAME_ART)),
    svg(PLAYER, "width=\"10\" height=\"10\" fill=\"#b23a2b\"", "Stop", Place::Missing(PLAYER_LATER_SAME_ART)),
    svg(PLAYER, "fill=\"#3a3f4a\"", "insert into A: or B:", Place::Missing(PLAYER_LATER_SAME_ART)),
    svg(PLAYER, "M8 1v3M8 12v3", "Settings", Place::Missing(PLAYER_LATER_SAME_ART)),
    svg(PLAYER, "M8 2l6 3-6 3-6-3z\" fill=\"#4b79c1\"", "Disk layers", Place::Missing(PLAYER_LATER_SAME_ART)),
    svg(
        PLAYER,
        "M13 8a5 5 0 1 1-1.5-3.5",
        "Refresh",
        Place::Missing("the player window is built in a later step, in crates/player-ui; its arrow head differs from the Remotes mock-up's Refresh"),
    ),
    svg(
        PLAYER,
        "x=\"2\" y=\"2.5\" width=\"12\" height=\"4\"",
        "Registries",
        Place::Missing("the player window is built in a later step, in crates/player-ui; its drive bays differ from the Remotes mock-up's registry"),
    ),
    glyph(PLAYER, '⏏', "Eject, on the toolbar and in the Media menu", Place::Missing(PLAYER_LATER)),
    glyph(PLAYER, 'Ⅱ', "Pause in the Machine menu", Place::Missing(PLAYER_LATER)),
    glyph(PLAYER, '■', "Stop in the Machine menu", Place::Missing(PLAYER_LATER)),
    glyph(PLAYER, '●', "the disk in a slot, the chosen scale, the chosen speed", Place::Missing(PLAYER_LATER)),
    glyph(PLAYER, '✓', "View › Toolbar, Status bar and Show overscan are on", Place::Missing(PLAYER_LATER)),
    glyph(PLAYER, '▸', "a submenu", Place::Missing(PLAYER_LATER)),
    glyph(PLAYER, '▾', "the file type and speed drop-downs", Place::Missing(PLAYER_LATER)),
    glyph(PLAYER, '✕', "a file the player refuses to open", Place::Missing(PLAYER_LATER)),
    glyph(PLAYER, '🔊', "audio playing or stopped", Place::Missing(PLAYER_LATER)),
    glyph(PLAYER, '🔇', "audio muted", Place::Missing(PLAYER_LATER)),
    glyph(PLAYER, '×', "close, and the times sign of picture sizes", Place::Missing(PLAYER_LATER)),
    glyph(PLAYER, '□', "maximise", Place::Missing(SYSTEM_TITLE_BAR)),
];

/// Characters the app paints instead of writing, because no font has them.
pub const PAINTED_ONLY: &[char] = &['ⓘ'];

/// The Question mark is the save-as-image mock-up's "?" disc: an ASCII "?" on
/// a styled span, so the scan of non-ASCII characters cannot find it.
pub const ASCII_MARKS: &[(&str, &str, Icon)] = &[(
    SAVE,
    "<span class=\"icon q\" aria-hidden=\"true\">?</span>",
    Icon::Question,
)];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn mockups() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../backlog/docs/design/mockups");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|e| e == "html"))
            .collect();
        files.sort();
        assert_eq!(files.len(), 14, "the approved mock-ups in {dir:?}");
        files
            .into_iter()
            .map(|path| {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                (name, std::fs::read_to_string(&path).unwrap())
            })
            .collect()
    }

    fn svgs(html: &str) -> Vec<&str> {
        let mut found = Vec::new();
        let mut rest = html;
        while let Some(start) = rest.find("<svg") {
            let end = rest[start..].find("</svg>").expect("an unclosed svg") + start + 6;
            found.push(&rest[start..end]);
            rest = &rest[end..];
        }
        found
    }

    /// Every SVG and every non-typographic character of every approved
    /// mock-up has an entry, and every entry names something its mock-up has.
    #[test]
    fn the_inventory_covers_every_icon_of_every_mockup() {
        let mut problems = Vec::new();
        for (name, html) in mockups() {
            let entries: Vec<_> = INVENTORY.iter().filter(|e| e.mockup == name).collect();
            for svg in svgs(&html) {
                let matches = entries
                    .iter()
                    .filter(|e| matches!(e.art, Art::Svg(key) if svg.contains(key)))
                    .count();
                if matches != 1 {
                    problems.push(format!("{name}: {matches} entries for {svg}"));
                }
            }
            let chars: BTreeSet<char> = html.chars().filter(|c| !c.is_ascii()).collect();
            for ch in &chars {
                let listed = TYPOGRAPHY.contains(*ch)
                    || entries
                        .iter()
                        .any(|e| matches!(e.art, Art::Glyph(g) if g == *ch));
                if !listed {
                    problems.push(format!("{name}: no entry for U+{:04X} {ch}", *ch as u32));
                }
            }
            for entry in &entries {
                let present = match entry.art {
                    Art::Svg(key) => svgs(&html).iter().any(|svg| svg.contains(key)),
                    Art::Glyph(ch) => chars.contains(&ch),
                };
                if !present {
                    problems.push(format!("{name}: stale entry for {}", entry.meaning));
                }
            }
            for (mockup, markup, _) in ASCII_MARKS {
                if *mockup == name && !html.contains(markup) {
                    problems.push(format!("{name}: no {markup}"));
                }
            }
        }
        for entry in INVENTORY {
            let where_or_why = match entry.place {
                Place::Painted(_, at) | Place::Text(at) | Place::Egui(at) => at,
                Place::Missing(reason) => reason,
            };
            assert!(!entry.meaning.is_empty() && !where_or_why.is_empty());
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// Every icon the module paints stands for a mock-up icon, and every
    /// mock-up character has a glyph in the fonts the desktop installs.
    #[test]
    fn every_painted_icon_and_mockup_glyph_is_drawable() {
        let painted: BTreeSet<_> = INVENTORY
            .iter()
            .filter_map(|e| match e.place {
                Place::Painted(icon, _) => Some(icon as usize),
                _ => None,
            })
            .chain(ASCII_MARKS.iter().map(|(_, _, icon)| *icon as usize))
            .collect();
        let all: BTreeSet<_> = Icon::ALL.iter().map(|icon| *icon as usize).collect();
        assert_eq!(
            painted, all,
            "icons without a mock-up, or mock-up icons without a painter"
        );
        let mut chars: BTreeSet<char> = TYPOGRAPHY.chars().collect();
        // Every mock-up character has a glyph, also those of screens the app
        // does not have yet, except the ones it paints and never writes.
        for entry in INVENTORY {
            if let Art::Glyph(ch) = entry.art {
                if PAINTED_ONLY.contains(&ch) {
                    assert!(
                        !matches!(entry.place, Place::Text(_) | Place::Egui(_)),
                        "{ch} is painted, never written"
                    );
                } else {
                    chars.insert(ch);
                }
            }
        }
        super::super::with_fonts(|fonts| {
            for ch in chars {
                for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    assert!(
                        fonts.has_glyph(&egui::FontId::new(14.0, family.clone()), ch),
                        "no {family:?} glyph for U+{:04X} {ch}",
                        ch as u32
                    );
                }
            }
        });
    }
}
