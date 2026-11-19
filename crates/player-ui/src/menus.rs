// SPDX-License-Identifier: AGPL-3.0-or-later
//! The player's Machine, Media and View menus, built from the running
//! machine's description as the player mock-up draws them
//! (`backlog/docs/design/mockups/player.html`, "Present or absent, not
//! greyed", "Slot menu", "View" and "Keys").
//!
//! Each menu is first a list of [`Entry`]s, then drawn: what a menu holds is
//! decided in one place, which the tests read, and the drawing only follows
//! it. An item the description does not offer is not in the list; one that
//! is offered and cannot act now (Eject of an empty slot) is greyed and says
//! why when hovered.

use crate::commands::{shortcut_text, slot_shortcuts};
use crate::{Command, Condition, Player};
use machine_view::pacing::Speed;
use ui_kit::skin::EJECT_MARK;

/// The mark of the medium in a slot, the chosen scale and the chosen speed.
pub(crate) const CHOSEN_MARK: &str = "●";
/// The mark of a view option that is on.
pub(crate) const ON_MARK: &str = "✓";
/// The marks of Pause and Stop in the Machine menu.
pub(crate) const PAUSE_MARK: &str = "Ⅱ";
pub(crate) const STOP_MARK: &str = "■";

/// The View menu's parts that belong to the application's window, not to
/// the machine: the toolbar, the status bar and the skin.
pub struct ViewChrome<'a> {
    pub toolbar: &'a mut bool,
    pub status_bar: &'a mut bool,
    pub classic: &'a mut bool,
}

/// One line of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    /// A greyed line that says what the menu acts on.
    Note(String),
    /// A slot's heading in the Media menu: its label and what is in it.
    Heading(String),
    Item(Item),
    Separator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    /// The mark column: a glyph, or "" to keep the labels aligned.
    pub mark: &'static str,
    pub label: String,
    pub shortcut: &'static str,
    /// `None` for an item of the application's that is greyed while a
    /// machine runs.
    pub command: Option<Command>,
    pub enabled: bool,
    /// Why a greyed item cannot act now, in one short phrase.
    pub why: Option<String>,
}

impl Item {
    fn new(
        mark: &'static str,
        label: impl Into<String>,
        shortcut: &'static str,
        command: Command,
    ) -> Self {
        Self {
            mark,
            label: label.into(),
            shortcut,
            command: Some(command),
            enabled: true,
            why: None,
        }
    }

    /// Greyed unless `enabled`, saying `why`.
    fn unless(mut self, enabled: bool, why: impl Into<String>) -> Self {
        if !enabled {
            self.enabled = false;
            self.why = Some(why.into());
        }
        self
    }
}

const NOT_RUNNING: &str = "The machine has stopped";
const HALTED: &str = "The machine waits for a reset";

impl Player {
    /// The Machine menu.
    pub(crate) fn machine_entries(&self) -> Vec<Entry> {
        let live = self.session.as_ref().is_some_and(|s| s.takes_media());
        let condition = self.condition();
        let mut entries = vec![
            Entry::Note(format!(
                "For the machine: {} ({})",
                self.prepared.title(),
                condition.word().to_lowercase()
            )),
            Entry::Item(
                Item::new(
                    PAUSE_MARK,
                    if self.paused { "Resume" } else { "Pause" },
                    "Ctrl+Alt+P",
                    Command::Pause,
                )
                .unless(
                    self.running(),
                    if condition == Condition::Halted {
                        HALTED
                    } else {
                        NOT_RUNNING
                    },
                ),
            ),
        ];
        if self.offers.reset {
            entries.push(Entry::Item(
                Item::new("", "Reset", "Ctrl+Alt+End", Command::Reset).unless(live, NOT_RUNNING),
            ));
        }
        entries.push(Entry::Item(Item::new(
            "",
            "Power cycle",
            "",
            Command::PowerCycle,
        )));
        entries.push(Entry::Item(Item::new(
            STOP_MARK,
            "Stop",
            "Ctrl+Alt+Q",
            Command::Stop,
        )));
        if self.offers.input {
            entries.push(Entry::Separator);
            entries.push(Entry::Item(
                Item::new("", "Input…", "", Command::Input)
                    .unless(self.session.is_some(), NOT_RUNNING),
            ));
        }
        entries
    }

    /// The Media menu: one section per removable slot, or no menu.
    pub(crate) fn media_entries(&self) -> Option<Vec<Entry>> {
        if self.slots.is_empty() {
            return None;
        }
        let live = self.session.as_ref().is_some_and(|s| s.takes_media());
        let mut entries = Vec::new();
        for (index, view) in self.slots.iter().enumerate() {
            if index > 0 {
                entries.push(Entry::Separator);
            }
            let inserted = view.inserted();
            entries.push(Entry::Heading(format!(
                "{} — {}",
                view.label,
                inserted.unwrap_or("empty")
            )));
            let state = view.state.as_ref();
            let accepts = live && state.is_some_and(|state| state.accepts_changes);
            let refused = if live {
                "The machine takes no media change now"
            } else {
                NOT_RUNNING
            };
            let catalogue = state.map_or(&[][..], |state| &state.catalogue[..]);
            let (eject_key, next_key) = slot_shortcuts(index);
            let eject = Item::new(EJECT_MARK, "Eject", eject_key, Command::Eject(view.id));
            entries.push(Entry::Item(if inserted.is_none() {
                eject.unless(
                    false,
                    format!("Nothing is in the {}", view.label.to_lowercase()),
                )
            } else {
                eject.unless(accepts, refused)
            }));
            let next = Item::new("", "Next in catalogue", next_key, Command::Next(view.id));
            entries.push(Entry::Item(if catalogue.is_empty() {
                next.unless(false, "The catalogue is empty")
            } else {
                next.unless(accepts, refused)
            }));
            if !catalogue.is_empty() {
                entries.push(Entry::Separator);
            }
            for (entry, medium) in catalogue.iter().enumerate() {
                let in_slot = state.and_then(|state| state.inserted) == Some(entry);
                entries.push(Entry::Item(
                    Item::new(
                        if in_slot { CHOSEN_MARK } else { "" },
                        medium.name.clone(),
                        "",
                        Command::Insert(view.id, entry),
                    )
                    .unless(accepts, refused),
                ));
            }
        }
        Some(entries)
    }

    /// The View menu's items between the window's toggles and the Speed,
    /// Skin and Theme submenus.
    pub(crate) fn view_entries(&self) -> Vec<Entry> {
        let mut entries = vec![
            Entry::Item(Item::new(
                if self.integer_scale { "" } else { CHOSEN_MARK },
                "Scale to window",
                "",
                Command::ScaleToWindow,
            )),
            Entry::Item(Item::new(
                if self.integer_scale { CHOSEN_MARK } else { "" },
                "Integer scale",
                "",
                Command::IntegerScale,
            )),
        ];
        if self.offers().show_overscan {
            entries.push(Entry::Item(Item::new(
                if self.show_overscan { ON_MARK } else { "" },
                "Show overscan",
                "",
                Command::ShowOverscan,
            )));
        }
        entries.push(Entry::Item(Item::new(
            "",
            "Full screen",
            "Ctrl+Alt+Enter",
            Command::Fullscreen,
        )));
        // The application's command for a registry listing, greyed while a
        // machine runs, so View keeps its words for every machine.
        entries.push(Entry::Item(Item {
            mark: "",
            label: "Refresh remote".into(),
            shortcut: "F5",
            command: None,
            enabled: false,
            why: None,
        }));
        let mute = self
            .session
            .as_ref()
            .map_or("Mute audio", |session| session.audio.mute_label());
        entries.push(Entry::Item(Item::new(
            "",
            mute,
            "Ctrl+Alt+M",
            Command::Mute,
        )));
        entries
    }

    /// View › Speed: the four speeds, the chosen one marked.
    pub(crate) fn speed_entries(&self) -> Vec<Entry> {
        let selected = self
            .session
            .as_ref()
            .map_or(Speed::Normal, |session| session.pacing.selected());
        let mut entries: Vec<Entry> = Speed::ALL
            .into_iter()
            .map(|speed| {
                Entry::Item(Item::new(
                    if speed == selected { CHOSEN_MARK } else { "" },
                    speed.label(),
                    "",
                    Command::Speed(speed),
                ))
            })
            .collect();
        entries.push(Entry::Separator);
        // Held, not clicked: the application's hotkey works it.
        entries.push(Entry::Item(Item {
            mark: "",
            label: "Hold for Unlimited".into(),
            shortcut: "Ctrl+Alt+T",
            command: None,
            enabled: false,
            why: None,
        }));
        entries
    }

    /// The Machine menu, the Media menu when the machine has a removable
    /// slot, and the View menu, in the application's menu bar where the
    /// PC's Machine, Media and View menus stand. A click pushes its command.
    pub fn menus(&self, ui: &mut egui::Ui, chrome: ViewChrome<'_>, commands: &mut Vec<Command>) {
        let macos = self.macos_shortcuts;
        let machine = self.machine_entries();
        ui.menu_button("Machine", |ui| draw(ui, &machine, macos, commands));
        if let Some(media) = self.media_entries() {
            ui.menu_button("Media", |ui| draw(ui, &media, macos, commands));
        }
        let view = self.view_entries();
        let speeds = self.speed_entries();
        ui.menu_button("View", |ui| {
            ui.checkbox(chrome.toolbar, "Toolbar");
            ui.checkbox(chrome.status_bar, "Status bar");
            ui.separator();
            draw(ui, &view, macos, commands);
            ui.menu_button("Speed", |ui| draw(ui, &speeds, macos, commands));
            ui.menu_button("Skin", |ui| {
                ui.selectable_value(chrome.classic, true, "Classic");
                ui.selectable_value(chrome.classic, false, "Modern");
            });
            ui.menu_button("Theme", |ui| {
                egui::widgets::global_theme_preference_buttons(ui);
            });
        });
    }
}

/// Draw `entries` into an open menu.
fn draw(ui: &mut egui::Ui, entries: &[Entry], macos: bool, commands: &mut Vec<Command>) {
    for entry in entries {
        match entry {
            Entry::Note(text) => {
                ui.add_enabled(false, egui::Label::new(text));
            }
            Entry::Heading(text) => {
                ui.label(egui::RichText::new(text).strong());
            }
            Entry::Separator => {
                ui.separator();
            }
            Entry::Item(item) => {
                let button =
                    egui::Button::new((ui_kit::shell_icons::mark(item.mark), item.label.as_str()))
                        .shortcut_text(shortcut_text(item.shortcut, macos));
                let response = ui.add_enabled(item.enabled && item.command.is_some(), button);
                let response = match &item.why {
                    Some(why) => response.on_disabled_hover_text(why),
                    None => response,
                };
                if response.clicked() {
                    commands.extend(item.command);
                    ui.close();
                }
            }
        }
    }
}
