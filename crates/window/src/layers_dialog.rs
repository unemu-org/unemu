// SPDX-License-Identifier: AGPL-3.0-or-later
//! Whole-machine disk-layer dialog model; machine-images §§4 and 7.
//! Publication and discard are requested only after an explicit review step.
//! Save as layer…, New image from here… and Keep as new image… all open the
//! one Save as new image… form (`backlog/docs/design/mockups/save-as-image.html`):
//! a saved layer always becomes a new image with a name and a tag.

use crate::{
    library_view::human_size,
    library_worker::WorkspaceKind,
    save_image::{self, SaveAsImage, TagLevel, Target},
};
use image_store::Digest;
use pc_image::inspection::ImageDetails;

pub(super) enum Action {
    None,
    Close,
    Import,
    Publish(String, Target),
    Discard(String),
}
/// The image the workspace's session pins: its reference, digest and title,
/// read by the library worker.
#[derive(Clone, Debug, Default)]
pub(super) struct SourceImage {
    pub reference: String,
    pub digest: Option<Digest>,
    pub title: Option<String>,
    /// Every local tag, for the tag rules.
    pub tags: Vec<(String, Digest)>,
    /// An unfinished save of this workspace: its tag and expected digest.
    pub pending: Option<(String, Option<Digest>)>,
}
pub(super) struct LayersDialog {
    workspace: Option<String>,
    kind: WorkspaceKind,
    title: String,
    details: Option<ImageDetails>,
    discard: bool,
    source: SourceImage,
    /// The Save as new image… form while it is open.
    pub save: Option<SaveAsImage>,
    /// Open the form once the worker has read the source image.
    save_wanted: Option<bool>,
    pub error: Option<String>,
}
impl LayersDialog {
    pub fn new(workspace: Option<String>) -> Self {
        Self {
            workspace,
            kind: WorkspaceKind::Regular,
            title: "Disk layers — Machine".into(),
            details: None,
            discard: false,
            source: SourceImage::default(),
            save: None,
            save_wanted: None,
            error: None,
        }
    }
    pub fn for_workspace(workspace: Option<String>, kind: WorkspaceKind, name: &str) -> Self {
        let mut dialog = Self::new(workspace);
        dialog.kind = kind;
        dialog.title = format!("Disk layers — {name}");
        dialog
    }
    pub fn set_machine_name(&mut self, name: &str) {
        self.title = format!("Disk layers — {name}");
    }
    pub fn set_source(&mut self, source: SourceImage) {
        self.source = source;
        if let Some(from_here) = self.save_wanted.take() {
            self.begin_save(from_here);
        }
    }
    /// Refresh the form's tags after the store refused a save and ask again.
    pub fn refused(&mut self, error: String, tags: Vec<(String, Digest)>) {
        self.error = Some(error);
        self.source.tags = tags.clone();
        if let Some(form) = &mut self.save {
            form.refresh(tags);
        }
    }
    /// Open the Save as new image… form; `from_here` offers flattening. Before
    /// the source image is known, the form opens when it arrives.
    pub fn begin_save(&mut self, from_here: bool) {
        let Some(digest) = self.source.digest.clone() else {
            self.save_wanted = Some(from_here);
            return;
        };
        let form = SaveAsImage::new(&self.source.reference, digest, self.source.tags.clone())
            .with_title(self.source.title.as_deref());
        self.save = Some(if from_here { form.with_flatten() } else { form });
        self.discard = false;
    }
    pub fn begin_keep(&mut self) {
        self.begin_save(false);
    }
    pub fn begin_discard(&mut self) {
        self.discard = true;
        self.save = None;
    }
    fn affected(&self) -> Vec<&pc_image::inspection::MediumDetails> {
        self.details
            .as_ref()
            .into_iter()
            .flat_map(|d| &d.media)
            .filter(|m| m.policy != pc_image::Policy::Readonly && m.working_sectors != Some(0))
            .collect()
    }
    pub fn set_workspace(&mut self, name: String) {
        self.workspace = Some(name);
    }
    pub fn set_details(&mut self, details: Result<ImageDetails, String>) {
        match details {
            Ok(details) => self.details = Some(details),
            Err(e) => self.error = Some(e),
        }
    }
    pub fn completed(&mut self) {
        self.discard = false;
        self.save = None;
        self.save_wanted = None;
        self.error = None;
    }
    /// Finish an interrupted save with its own tag and expected digest, as a
    /// repeated publication does; nothing else can run until it finishes.
    fn finish_pending(&self) -> Option<Action> {
        let (reference, expected) = self.source.pending.clone()?;
        Some(Action::Publish(
            self.workspace.clone()?,
            Target {
                reference,
                expected,
                title: None,
                flatten: false,
            },
        ))
    }
    fn pending_notice(&self, ui: &mut egui::Ui, busy: bool) -> Option<Action> {
        let (tag, _) = self.source.pending.as_ref()?;
        ui.add(egui::Label::new(format!("An earlier save to {tag} did not finish. The machine cannot start until it does; finishing keeps every change it was saving.")).wrap());
        ui.add_enabled(!busy, egui::Button::new(format!("Finish saving {tag}")))
            .clicked()
            .then(|| self.finish_pending())
            .flatten()
    }
    /// Save: the form's tag rules decide; the media must be inspected first.
    fn publication(&mut self) -> Option<Action> {
        self.details.as_ref()?;
        let name = self.workspace.clone()?;
        let target = self.save.as_mut()?.submit()?;
        Some(Action::Publish(name, target))
    }
    fn discard(&self) -> Option<Action> {
        (self.discard && (self.details.is_some() || self.error.is_some()))
            .then(|| {
                self.workspace
                    .as_ref()
                    .map(|name| Action::Discard(name.clone()))
            })
            .flatten()
    }
    pub fn show(&mut self, ctx: &egui::Context, busy: bool, classic: bool) -> Action {
        if self.save.is_some() {
            return self.show_save(ctx, busy, classic);
        }
        let mut action = Action::None;
        egui::Modal::new(egui::Id::unique("disk-layers")).show(ctx, |ui| {
            ui.set_width(700.0); if classic { crate::shell::gradient_title(ui, &self.title); } else { ui.heading(&self.title); } ui.separator();
            if self.workspace.is_none() {
                ui.label("Disk layers need a machine image. Import a machine file to keep settings, firmware and every drive together.");
                if ui.button("File > Import image…").clicked() { action = Action::Import; }
            } else {
                egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                    if let Some(details) = &self.details {
                        for medium in &details.media {
                            ui.strong(format!("{} · {} · {}", medium.drive, medium.name, medium.policy.as_str()));
                            for (i, member) in medium.chain.iter().enumerate() { ui.monospace(format!("{}{}  {}  {}{}", "  ".repeat(i), if member.blank_bytes.is_some() { "└─ zero base" } else if i == 0 { "└─ base" } else { "└─ layer" }, &member.digest.hex()[..12], member.blank_bytes.map_or_else(|| human_size(member.size), |bytes| format!("{} blank · {} stored", human_size(bytes), human_size(member.size))), if member.shared_by>1{format!("  shared by {} images",member.shared_by)}else{String::new()})); }
                            ui.monospace(format!("    └─ working changes: {} sectors · not saved", medium.working_sectors.map_or_else(|| "unknown".into(), |n| n.to_string())));
                            ui.add_space(6.0);
                        }
                        for slot in &details.slots { ui.label(slot); }
                        ui.strong("Firmware"); for firmware in &details.firmware { ui.label(firmware); }
                        ui.label(format!("Settings: pinned image configuration · stored {} · exclusive {} · dedup savings {}", human_size(details.stored_bytes), human_size(details.exclusive_bytes), human_size(details.dedup_bytes)));
                    } else { ui.label("Loading chains…"); }
                });
                ui.label("Saving stops the machine first. Settings, firmware, catalogues and all drive heads are published together.");
                if let Some(a) = self.pending_notice(ui, busy) { action = a; }
                ui.separator();
                if self.discard {
                    ui.strong("Discard changes to the affected media?");
                    if self.details.is_none(){ui.label("Media inspection failed. This discards every writable medium and CMOS in this workspace.");}
                    for medium in self.affected() { ui.label(format!("{} · {} · {}", medium.drive, medium.id, medium.name)); }
                    ui.label("The workspace's CMOS, settings and all media changes are removed. Immutable bases remain.");
                    if ui.add_enabled(!busy, egui::Button::new("Confirm discard")).clicked() { if let Some(a) = self.discard() { action = a; } }
                    if ui.button("Cancel action").clicked() { self.discard = false; }
                }
                let can_save = self.source.digest.is_some();
                ui.add_enabled_ui(!busy && self.details.is_some(), |ui| {
                    ui.horizontal(|ui| {
                        if ui.add_enabled(can_save, egui::Button::new(if self.kind.disposable(){"Keep as new image…"}else{"Save as layer…"})).clicked() { self.begin_save(false); }
                        if ui.button("Discard changes").clicked() { self.begin_discard(); }
                        if !self.kind.disposable() && ui.add_enabled(can_save, egui::Button::new("New image from here…")).clicked() { self.begin_save(true); }
                    });
                });
            }
            if let Some(e) = &self.error { ui.add(egui::Label::new(e).wrap()); }
            if ui.button("Close").clicked() { action = Action::Close; }
        });
        action
    }

    /// The Save as new image… dialog: name and tag, what is saved, the base
    /// image, and the redistribution note for new disk layers.
    fn show_save(&mut self, ctx: &egui::Context, busy: bool, classic: bool) -> Action {
        let mut action = Action::None;
        let mut cancel = false;
        let mut save = false;
        let workspace = self.workspace.clone().unwrap_or_default();
        let changed: Vec<_> = self.affected().into_iter().cloned().collect();
        let loaded = self.details.is_some();
        let pending = self.source.pending.as_ref().map(|(tag, _)| tag.clone());
        let mut finish = false;
        let Some(form) = &mut self.save else {
            return action;
        };
        egui::Modal::new(egui::Id::unique("save-as-image")).show(ctx, |ui| {
            ui.set_width(640.0);
            if classic {
                crate::shell::gradient_title(ui, "Save as new image");
            } else {
                ui.heading("Save as new image");
            }
            ui.label(format!(
                "From {} {} · workspace {workspace}",
                form.source,
                save_image::short(&form.source_digest)
            ));
            ui.label("Saving stops the machine first: every drive finishes its writes, then the changes are frozen into the new image.");
            if let Some(tag) = &pending {
                ui.add(egui::Label::new(format!("An earlier save to {tag} did not finish. The machine cannot start until it does; finishing keeps every change it was saving.")).wrap());
                finish = ui
                    .add_enabled(!busy, egui::Button::new(format!("Finish saving {tag}")))
                    .clicked();
            }
            ui.separator();
            ui.strong("Name");
            save_image::form(ui, form);
            ui.separator();
            ui.strong("What is saved");
            if !loaded {
                ui.label("Loading chains…");
            }
            for medium in &changed {
                let layered = medium.chain.iter().any(|m| m.blank_bytes.is_some()) || medium.chain.len() > 1;
                ui.label(format!(
                    "{} · {} — {} — {}",
                    medium.drive,
                    medium.name,
                    medium.working_sectors.map_or_else(
                        || "changes of unknown size".into(),
                        |n| format!("{n} sectors written")
                    ),
                    if layered { "a new layer on its chain" } else { "frozen into a new blob" }
                ));
            }
            if loaded && changed.is_empty() {
                ui.label("No drive has unsaved changes: every disk and medium is shared.");
            }
            ui.label("Hardware settings unchanged; firmware and unchanged media are shared with the source image.");
            ui.separator();
            ui.strong("Base image (FROM chain)");
            ui.monospace(format!(
                "image  {}  {}\n        └─ {}  this save",
                form.source,
                save_image::short(&form.source_digest),
                form.reference()
            ));
            ui.add(egui::Label::new(egui::RichText::new(format!("The new image records {} as its base for provenance. Running it never needs the base image or its tag.", form.source)).small()).wrap());
            if !changed.is_empty() {
                ui.add(egui::Label::new("New disk layers count as user-supplied until you mark them open, so this image can be pushed only to a private registry. Its base stays as it is.").wrap());
            }
            ui.add(egui::Label::new(egui::RichText::new("After saving, this workspace runs the new image with fresh, empty journals.").small()).wrap());
            if let Some(e) = &self.error {
                ui.add(egui::Label::new(e).wrap());
            }
            ui.separator();
            ui.horizontal(|ui| {
                let check = form.check();
                let ready = !busy && loaded && (check.level != TagLevel::Bad || check.taken.is_some());
                if ui.add_enabled(ready, egui::Button::new("Save")).clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if let Some(target) = save_image::prompts(ctx, form, classic) {
            if let Some(name) = self.workspace.clone() {
                action = Action::Publish(name, target);
            }
        } else if form.prompt.is_none() && ctx.input_mut(crate::shell::escape_pressed) {
            cancel = true;
        }
        if save {
            if let Some(a) = self.publication() {
                action = a;
            }
        }
        if finish {
            if let Some(a) = self.finish_pending() {
                action = a;
            }
        }
        if cancel {
            self.save = None;
        }
        action
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn details() -> ImageDetails {
        ImageDetails {
            media: vec![],
            slots: vec![],
            firmware: vec![],
            hardware: vec![],
            stored_bytes: 0,
            exclusive_bytes: 0,
            dedup_bytes: 0,
        }
    }
    pub(crate) fn source(reference: &str, digest: &Digest) -> SourceImage {
        SourceImage {
            reference: reference.into(),
            digest: Some(digest.clone()),
            title: Some("Example machine".into()),
            tags: vec![(reference.into(), digest.clone())],
            pending: None,
        }
    }
    #[test]
    fn save_as_layer_and_new_image_from_here_open_the_same_form() {
        let mut dialog = LayersDialog::new(Some("gaming-synthetic".into()));
        dialog.set_source(source("example:1.0.0", &Digest::of(b"example")));
        dialog.begin_save(false);
        assert!(
            dialog.publication().is_none(),
            "media are not inspected yet"
        );
        dialog.set_details(Ok(details()));
        let form = dialog.save.as_mut().unwrap();
        assert_eq!(form.reference(), "example:1.1.0");
        assert_eq!(form.title.as_deref(), Some("Example machine"));
        assert_eq!(form.flatten, None);
        form.set_tag("saved".into());
        assert!(matches!(
            dialog.publication(),
            Some(Action::Publish(name, Target { reference, expected: None, title: None, flatten: false }))
                if name == "gaming-synthetic" && reference == "example:saved"
        ));
        dialog.begin_save(true);
        let form = dialog.save.as_mut().unwrap();
        form.flatten = Some(true);
        form.title = Some("Renamed".into());
        assert!(matches!(
            dialog.publication(),
            Some(Action::Publish(_, Target { flatten: true, title: Some(title), .. })) if title == "Renamed"
        ));
        // A taken version tag is refused, never published.
        dialog.save.as_mut().unwrap().set_tag("1.0.0".into());
        assert!(dialog.publication().is_none());
        assert!(matches!(
            dialog.save.as_ref().unwrap().prompt,
            Some(save_image::Prompt::Refuse(_))
        ));
        dialog.completed();
        assert!(dialog.save.is_none());
        assert!(dialog.publication().is_none());
    }
    #[test]
    fn the_form_waits_for_its_source_and_offers_to_finish_a_pending_save() {
        let mut dialog = LayersDialog::new(Some("regular-1".into()));
        dialog.begin_keep();
        assert!(dialog.save.is_none(), "the source is not read yet");
        let mut source = source("example:1.0.0", &Digest::of(b"example"));
        source.pending = Some(("example:saved".into(), Some(Digest::of(b"old"))));
        dialog.set_source(source);
        assert!(
            dialog.save.is_some(),
            "the form opens once the source arrives"
        );
        assert!(matches!(
            dialog.finish_pending(),
            Some(Action::Publish(name, Target { reference, expected: Some(d), title: None, flatten: false }))
                if name == "regular-1" && reference == "example:saved" && d == Digest::of(b"old")
        ));
    }
    #[test]
    fn discard_requires_details_and_explicit_affected_media_confirmation() {
        let mut dialog = LayersDialog::new(Some("gaming-synthetic".into()));
        assert!(dialog.discard().is_none());
        dialog.set_details(Ok(details()));
        assert!(dialog.discard().is_none());
        dialog.begin_discard();
        assert!(
            matches!(dialog.discard(), Some(Action::Discard(name)) if name == "gaming-synthetic")
        );
        dialog.discard = false;
        assert!(dialog.discard().is_none());
        let mut file_dialog = LayersDialog::new(None);
        assert!(file_dialog.publication().is_none());
        assert!(file_dialog.discard().is_none());
    }
    /// Finish saving <tag>, as its button asks.
    pub(crate) fn finish(dialog: &LayersDialog) -> Option<Action> {
        dialog.finish_pending()
    }
    /// The publication a user requests through the Save as new image… form.
    pub(crate) fn reviewed_publication(
        name: &str,
        kind: WorkspaceKind,
        details: ImageDetails,
        source: SourceImage,
        tag: &str,
        flatten: bool,
        title: Option<&str>,
    ) -> Action {
        let mut dialog = LayersDialog::for_workspace(Some(name.into()), kind, "Synthetic machine");
        dialog.set_source(source);
        dialog.set_details(Ok(details));
        dialog.begin_save(flatten);
        let form = dialog.save.as_mut().unwrap();
        let (repo, tag) = tag.split_once(':').unwrap();
        form.set_repo(repo.into());
        form.set_tag(tag.into());
        if let Some(title) = title {
            form.title = Some(title.into());
        }
        if let Some(choice) = &mut form.flatten {
            *choice = flatten;
        }
        dialog.publication().unwrap()
    }
    #[test]
    fn the_save_form_renders_without_a_window() {
        let mut dialog = LayersDialog::for_workspace(
            Some("regular-1".into()),
            WorkspaceKind::Regular,
            "Synthetic machine",
        );
        dialog.set_source(source("example:1.0.0", &Digest::of(b"example")));
        dialog.set_details(Ok(details()));
        dialog.begin_save(true);
        let ctx = egui::Context::default();
        let mut frame = || {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                assert!(matches!(dialog.show(ui.ctx(), false, true), Action::None));
            });
            output.textures_delta.clear();
            output
        };
        let _ = frame();
        let output = frame();
        for text in [
            "Save as new image",
            "Base image (FROM chain)",
            "Flatten disk chains into new bases",
        ] {
            assert!(
                output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(t) if t.galley.text().contains(text))),
                "{text}"
            );
        }
    }
    #[test]
    fn affected_media_excludes_readonly_and_unchanged_writable_media() {
        let mut details = details();
        for (id, policy, sectors) in [
            ("readonly", pc_image::Policy::Readonly, Some(3)),
            ("unchanged", pc_image::Policy::Journal, Some(0)),
            ("changed", pc_image::Policy::Journal, Some(1)),
            ("volatile", pc_image::Policy::Volatile, None),
        ] {
            details.media.push(pc_image::inspection::MediumDetails {
                id: id.into(),
                drive: id.into(),
                name: id.into(),
                policy,
                chain: vec![],
                working_sectors: sectors,
            });
        }
        let mut dialog = LayersDialog::for_workspace(
            Some("disposable".into()),
            WorkspaceKind::Disposable,
            "Synthetic machine",
        );
        dialog.set_details(Ok(details));
        assert_eq!(dialog.title, "Disk layers — Synthetic machine");
        assert_eq!(
            dialog
                .affected()
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["changed", "volatile"]
        );
    }
}
