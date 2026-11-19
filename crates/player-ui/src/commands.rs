// SPDX-License-Identifier: AGPL-3.0-or-later
//! The player's command vocabulary: what its menus push and what the
//! application's hotkeys and toolbar ask of a running player
//! (core-contract.md §13.3, "Beside the PC's path"). It is the player's own;
//! the application's commands for the library and the PC are another list.

use machine_view::pacing::Speed;
use unemu_core_api::SlotId;

/// One thing a person asks of the running machine or its view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Pause, or Resume a paused machine (Ctrl+Alt+P).
    Pause,
    /// A soft reset (Ctrl+Alt+End), for a machine that lists one.
    Reset,
    /// A hard reset, or the machine started again from its file.
    PowerCycle,
    /// Finish the machine and return to the library (Ctrl+Alt+Q).
    Stop,
    /// Save the machine's state into the content's quick slot (Ctrl+Alt+S),
    /// for a machine that lists saved state.
    SaveState,
    /// Load the quick slot's state (Ctrl+Alt+L), once one was saved.
    LoadState,
    /// Machine › Input…: the Input dialog, for a machine with pads.
    Input,
    /// Eject the medium of a removable slot (Ctrl+Alt+E for the first slot,
    /// with Shift for the second).
    Eject(SlotId),
    /// The slot's next catalogue entry, wrapping to the first (Ctrl+Alt+F,
    /// with Shift for the second slot).
    Next(SlotId),
    /// A catalogue entry of the slot, by its place in the catalogue.
    Insert(SlotId, usize),
    /// View › Scale to window: the picture as large as the panel allows.
    ScaleToWindow,
    /// View › Integer scale: a whole multiple of the rows shown.
    IntegerScale,
    /// View › Show overscan, on or off.
    ShowOverscan,
    /// View › Full screen (Ctrl+Alt+Enter), which the application carries
    /// out.
    Fullscreen,
    /// Mute or unmute the machine's sound (Ctrl+Alt+M).
    Mute,
    /// View › Speed.
    Speed(Speed),
    /// Hold for Unlimited (Ctrl+Alt+T): held, or let go.
    FastForward(bool),
    /// Give a captured pointer back to the host (Ctrl+Alt+G).
    Release,
}

/// A menu shortcut as the host writes it: the application's hotkeys use
/// Command on macOS where the other hosts use Ctrl+Alt.
pub(crate) fn shortcut_text(shortcut: &'static str, macos: bool) -> &'static str {
    if !macos {
        return shortcut;
    }
    match shortcut {
        "Ctrl+Alt+P" => "⌘P",
        "Ctrl+Alt+T" => "⌘T",
        "Ctrl+Alt+Q" => "⌘K",
        "Ctrl+Alt+S" => "⌘S",
        "Ctrl+Alt+L" => "⌘L",
        "Ctrl+Alt+End" => "⌘End",
        "Ctrl+Alt+Enter" => "⌘Enter",
        "Ctrl+Alt+M" => "⌘⇧M",
        "Ctrl+Alt+F" => "⌘F",
        "Ctrl+Alt+Shift+F" => "⌘⇧F",
        "Ctrl+Alt+E" => "⌘E",
        "Ctrl+Alt+Shift+E" => "⌘⇧E",
        _ => shortcut,
    }
}

/// The media keys of the first two removable slots, as the session takes
/// them: Eject, then Next in catalogue.
pub(crate) fn slot_shortcuts(index: usize) -> (&'static str, &'static str) {
    match index {
        0 => ("Ctrl+Alt+E", "Ctrl+Alt+F"),
        1 => ("Ctrl+Alt+Shift+E", "Ctrl+Alt+Shift+F"),
        _ => ("", ""),
    }
}
