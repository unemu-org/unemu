// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tools › Preferences…: a tabbed dialog whose Registries page follows the
//! approved registry-accounts mock-up; `backlog/docs/design/machine-images.md`
//! §5.1 and §7. Rendering only: every decision lives in
//! [`crate::registries_model`].

use crate::registries_model::{
    namespaces_text, secret_hint, store_description, Mark, RegistriesPage, SignIn, TestState,
};
use image_registry::credential_store::CredentialStore;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    General,
    Registries,
}

pub(crate) struct PreferencesDialog {
    pub tab: Tab,
    pub registries: RegistriesPage,
    /// General › the macOS system-shortcut option; `None` on other hosts.
    pub system_shortcuts: Option<SystemShortcuts>,
}

/// The opt-in that hands macOS system shortcuts to the guest.
pub(crate) struct SystemShortcuts {
    pub on: bool,
    /// What macOS said about the Accessibility privilege when it was turned on.
    pub note: Option<String>,
}

/// Why the option asks for Accessibility access, in one sentence.
pub(crate) const SYSTEM_SHORTCUTS_WHY: &str = "macOS takes Ctrl+arrows, Ctrl+digits, \
    Ctrl+F1–F8, Ctrl+Space and F11 for Mission Control, Spaces and keyboard navigation \
    before any app sees them, so passing them to the guest needs Accessibility access \
    for unemupc; Command shortcuts always stay with macOS.";

impl PreferencesDialog {
    pub fn new(path: Option<PathBuf>, store: Arc<dyn CredentialStore>) -> Self {
        Self {
            tab: Tab::Registries,
            registries: RegistriesPage::new(
                path,
                store,
                image_registry::credentials::EnvCredentials::from_env(),
            ),
            system_shortcuts: None,
        }
    }
}

pub(crate) enum Action {
    None,
    Close,
    /// Open the ROM folders and image store dialog instead.
    FirmwareFolders,
    /// The system-shortcut check box changed; it applies at once.
    SystemShortcuts(bool),
}

pub(crate) fn show(ctx: &egui::Context, dialog: &mut PreferencesDialog, classic: bool) -> Action {
    let page = &mut dialog.registries;
    if page.poll() {
        ctx.request_repaint();
    }
    if page.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    let mut action = Action::None;
    egui::Modal::new(egui::Id::unique("preferences")).show(ctx, |ui| {
        ui.set_width(720.0);
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 30.0).max(100.0);
            ui.allocate_ui(egui::vec2(width, 23.0), |ui| {
                if classic {
                    ui_kit::skin::gradient_title(ui, "Preferences");
                } else {
                    ui.heading("Preferences");
                }
            });
            if ui
                .add_sized([23.0, 23.0], egui::Button::new("×"))
                .on_hover_text("Close")
                .clicked()
            {
                action = Action::Close;
            }
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            for (tab, label) in [(Tab::General, "General"), (Tab::Registries, "Registries")] {
                let response = ui.selectable_label(dialog.tab == tab, label);
                if classic {
                    ui_kit::skin::bevel(ui, response.rect, dialog.tab == tab);
                }
                if response.clicked() {
                    dialog.tab = tab;
                }
            }
        });
        egui::Frame::group(ui.style())
            .corner_radius(0)
            .show(ui, |ui| {
                ui.set_min_height(300.0);
                match dialog.tab {
                    Tab::General => {
                        ui.label(
                            "ROM folders and the image store folder are set in their own dialog.",
                        );
                        if ui.button("ROM folders and image store…").clicked() {
                            action = Action::FirmwareFolders;
                        }
                        if let Some(shortcuts) = &dialog.system_shortcuts {
                            ui.separator();
                            let mut on = shortcuts.on;
                            if ui
                                .checkbox(
                                    &mut on,
                                    "Send macOS system shortcuts to the guest while it has the keyboard",
                                )
                                .changed()
                            {
                                action = Action::SystemShortcuts(on);
                            }
                            small(ui, SYSTEM_SHORTCUTS_WHY);
                            if let Some(note) = &shortcuts.note {
                                warning(ui, note.as_str());
                            }
                        }
                    }
                    Tab::Registries => registries_page(ui, &mut dialog.registries),
                }
            });
        let page = &dialog.registries;
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} of {} registries signed in",
                page.signed_in_count(),
                page.rows.len()
            ));
            ui.separator();
            ui.label(format!("Default registry: {}", page.default_host));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Every registry action applies at once, so there is nothing to apply.
            ui.add_enabled(false, egui::Button::new("Apply"));
            if ui.button("Cancel").clicked() {
                action = Action::Close;
            }
            if ui.button("OK").clicked() {
                action = Action::Close;
            }
        });
    });
    let page = &mut dialog.registries;
    editor_modal(ctx, page);
    login_modal(ctx, page);
    confirm_modal(ctx, page);
    if ctx.input_mut(ui_kit::skin::escape_pressed) {
        if page.confirm.is_some() {
            page.confirm_public(false);
        } else if page.login.is_some() {
            page.cancel_login();
            forget_password(ctx);
        } else if page.editor.is_some() {
            page.editor = None;
        } else {
            action = Action::Close;
        }
    }
    if !matches!(action, Action::None) {
        // Closing Preferences drops the page, which cancels a login in flight.
        page.cancel_login();
        forget_password(ctx);
    }
    action
}

/// The Log in dialog's password field, named so its state can be dropped.
pub(crate) fn password_field_id() -> egui::Id {
    egui::Id::unique("registry-login-password")
}

/// Drop the password field's editing state, whose undo history holds the
/// typed text, so Undo can never bring back a password, also not in the next
/// Log in dialog for another registry.
fn forget_password(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.remove::<egui::widgets::text_edit::TextEditState>(password_field_id())
    });
}

fn small(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(egui::RichText::new(text.into()).small()).wrap());
}

fn warning(ui: &mut egui::Ui, text: impl Into<String>) {
    let color = ui.visuals().warn_fg_color;
    ui.add(egui::Label::new(egui::RichText::new(text.into()).small().color(color)).wrap());
}

fn error(ui: &mut egui::Ui, text: &str) {
    let color = ui.visuals().error_fg_color;
    ui.add(egui::Label::new(egui::RichText::new(text).color(color)).wrap());
}

fn registries_page(ui: &mut egui::Ui, page: &mut RegistriesPage) {
    ui.add(
        egui::Label::new(format!(
            "Registries this emulator may pull from and push to. A short name such as open-at:1.0.0 means {}/unemu/open-at:1.0.0, the way Docker names resolve to Docker Hub.",
            page.default_host
        ))
        .wrap(),
    );
    let mut clicked = None;
    egui::Frame::group(ui.style())
        .corner_radius(0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("registry-rows")
                .max_height(150.0)
                .show(ui, |ui| {
                    egui::Grid::new("registry-table")
                        .num_columns(4)
                        .striped(true)
                        .min_col_width(90.0)
                        .show(ui, |ui| {
                            for heading in ["Registry / namespace", "User", "Signed in", "Private"]
                            {
                                ui.strong(heading);
                            }
                            ui.end_row();
                            for line in page.table() {
                                if ui
                                    .selectable_label(line.row == page.selected, line.name)
                                    .clicked()
                                {
                                    clicked = Some(line.row);
                                }
                                ui.label(line.user);
                                ui.label(line.signed_in);
                                ui.label(line.private);
                                ui.end_row();
                            }
                        });
                });
        });
    if let Some(index) = clicked {
        page.select(index);
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button("Add…").clicked() {
            page.open_add();
        }
        if ui
            .add_enabled(page.selected_row().is_some(), egui::Button::new("Edit…"))
            .clicked()
        {
            page.open_edit();
        }
        if ui
            .add_enabled(page.can_log_in(), egui::Button::new("Log in…"))
            .clicked()
        {
            page.open_login();
        }
        if ui
            .add_enabled(page.can_log_out(), egui::Button::new("Log out"))
            .clicked()
        {
            page.log_out_selected();
        }
        if ui
            .add_enabled(page.can_remove(), egui::Button::new("Remove"))
            .clicked()
        {
            page.remove_selected();
        }
        if ui
            .add_enabled(page.can_set_default(), egui::Button::new("Set as default"))
            .clicked()
        {
            page.set_default_selected();
        }
        if ui
            .add_enabled(page.can_test(), egui::Button::new("Test connection"))
            .clicked()
        {
            page.test_selected();
        }
    });
    if let Some(reason) = page.remove_blocked() {
        warning(ui, reason);
    }
    if let Some(row) = page.selected_row() {
        let kind = page.store_kind();
        egui::Grid::new("registry-details")
            .num_columns(2)
            .spacing([12.0, 3.0])
            .show(ui, |ui| {
                ui.label("Host");
                ui.label(&row.entry.host);
                ui.end_row();
                ui.label("Connection");
                ui.label(if row.entry.plain_http {
                    "Plain HTTP (test registry on this computer)"
                } else {
                    "HTTPS with TLS"
                });
                ui.end_row();
                ui.label("Namespaces");
                ui.add(egui::Label::new(namespaces_text(&row.entry)).wrap());
                ui.end_row();
                ui.label("Credentials");
                ui.add(
                    egui::Label::new(match (&row.sign_in, &row.entry.user) {
                        (SignIn::SignedIn, Some(user)) => {
                            format!("Signed in as {user} · secret in the {}", kind.name())
                        }
                        (SignIn::Checking, _) => "Checking…".to_owned(),
                        (SignIn::Unknown(reason), _) => {
                            format!("The credential store did not answer: {reason}")
                        }
                        _ => "None stored".to_owned(),
                    })
                    .wrap(),
                );
                ui.end_row();
                ui.label("Connection test");
                ui.vertical(|ui| match &row.test {
                    TestState::NotRun => {
                        ui.label("Not run yet");
                    }
                    TestState::Running => {
                        ui.label(format!("Contacting {}…", row.entry.host));
                    }
                    TestState::Failed(message) => error(ui, message),
                    TestState::Done(lines) => {
                        for line in lines {
                            let (mark, color) = match line.mark {
                                Mark::Good => ("✓", egui::Color32::from_rgb(31, 107, 51)),
                                Mark::Bad => ("✗", ui.visuals().error_fg_color),
                                Mark::Note => ("•", ui.visuals().text_color()),
                            };
                            ui.horizontal(|ui| {
                                ui.colored_label(color, mark);
                                ui.add(egui::Label::new(&line.text).wrap());
                            });
                        }
                    }
                });
                ui.end_row();
            });
    }
    small(ui, secret_hint(page.store_kind()));
    if let Some(message) = &page.error {
        error(ui, message);
    } else if let Some(message) = &page.status {
        ui.add(egui::Label::new(message).wrap());
    }
}

fn editor_modal(ctx: &egui::Context, page: &mut RegistriesPage) {
    let Some(editor) = &mut page.editor else {
        return;
    };
    let mut submit = false;
    let mut cancel = false;
    let title = match &editor.original {
        Some(host) => format!("Edit registry {host}"),
        None => "Add registry".to_owned(),
    };
    egui::Modal::new(egui::Id::unique("registry-editor")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading(title);
        egui::Grid::new("registry-editor-fields")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Host");
                ui.vertical(|ui| {
                    if editor.original.is_some() {
                        ui.label(&editor.host);
                    } else {
                        ui.add(
                            egui::TextEdit::singleline(&mut editor.host)
                                .hint_text("registry.example:5000"),
                        );
                        small(ui, "A host name with a dot or a port, or localhost.");
                    }
                });
                ui.end_row();
            });
        egui::Grid::new("registry-editor-namespaces")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Namespaces");
                ui.vertical(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut editor.namespaces)
                            .hint_text("empty = the whole registry"),
                    );
                    small(
                        ui,
                        "Comma-separated, such as unemu, private. The box below marks each listed namespace private; NAME:public or NAME:private sets one on its own. unemu/ is always public, open content only. A push into a namespace not listed here is refused.",
                    );
                });
                ui.end_row();
            });
        ui.checkbox(&mut editor.private, "Private registry");
        small(
            ui,
            "Images with user-supplied content may be pushed only to a private registry or namespace, never into unemu/. Before the first such push, anonymous access is probed and must be refused. There is no override.",
        );
        ui.checkbox(
            &mut editor.plain_http,
            "Allow plain HTTP (test registry on this computer only)",
        );
        small(
            ui,
            "TLS is on by default. A login is sent only over TLS or to a test registry on this computer.",
        );
        if let Some(message) = &editor.error {
            error(ui, message);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            let label = if editor.original.is_some() { "Save" } else { "Add" };
            if ui.button(label).clicked() {
                submit = true;
            }
        });
    });
    if cancel {
        page.editor = None;
    } else if submit {
        page.submit_editor();
    }
}

fn login_modal(ctx: &egui::Context, page: &mut RegistriesPage) {
    let kind = page.store_kind();
    let hint = page.login_hint();
    let Some(dialog) = &mut page.login else {
        return;
    };
    let mut submit = false;
    let mut cancel = false;
    egui::Modal::new(egui::Id::unique("registry-login")).show(ctx, |ui| {
        ui.set_width(480.0);
        ui.heading(format!("Log in to {}", dialog.host));
        egui::Grid::new("registry-login-fields")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Registry");
                ui.label(&dialog.host);
                ui.end_row();
                ui.label("User");
                ui.add_enabled(!dialog.busy, egui::TextEdit::singleline(&mut dialog.user));
                ui.end_row();
                ui.label("Password or token");
                let response = ui.add_enabled(
                    !dialog.busy,
                    egui::TextEdit::singleline(&mut dialog.password)
                        .id(password_field_id())
                        .password(true),
                );
                ui_kit::text_clipboard::mark_password(ui.ctx(), response.id);
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                ui.end_row();
            });
        if let Some(message) = &dialog.error {
            error(ui, message);
        }
        egui::Frame::group(ui.style())
            .corner_radius(0)
            .show(ui, |ui| {
                ui.label("Where the secret is kept");
                let (name, how) = store_description(kind);
                ui.add(egui::RadioButton::new(true, format!("{name}  [detected]")));
                small(ui, how);
                ui.add_enabled(
                    false,
                    egui::RadioButton::new(false, "Use a Docker credential helper (later)"),
                );
                small(
                    ui,
                    "Reusing the logins you already have for Docker is planned, not built.",
                );
            });
        small(ui, hint);
        if dialog.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Contacting {}…", dialog.host));
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui
                .add_enabled(!dialog.busy, egui::Button::new("Log in"))
                .clicked()
            {
                submit = true;
            }
        });
    });
    if cancel {
        page.cancel_login();
        forget_password(ctx);
    } else if submit {
        page.submit_login();
        forget_password(ctx);
    }
}

fn confirm_modal(ctx: &egui::Context, page: &mut RegistriesPage) {
    let Some(confirm) = &page.confirm else {
        return;
    };
    let mut answer = None;
    egui::Modal::new(egui::Id::unique("registry-confirm-public")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.heading(format!("Make {} public?", confirm.exposed));
        ui.add(
            egui::Label::new(format!(
                "Images with user-supplied content can then no longer be pushed to {}. Only a registry or namespace marked private may receive them; there is no override.",
                confirm.exposed
            ))
            .wrap(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
            if ui.button("Make public").clicked() {
                answer = Some(true);
            }
        });
    });
    if let Some(accept) = answer {
        page.confirm_public(accept);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registries_model::ConfirmPublic;
    use image_registry::{credential_store::SessionStore, credentials::EnvCredentials};

    fn texts(output: &egui::FullOutput) -> Vec<String> {
        fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            collect(&shape.shape, &mut out);
        }
        out
    }

    #[test]
    fn every_page_and_dialog_renders_without_a_window_and_masks_the_password() {
        let temp = assembly::test_support::TempDir::new("preferences-render");
        let path = temp.path().join("preferences.toml");
        std::fs::write(
            &path,
            "[[registries]]\nhost = \"nas.example\"\nprivate = true\nuser = \"me\"\n",
        )
        .unwrap();
        let mut dialog = PreferencesDialog {
            tab: Tab::Registries,
            registries: RegistriesPage::new(
                Some(path),
                Arc::new(SessionStore::default()),
                EnvCredentials::default(),
            ),
            system_shortcuts: None,
        };
        let ctx = egui::Context::default();
        let frame = |dialog: &mut PreferencesDialog, events| {
            let mut action = Action::None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| action = show(ui.ctx(), dialog, true),
            );
            output.textures_delta.clear();
            (action, output)
        };
        let _ = frame(&mut dialog, vec![]);
        let (action, output) = frame(&mut dialog, vec![]);
        assert!(matches!(action, Action::None));
        let shown = texts(&output);
        assert!(
            shown.iter().any(|t| t.contains("registry.unemu.dev")),
            "{shown:?}"
        );
        assert!(shown.iter().any(|t| t.contains("this session only")));

        dialog.registries.select(1);
        dialog.registries.open_login();
        dialog.registries.login.as_mut().unwrap().password = "synthetic-secret".into();
        let _ = frame(&mut dialog, vec![]);
        let (_, output) = frame(&mut dialog, vec![]);
        let shown = texts(&output);
        assert!(shown
            .iter()
            .any(|t| t.contains("Docker credential helper (later)")));
        assert!(
            !shown.iter().any(|t| t.contains("synthetic-secret")),
            "{shown:?}"
        );
        let escape = || {
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]
        };
        let (action, _) = frame(&mut dialog, escape());
        assert!(matches!(action, Action::None));
        assert!(
            dialog.registries.login.is_none(),
            "Escape closes the top dialog only"
        );

        dialog.registries.open_edit();
        dialog.registries.confirm = Some(ConfirmPublic {
            host: "nas.example".into(),
            exposed: "nas.example".into(),
            settings: Default::default(),
        });
        let _ = frame(&mut dialog, vec![]);
        let (_, output) = frame(&mut dialog, vec![]);
        assert!(texts(&output)
            .iter()
            .any(|t| t.contains("Make nas.example public?")));
        let _ = frame(&mut dialog, escape());
        assert!(dialog.registries.confirm.is_none() && dialog.registries.editor.is_some());
        let _ = frame(&mut dialog, escape());
        assert!(dialog.registries.editor.is_none());

        dialog.tab = Tab::General;
        let _ = frame(&mut dialog, vec![]);
        let (action, _) = frame(&mut dialog, escape());
        assert!(matches!(action, Action::Close));
    }

    #[test]
    fn the_system_shortcut_option_explains_itself_and_toggles_at_once() {
        const LABEL: &str = "Send macOS system shortcuts to the guest while it has the keyboard";
        fn find(shape: &egui::Shape) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == LABEL => {
                    Some(text.visual_bounding_rect().center())
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(find),
                _ => None,
            }
        }
        let temp = assembly::test_support::TempDir::new("preferences-system-shortcuts");
        let mut dialog = PreferencesDialog {
            tab: Tab::General,
            registries: RegistriesPage::new(
                Some(temp.path().join("preferences.toml")),
                Arc::new(SessionStore::default()),
                EnvCredentials::default(),
            ),
            system_shortcuts: None,
        };
        let ctx = egui::Context::default();
        let frame = |dialog: &mut PreferencesDialog, events| {
            let mut action = Action::None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| action = show(ui.ctx(), dialog, true),
            );
            output.textures_delta.clear();
            (action, output)
        };
        let _ = frame(&mut dialog, vec![]);
        let (_, output) = frame(&mut dialog, vec![]);
        assert!(!texts(&output).iter().any(|t| t == LABEL), "macOS only");

        dialog.system_shortcuts = Some(SystemShortcuts {
            on: false,
            note: Some("synthetic note".into()),
        });
        let _ = frame(&mut dialog, vec![]);
        let (_, output) = frame(&mut dialog, vec![]);
        let shown = texts(&output);
        assert!(shown.iter().any(|t| t == SYSTEM_SHORTCUTS_WHY), "{shown:?}");
        assert!(shown.iter().any(|t| t == "synthetic note"));
        let at = output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape))
            .expect("the check box is shown");
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let _ = frame(
            &mut dialog,
            vec![egui::Event::PointerMoved(at), button(true)],
        );
        let (action, _) = frame(&mut dialog, vec![button(false)]);
        assert!(matches!(action, Action::SystemShortcuts(true)));
    }

    #[test]
    fn namespaces_render_as_child_rows_and_the_add_dialog_has_their_field() {
        let temp = assembly::test_support::TempDir::new("preferences-namespaces");
        let path = temp.path().join("preferences.toml");
        std::fs::write(
            &path,
            "[[registries]]\nhost = \"registry.unemu.dev\"\nnamespaces = [{ name = \"unemu\", private = false }, { name = \"private\", private = true }]\n",
        )
        .unwrap();
        let mut dialog = PreferencesDialog {
            tab: Tab::Registries,
            registries: RegistriesPage::new(
                Some(path),
                Arc::new(SessionStore::default()),
                EnvCredentials::default(),
            ),
            system_shortcuts: None,
        };
        let ctx = egui::Context::default();
        let frame = |dialog: &mut PreferencesDialog| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = show(ui.ctx(), dialog, true);
            });
            output.textures_delta.clear();
            texts(&output)
        };
        let _ = frame(&mut dialog);
        let shown = frame(&mut dialog);
        for wanted in [
            "Per namespace",
            "    unemu/ — official namespace — open images, public pulls",
            "No — open only",
            "    private/ — proprietary images — login always required",
            "unemu/ — public, open content only; private/ — private",
        ] {
            assert!(shown.iter().any(|t| t == wanted), "{wanted}: {shown:?}");
        }
        dialog.registries.open_add();
        let _ = frame(&mut dialog);
        let shown = frame(&mut dialog);
        assert!(shown.iter().any(|t| t == "Namespaces"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "empty = the whole registry"));
        assert!(shown
            .iter()
            .any(|t| t.contains("unemu/ is always public, open content only")));
    }

    #[test]
    fn the_password_field_keeps_no_undo_history_after_submit_or_cancel() {
        use egui::widgets::text_edit::TextEditState;
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let unreachable = closed.local_addr().unwrap().to_string();
        drop(closed);
        let temp = assembly::test_support::TempDir::new("preferences-password");
        let path = temp.path().join("preferences.toml");
        std::fs::write(
            &path,
            format!(
                "[[registries]]\nhost = \"{unreachable}\"\nplain_http = true\n[[registries]]\nhost = \"lab.example\"\n"
            ),
        )
        .unwrap();
        let mut dialog = PreferencesDialog {
            tab: Tab::Registries,
            registries: RegistriesPage::new(
                Some(path),
                Arc::new(SessionStore::default()),
                EnvCredentials::default(),
            ),
            system_shortcuts: None,
        };
        let ctx = egui::Context::default();
        let frame = |dialog: &mut PreferencesDialog, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    let _ = show(ui.ctx(), dialog, true);
                },
            );
            output.textures_delta.clear();
        };
        let id = password_field_id();
        let no_history = |ctx: &egui::Context| {
            TextEditState::load(ctx, id)
                .is_none_or(|state| format!("{:?}", state.undoer()).contains("undo count: 0"))
        };
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        dialog.registries.select(1);
        dialog.registries.open_login();
        dialog.registries.login.as_mut().unwrap().user = "ci".into();
        frame(&mut dialog, vec![]);
        ctx.memory_mut(|memory| memory.request_focus(id));
        frame(&mut dialog, vec![]);
        frame(&mut dialog, vec![egui::Event::Text("typed-secret".into())]);
        frame(&mut dialog, vec![]);
        assert_eq!(
            dialog.registries.login.as_ref().unwrap().password,
            "typed-secret",
            "the real text field received the typing"
        );
        assert!(!no_history(&ctx), "typing left undo history");
        frame(&mut dialog, vec![key(egui::Key::Enter)]);
        let login = dialog.registries.login.as_ref().unwrap();
        assert!(login.busy && login.password.is_empty());
        assert!(TextEditState::load(&ctx, id).is_none());
        dialog.registries.wait();
        frame(&mut dialog, vec![]);
        assert!(dialog.registries.login.as_ref().unwrap().error.is_some());
        assert!(no_history(&ctx));

        // Typing again, then Escape: the next dialog, for another registry,
        // starts without the old history.
        ctx.memory_mut(|memory| memory.request_focus(id));
        frame(&mut dialog, vec![]);
        frame(&mut dialog, vec![egui::Event::Text("second".into())]);
        frame(&mut dialog, vec![]);
        assert!(!no_history(&ctx));
        frame(&mut dialog, vec![key(egui::Key::Escape)]);
        assert!(dialog.registries.login.is_none());
        assert!(TextEditState::load(&ctx, id).is_none());
        dialog.registries.select(2);
        dialog.registries.open_login();
        frame(&mut dialog, vec![]);
        assert_eq!(
            dialog.registries.login.as_ref().unwrap().host,
            "lab.example"
        );
        assert!(no_history(&ctx));
        frame(&mut dialog, vec![key(egui::Key::Escape)]);
        assert!(TextEditState::load(&ctx, id).is_none());
    }
}
