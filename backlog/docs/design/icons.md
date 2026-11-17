# Desktop icons

Where the desktop's icons come from, which mock-up icon the app draws where,
and which it does not. The approved mock-ups in [`mockups/`](mockups/) are the
reference; [machine images §7](machine-images.md#7-approved-desktop-behavior-and-technology)
covers the desktop as a whole. The authoritative, tested list is the
`inventory` module beside `crates/window/src/shell_icons.rs`: its tests read
every mock-up, so an icon added to a mock-up without an entry, or an entry
whose icon left its mock-up, fails the build.

## How icons are drawn

- **SVG artwork is painted.** Every inline SVG of the mock-ups uses a 16-unit
  view box (48 for the empty-library picture) and a handful of rectangles,
  lines and polygons. `shell_icons::paint` draws each with egui shapes from the
  SVG's own coordinates, so the build needs no SVG renderer or icon font. The
  SVGs' `currentColor` parts take the widget's text colour (white on a
  selection); their fixed fills keep the mock-up colours: Start `#2c8a3a`, Stop
  `#b23a2b`, the floppy `#3a3f4a`/`#c9cdd4`/`#e9e6dc`, the top layer
  `#4b79c1`, the application monitor `#c9d6e8`/`#0b1a33`/`#0b2a5a`. The
  empty-library picture takes the panel and selection colours the mock-up
  names (`--face`, `--sel`, `--sel-ink`).
- **Unicode marks stay text.** ★ ⚠ ✓ ✗ ▶ Ⅱ ■ ⏏ 🔊 🔇 ● ○ ↑ ↓ ▲ ▼ and the
  rest are drawn as the same characters. egui's bundled fonts (Ubuntu Light,
  Hack, Noto Emoji, emoji-icon-font) lacked ✓ U+2713, ✗ U+2717, ✕ U+2715,
  Ⅱ U+2161 and ⌫ U+232B, so the app drew ✓ and ✗ as empty boxes. A subset of
  DejaVu Sans with those five glyphs now follows egui's fonts in both the
  proportional and monospace families.
- **ⓘ is painted** (a circle and an "i"): none of the fonts has U+24D8.

Tests: every character in a string literal of the desktop crate has a glyph in
both families; every mock-up character has one (also those of screens not built yet), except ⓘ, which is painted; every
icon paints inside its square in both skins and both themes, its text-coloured
parts at 3:1 or more against the panel (WCAG 2.x non-text contrast).

## Font licence

`crates/window/assets/fonts/dejavu/DejaVuSans-UI-symbols.ttf` is cut from DejaVu
Sans 2.37 (Bitstream Vera and Arev Fonts licences, SPDX `Bitstream-Vera`;
DejaVu changes are public domain). The licence permits modified fonts under a
name without "Bitstream", "Vera", "Tavmjong Bah" or "Arev", so the subset keeps
the name "DejaVu Sans"; its name table keeps the copyright and licence records.
The release hash, the subset command and the output hash are in the folder's
`NOTICE.md`; the original `LICENSE` is beside it. The `window` crate's
`license` field names `Bitstream-Vera`, which `deny.toml` allows for that crate
only, and About, `--help` and `--version` carry the attribution. The painted
icons are the project's own mock-up artwork, under the project licence.

## Where each mock-up icon goes

| Mock-up | Icon | The app |
|---|---|---|
| main-window | application monitor | classic title strip (Windows and Linux; macOS uses its native title bar) |
| main-window, library, machines, media-picker | Start ▶ green, Pause, Stop red, A: and B: floppy, Eject, Settings gear, Layers | toolbar; Resume reuses Start's triangle; A: and B: open the media picker; Ctrl+Alt+Del is code text without an icon, as in the mock-up |
| main-window | ▶ Ⅱ ■ | Machine menu mark column (Start, Pause/Resume, Stop); every Machine menu item keeps the column |
| main-window | ⏏ | Media › Eject A:/B: and the floppy drive menus |
| main-window | ⚠ after a name | rows of unreadable images, firmware packages, media sets and machine files with a problem (replaces the old `[!]` prefix) |
| main-window | 🔊 / 🔇 | status bar audio cell, in place of the word "Audio" (`🔊 on`, `🔇 muted (user)`, `🔇 unavailable`), so the text keeps its width; 🔇 while muted or no sound device is active |
| machines, media-picker | machine (a monitor on a stand) | tree Machines, machine rows, a machine's details heading |
| machines | image, media set, firmware, machine file | tree collections (Images, Installation media, Firmware, Machine files) and their rows; the image icon heads an image's details |
| machines | ★ | after a favourite machine's or image's name, in its row and its details heading |
| machines | ⚑ tag moved | the Source image badge of a machine whose tag now names another image, in its row and details |
| machines | ● differs from the image | Settings… on a machine: the hint of a differing field and its tab label |
| machines | ⓘ | the migration notice and the banner of Settings… on a machine (painted, as everywhere) |
| machines | ⌘ ⇧ ⌫ | macOS menu shortcuts, ⌫ for Delete machine… and Delete tag… |
| machines | ⚠ unfinished session | before the library's Unfinished sessions heading |
| machines, new-machine | ✓ finished step | New machine wizard step list ("!" still marks a step that needs attention) |
| new-machine, save-as-image | ✓ free tag | the tag check line of New machine… and Save as new image… |
| save-as-image, firmware | ⓘ | the notes of image Settings… and Edit metadata… |
| save-as-image | ? disc, ✕ disc | Move tag? and Version tag already exists |
| get-templates | monitor over a keyboard | the empty library card |
| media-picker | application monitor, Start, Stop, A: and B: floppy, machine, installation media set | the same places as above; ⌘ ⇧ in macOS menu shortcuts |
| media-picker | ▾ | Installation media › Insert into… ▾ menu button |
| media-picker | ↑ ↓ | the picker's key note (↑↓ choose) |
| media-picker | ▸ ● | the Catalogue submenu under each floppy drive in the Media menu and a drive chip's menu (revision 3): egui's submenu arrow draws ▸, enabled or not, and the submenu's mark column writes ● before the disk in the drive |
| registry-accounts, get-templates, firmware, save-as-image | ● ○ ✓ ✗ ⚠ ▲ ▼ ↑ ↓ └ ─ | already in the app's text; ✓ and ✗ are visible since the font subset |
| firmware-settings | application monitor, Start, Settings, machine, image, media set, firmware, machine file | the same places as in the machines mock-up |
| firmware-settings | ↑ | the Newer version available hint and the quiet "↑ newer BIOS" badge of machine and image rows |
| firmware-settings | ⓘ ⚠ ✓ ● | the Settings… banner, the Firmware tab's CMOS line (⚠ another layout, ✓ the same) and changed tab labels |
| remotes | application monitor, Start, machine, image, media set, firmware, machine file | the same places as in the machines mock-up; − ▲ ▼ ⌘ as in the library lists and menus |
| remotes | registry, namespace, lock | the Remotes tree: each registry, each namespace, and the lock after a private namespace until a login lists it; the registry icon also on the toolbar's Registries… |
| remotes | Refresh | the toolbar's Refresh, which lists the shown registry again |
| remotes | Pull | the toolbar's Pull…, while the library shows Remotes: the tags selected there |
| remotes, get-templates | ✓ ✗ | the stage of each tag in Remotes' Pull… (Pulled ✓, Failed ✗) |
| remotes | the four message pictures (unreachable, login, empty, listing) | the Remotes list's message instead of the rows, 40 px |
| remotes | ⚠ | the strips of a stopped or stale listing, and the Local state of a tag whose local name holds another image |

The name column of the library lists is now left-aligned: a centred egui label
drops the space the icon and a group's indent need, and the mock-ups'
name cells are left-aligned. Child rows of a repository group are set in by
16 points, which the old leading spaces never did in a centred cell.

## Not drawn, with the reason

- **library.html toolbar** (New, Properties, Browse registry…, Push, Delete) and
  **machines.html toolbar** (Gaming mode, New machine from image…, Save as new
  image…): the app has one toolbar, the main-window mock-up's machine toolbar,
  and these commands are not on it (they are in the menus and the details
  panes). Run, Start, Stop and Settings share the toolbar's icons. The
  firmware-settings mock-up's Save as new image… and Create image… toolbar
  buttons are not on it either.
- **main-window welcome list monitor**: the welcome list holds images and
  machine files, which take the machines mock-up's own icons.
- **View menu ✓ and ●**: egui's check boxes (Toolbar, Status bar) and selection
  highlight (scale modes) show the state; Mute audio says Unmute audio instead.
  Their accessibility state ("toggled") comes from those widgets.
- **A drive menu's ● write mode**: the drive menu names the write mode as text
  (a departure accepted on 2026-11-15).
- **library.html ★ rating**: the app has no rating field.
- **□ maximise in title bars**: the system title bar draws window buttons.
- **library.html ▸**: only in the mock-up's explanatory note.
- **media-picker.html ←**: only in the Catalogue notes, naming the Left arrow key.
