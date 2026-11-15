// SPDX-License-Identifier: AGPL-3.0-or-later
//! Whole-machine disk-layer dialog model; machine-images §§4 and 7.
//! Publication and discard are requested only after an explicit review step.

use crate::{library_view::human_size, library_worker::WorkspaceKind};
use pc_image::inspection::ImageDetails;

pub(super) enum Action {
    None,
    Close,
    Import,
    Publish(String, String, bool),
    Discard(String),
}
#[derive(PartialEq)]
enum Review {
    None,
    Save,
    New,
    Discard,
}
pub(super) struct LayersDialog {
    workspace: Option<String>,
    kind: WorkspaceKind,
    title: String,
    details: Option<ImageDetails>,
    review: Review,
    tag: String,
    flatten: bool,
    pub error: Option<String>,
}
impl LayersDialog {
    pub fn new(workspace: Option<String>) -> Self {
        Self {
            workspace,
            kind: WorkspaceKind::Regular,
            title: "Disk layers — Machine".into(),
            details: None,
            review: Review::None,
            tag: String::new(),
            flatten: false,
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
    pub fn begin_keep(&mut self) {
        self.review = Review::Save;
    }
    pub fn begin_discard(&mut self) {
        self.review = Review::Discard;
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
        self.review = Review::None;
        self.error = None;
    }
    fn publication(&self) -> Option<Action> {
        if self.details.is_none()
            || !matches!(self.review, Review::Save | Review::New)
            || self.tag.trim().is_empty()
        {
            return None;
        }
        self.workspace.as_ref().map(|name| {
            Action::Publish(
                name.clone(),
                self.tag.trim().into(),
                self.review == Review::New && self.flatten,
            )
        })
    }
    fn discard(&self) -> Option<Action> {
        (self.review == Review::Discard && (self.details.is_some() || self.error.is_some()))
            .then(|| {
                self.workspace
                    .as_ref()
                    .map(|name| Action::Discard(name.clone()))
            })
            .flatten()
    }
    pub fn show(&mut self, ctx: &egui::Context, busy: bool, classic: bool) -> Action {
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
                ui.separator();
                if self.review == Review::Discard {
                    ui.strong("Discard changes to the affected media?");
                    if self.details.is_none(){ui.label("Media inspection failed. This discards every writable medium and CMOS in this workspace.");}
                    for medium in self.affected() { ui.label(format!("{} · {} · {}", medium.drive, medium.id, medium.name)); }
                    ui.label("The workspace's CMOS, settings and all media changes are removed. Immutable bases remain.");
                    if ui.add_enabled(!busy, egui::Button::new("Confirm discard")).clicked() { if let Some(a) = self.discard() { action = a; } }
                } else if self.review != Review::None {
                    ui.label("New repository:tag (must not already exist)"); ui.text_edit_singleline(&mut self.tag);
                    if self.review == Review::New { ui.checkbox(&mut self.flatten, "Flatten disk chains into new bases"); }
                    if ui.add_enabled(!busy && self.publication().is_some(), egui::Button::new("Publish")).clicked() { if let Some(a) = self.publication() { action = a; } }
                }
                if self.review != Review::None && ui.button("Cancel action").clicked() { self.review = Review::None; }
                ui.add_enabled_ui(!busy && self.details.is_some(), |ui| {
                    ui.horizontal(|ui| {
                        if ui.button(if self.kind.disposable(){"Keep as new image…"}else{"Save as layer…"}).clicked() { self.review = Review::Save; }
                        if ui.button("Discard changes").clicked() { self.review = Review::Discard; }
                        if !self.kind.disposable() && ui.button("New image from here…").clicked() { self.review = Review::New; }
                    });
                });
            }
            if let Some(e) = &self.error { ui.add(egui::Label::new(e).wrap()); }
            if ui.button("Close").clicked() { action = Action::Close; }
        });
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
    #[test]
    fn publication_requires_tag_and_review_and_new_image_can_flatten() {
        let mut dialog = LayersDialog::new(Some("gaming-synthetic".into()));
        assert!(dialog.publication().is_none());
        dialog.review = Review::Save;
        assert!(dialog.publication().is_none());
        dialog.tag = "example:saved".into();
        assert!(dialog.publication().is_none());
        dialog.set_details(Ok(details()));
        dialog.flatten = true;
        assert!(
            matches!(dialog.publication(), Some(Action::Publish(name,tag,false)) if name == "gaming-synthetic" && tag == "example:saved")
        );
        dialog.review = Review::New;
        assert!(matches!(
            dialog.publication(),
            Some(Action::Publish(_, _, true))
        ));
        dialog.completed();
        assert!(dialog.publication().is_none());
    }
    #[test]
    fn discard_requires_details_and_explicit_affected_media_confirmation() {
        let mut dialog = LayersDialog::new(Some("gaming-synthetic".into()));
        assert!(dialog.discard().is_none());
        dialog.set_details(Ok(details()));
        assert!(dialog.discard().is_none());
        dialog.review = Review::Discard;
        assert!(
            matches!(dialog.discard(), Some(Action::Discard(name)) if name == "gaming-synthetic")
        );
        dialog.review = Review::None;
        assert!(dialog.discard().is_none());
        let file_dialog = LayersDialog::new(None);
        assert!(file_dialog.publication().is_none());
        assert!(file_dialog.discard().is_none());
    }
    pub(crate) fn reviewed_publication(
        name: &str,
        kind: WorkspaceKind,
        details: ImageDetails,
        tag: &str,
        flatten: bool,
    ) -> Action {
        let mut dialog = LayersDialog::for_workspace(Some(name.into()), kind, "Synthetic machine");
        dialog.set_details(Ok(details));
        dialog.tag = tag.into();
        dialog.review = if flatten { Review::New } else { Review::Save };
        dialog.flatten = flatten;
        dialog.publication().unwrap()
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
