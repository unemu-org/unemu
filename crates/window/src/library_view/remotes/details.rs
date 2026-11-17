// SPDX-License-Identifier: AGPL-3.0-or-later
//! The details pane of the Remotes selection and its actions, as the
//! approved remotes mock-up, revision 2
//! (`backlog/docs/design/mockups/remotes.html`), draws them; machine-images
//! §3 (local tags), §5.1 (registries) and §5.6 (the release an image needs).
//!
//! For one tag the pane shows what the registry's manifest says (its
//! annotations, digest, type and licence, and for a media set its disks), the
//! edition and version its metadata record adds once it is selected, the
//! local tag a pull writes and where it comes from, how the tag stands
//! against the library, what a pull would download with the blobs the store
//! holds left out, and the repository's tags. Its buttons: Show in library
//! for a tag in the library, Pull…, Pull and create machine… for a machine
//! image, Pull and insert… for an installation media set (offered while a
//! stopped machine has a drive that takes the set), and Contents…, which
//! lists every descriptor of the manifest; none of them is fetched. A tag
//! this release cannot run, or one of a registry that no longer answers,
//! cannot be pulled, and the pane says why under its buttons.
//!
//! For several tags the pane lists them with their local states and offers
//! Pull… for all of them, which leaves out, with the reason, the ones it
//! cannot pull.

use super::{type_label, Group, LibraryView};
use crate::{
    library_view::{human_size, machines::MachineRow, pane, Filter, ImageRow, Request},
    registry_transfer::kind_label,
    remote_browse::{LocalState, TagRow},
    remote_pull::{drives_for, refusal, Drive, Mode, Pick, RemotePull},
};
use egui::RichText;
use image_registry::DEFAULT_REGISTRY;
use image_store::{license, media_type as mt, Descriptor, Digest};
use ui_kit::shell_icons::Icon;

const DESCRIPTION: &str = "org.opencontainers.image.description";
const VENDOR: &str = "org.opencontainers.image.vendor";
const LICENSES: &str = "org.opencontainers.image.licenses";
const SOURCE: &str = "org.opencontainers.image.source";
const TITLE: &str = "org.opencontainers.image.title";

/// What a click in the pane asked for.
enum PaneAction {
    Pull(Mode),
    ShowInLibrary,
    NewMachine,
    Insert,
    Contents,
    /// A tag of the repository's table.
    Pick(String),
    ClearSelection,
}

/// One selected tag and what the pane needs of it, copied from the model so
/// the pane can draw while it changes.
struct Selected {
    host: String,
    repository: String,
    tag: String,
    /// `None` while its record is read.
    row: Option<TagRow>,
    /// The repository's tags, newest first, with the rows read so far.
    tags: Vec<(String, Option<TagRow>)>,
    blocked: Option<String>,
}

impl Selected {
    fn pick(&self) -> Pick<'_> {
        Pick {
            host: &self.host,
            repository: &self.repository,
            tag: &self.tag,
            row: self.row.as_ref(),
            blocked: self.blocked.clone(),
        }
    }
}

/// A pick Pull… of several takes: one it can pull, and not one whose local
/// name holds another image, which goes alone.
fn takes(pick: &Pick<'_>) -> bool {
    refusal(pick).is_none()
        && !matches!(
            pick.row.and_then(|row| row.local.as_ref()),
            Some(LocalState::OtherImage { .. })
        )
}

/// The short digest the panes show.
fn short(digest: &Digest) -> String {
    format!("sha256:{}", pane::short_digest(digest.hex()))
}

/// A descriptor's redistribution mark is open unless it says user-supplied.
fn open(descriptor: &Descriptor) -> bool {
    descriptor
        .annotations
        .get(license::REDISTRIBUTION)
        .map(String::as_str)
        != Some(license::USER_SUPPLIED)
}

/// "7 × 1.44M diskettes", or each format's count for a mixed set.
fn media_line(media: &[(String, String)]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for (_, format) in media {
        match counts.iter_mut().find(|(f, _)| f == format) {
            Some((_, count)) => *count += 1,
            None => counts.push((format, 1)),
        }
    }
    let disks = if media.len() == 1 {
        "diskette"
    } else {
        "diskettes"
    };
    let parts: Vec<String> = counts
        .iter()
        .map(|(format, count)| format!("{count} × {format}"))
        .collect();
    format!("{} {disks}", parts.join(", "))
}

/// The Local state as the list says it, in words for the table.
fn state_text(row: Option<&TagRow>) -> String {
    let Some(row) = row else {
        return "Not read yet".into();
    };
    let Ok(record) = &row.record else {
        return "Could not be read".into();
    };
    if let Some(reason) = record.not_runnable() {
        return reason;
    }
    match &row.local {
        Some(LocalState::InLibrary) => "In library".into(),
        Some(LocalState::NewerAvailable { have }) => format!("Newer available · you have {have}"),
        Some(LocalState::LocalNewer { have }) => format!("Local is newer: {have}"),
        Some(LocalState::OtherImage { .. }) => "⚠ Local tag names another image".into(),
        Some(LocalState::NotPulled { same_as: Some(tag) }) => {
            format!("Not pulled · same image as {tag}")
        }
        Some(LocalState::NotPulled { same_as: None }) | None => "Not pulled".into(),
    }
}

/// What the pane says of the tag against the library, in a sentence.
fn local_sentence(row: &TagRow) -> String {
    match &row.local {
        Some(LocalState::InLibrary) => "In the library under this name.".into(),
        Some(LocalState::NewerAvailable { have }) if release_alias(row) => format!(
            "Newer than your copy. The library has {have}, an older revision of this release. Pull writes the registry's newest revision and moves your local alias {} forward to it; {have} and the machines made from it stay as they are.",
            row.local_tag
        ),
        Some(LocalState::NewerAvailable { have }) => format!(
            "Newer than your copy. The library has {have} from this registry. Pull adds this tag beside it; your copy and the machines made from it stay as they are."
        ),
        Some(LocalState::LocalNewer { have }) => format!(
            "Your copy is newer: {have} is a higher revision of this release than the registry lists. Pull writes the registry's revision beside it and leaves your alias {} where it is.",
            row.local_tag
        ),
        Some(LocalState::OtherImage { current }) if release_alias(row) => format!(
            "⚠ Your alias under this name names another image ({}), which no newer or older revision of this release on the registry explains. Pull writes the registry's revision of {} and moves your alias only forward.",
            short(current),
            row.tag
        ),
        Some(LocalState::OtherImage { current }) => format!(
            "⚠ Your copy under this name is another image ({}): the registry's tag {} changed after you pulled it. Version and revision tags never move in unemupc, so another tool or a rebuilt registry moved it. Pull does not overwrite your copy; Pull… offers another local tag.",
            short(current),
            row.tag
        ),
        Some(LocalState::NotPulled { same_as: Some(tag) }) => format!(
            "Not in the library under this name. The same image is there as {tag}; Pull adds this name without fetching anything."
        ),
        Some(LocalState::NotPulled { same_as: None }) | None => {
            "Not in the library under this name.".into()
        }
    }
}

/// Whether the row is the release alias of a set or package, which a pull
/// resolves to a revision and moves forward ([`image_store::tag`]).
fn release_alias(row: &TagRow) -> bool {
    row.record
        .as_ref()
        .is_ok_and(|record| image_store::tag::governs(&record.artifact_type))
        && matches!(
            image_store::tag::parse(&row.tag),
            Some(image_store::tag::Kind::Alias { .. })
        )
}

impl LibraryView {
    /// The floppy drives of the library's machines that are stopped: every
    /// one but the machine that runs.
    pub(in crate::library_view) fn stopped_drives(&self) -> Vec<Drive> {
        let running = self.active_machine.as_ref().map(|(id, _)| id.as_str());
        self.machines
            .iter()
            .filter(|machine| Some(machine.id()) != running)
            .flat_map(|machine: &MachineRow| {
                machine.drives.iter().map(|&(unit, geometry)| {
                    let letter = crate::media_picker::drive_name(unit);
                    let pending = machine
                        .listing
                        .machine
                        .pending
                        .iter()
                        .find(|p| p.drive == letter)
                        .map(|p| self.pending_label(p));
                    let created = machine
                        .created
                        .iter()
                        .find(|c| c.drive == letter)
                        .map(|c| c.title.clone());
                    Drive {
                        machine: machine.id().to_owned(),
                        name: machine.name().to_owned(),
                        unit,
                        geometry,
                        holds: pending.or(created),
                    }
                })
            })
            .collect()
    }

    /// The selected tags of the shown registry as the model read them;
    /// with `tags`, each one's repository's tags for the pane's table.
    fn remote_selected(&self, tags: bool) -> Vec<Selected> {
        let (Some(model), Some(node)) = (&self.remote.model, &self.remote.node) else {
            return Vec::new();
        };
        let Some(registry) = model.registry(&node.host) else {
            return Vec::new();
        };
        let blocked = registry.pull_blocked();
        self.remote
            .selection()
            .into_iter()
            .filter_map(|(repository, tag)| {
                let listed = registry.repository(&repository)?;
                Some(Selected {
                    host: registry.host.clone(),
                    row: listed.row(&tag).cloned(),
                    tags: if tags {
                        super::newest_first(listed)
                            .into_iter()
                            .map(|t| {
                                let row = listed.row(&t).cloned();
                                (t, row)
                            })
                            .collect()
                    } else {
                        Vec::new()
                    },
                    repository,
                    tag,
                    blocked: blocked.clone(),
                })
            })
            .collect()
    }

    /// Whether Pull… has something to pull: a selected tag it can pull, or
    /// of several, one it does not leave out.
    pub fn can_pull_remote(&self) -> bool {
        if self.remote.node.is_none() || self.remote.model.is_none() {
            return false;
        }
        let selected = self.remote_selected(false);
        let picks: Vec<Pick<'_>> = selected.iter().map(Selected::pick).collect();
        match picks.as_slice() {
            [] => false,
            [one] => refusal(one).is_none(),
            several => several.iter().any(takes),
        }
    }

    /// Pull…, Pull and create machine… or Pull and insert… of the selected
    /// tags: the dialog opens, and pulls once the user confirms.
    pub fn begin_remote_pull(&mut self, mode: Mode) {
        let Some(accounts) = self.remote.model.as_ref().map(|m| m.accounts().clone()) else {
            return;
        };
        let selected = self.remote_selected(false);
        let picks: Vec<Pick<'_>> = selected.iter().map(Selected::pick).collect();
        let drives = self.stopped_drives();
        self.remote_pull =
            RemotePull::open(accounts, mode, &picks, self.snapshot.tags.clone(), &drives);
    }

    /// Select `tag` in the library collection of its artifact type: an image
    /// at once, a set or a package once the library lists it.
    pub(in crate::library_view) fn show_in_library(
        &mut self,
        tag: &str,
        artifact_type: &str,
        image: Descriptor,
    ) {
        match artifact_type {
            mt::MEDIA_SET | mt::FIRMWARE => {
                self.remote.node = None;
                self.selected = None;
                self.machine = None;
                self.machine_settings = None;
                self.readiness = None;
                self.pending_launch = None;
                self.clear_file_selection = true;
                self.query.text.clear();
                if artifact_type == mt::FIRMWARE {
                    self.query.filter = Filter::Firmware;
                    self.media_selected = None;
                    self.firmware_reselect = Some(tag.to_owned());
                } else {
                    self.query.filter = Filter::Media;
                    self.firmware_selected = None;
                    self.media_reselect = Some(tag.to_owned());
                }
                self.send(Request::Query(self.query.clone()));
            }
            _ => self.select_image(ImageRow {
                reference: tag.to_owned(),
                aliases: Vec::new(),
                descriptor: image,
                summary: None,
                failure: None,
            }),
        }
    }

    /// New machine from image… on a pulled image, selected in Images.
    pub(in crate::library_view) fn new_machine_from(
        &mut self,
        tag: &str,
        image: Descriptor,
        name: &str,
    ) {
        let row = ImageRow {
            reference: tag.to_owned(),
            aliases: Vec::new(),
            descriptor: image,
            summary: None,
            failure: None,
        };
        self.select_image(row.clone());
        self.new_machine_named(row, name);
    }

    /// The details pane while a remote node is shown.
    pub(in crate::library_view) fn remote_details(&mut self, ui: &mut egui::Ui) {
        let selected = self.remote_selected(true);
        let action = match selected.as_slice() {
            [] => {
                pane::meta(
                    ui,
                    "Select a tag for its details and Pull…. Ctrl (⌘ on macOS) adds a tag to the selection; Shift selects a range.",
                );
                None
            }
            [one] => self.one_tag(ui, one),
            several => several_tags(ui, several),
        };
        let Some(action) = action else {
            return;
        };
        match action {
            PaneAction::Pull(mode) => self.begin_remote_pull(mode),
            PaneAction::Contents => {
                if let [one] = selected.as_slice() {
                    self.remote.contents =
                        Some((one.host.clone(), one.repository.clone(), one.tag.clone()));
                }
            }
            PaneAction::Pick(tag) => {
                // As a click on the tag's row: the repository opens and
                // reads its other tags, and the tag reads its metadata.
                if let [one] = selected.as_slice() {
                    let mut requests = Vec::new();
                    if let Some(model) = &mut self.remote.model {
                        requests.push(model.select(&one.host, &one.repository, &tag));
                        requests.push(model.expand(&one.host, &one.repository));
                    }
                    self.remote
                        .expanded
                        .insert((one.host.clone(), one.repository.clone()));
                    self.remote.pick(
                        &one.host,
                        one.repository.clone(),
                        tag,
                        egui::Modifiers::NONE,
                    );
                    for request in requests {
                        self.send_browse(request);
                    }
                }
            }
            PaneAction::ClearSelection => self.remote.multi.clear(),
            PaneAction::ShowInLibrary | PaneAction::NewMachine | PaneAction::Insert => {
                let [one] = selected.as_slice() else {
                    return;
                };
                let Some(Ok(record)) = one.row.as_ref().map(|r| &r.record) else {
                    return;
                };
                let local_tag = one
                    .row
                    .as_ref()
                    .map(|r| r.local_tag.clone())
                    .unwrap_or_default();
                let image =
                    Descriptor::new(mt::MANIFEST, record.digest.clone(), record.manifest_size);
                match action {
                    PaneAction::ShowInLibrary => {
                        self.show_in_library(&local_tag, &record.artifact_type, image)
                    }
                    PaneAction::NewMachine => {
                        let name = record.title.clone().unwrap_or_else(|| local_tag.clone());
                        self.new_machine_from(&local_tag, image, &name);
                    }
                    _ => {
                        // Insert… of a set in the library: the media picker on
                        // the first stopped drive that takes it, the set chosen.
                        let drives = self.stopped_drives();
                        if let Some(drive) = drives_for(&record.formats, &drives).first() {
                            self.picker_wanted =
                                Some((drive.machine.clone(), drive.unit, record.digest.clone()));
                        }
                    }
                }
            }
        }
    }

    /// The pane of one selected tag.
    fn one_tag(&self, ui: &mut egui::Ui, selected: &Selected) -> Option<PaneAction> {
        let Some(row) = &selected.row else {
            ui.horizontal(|ui| {
                ui.spinner();
                pane::text(
                    ui,
                    format!(
                        "Reading {}:{} from {}…",
                        selected.repository, selected.tag, selected.host
                    ),
                );
            });
            return None;
        };
        let record = match &row.record {
            Ok(record) => record,
            Err(reason) => {
                pane::title(
                    ui,
                    Icon::Registry,
                    &format!("{}:{}", selected.repository, selected.tag),
                    false,
                );
                pane::text(
                    ui,
                    RichText::new(format!("The tag could not be read: {reason}"))
                        .color(ui.visuals().warn_fg_color),
                );
                return None;
            }
        };
        let mut action = None;
        let group = Group::of(&record.artifact_type);
        let title = record
            .title
            .clone()
            .unwrap_or_else(|| format!("{}:{}", selected.repository, selected.tag));
        let template =
            type_label(&record.artifact_type, &selected.repository) == "Machine template";
        let kind = match group {
            Some(Group::Machines) if template => "TEMPLATE",
            Some(Group::Machines) => "IMAGE",
            Some(Group::Media) => "MEDIA SET",
            Some(Group::Firmware) => "FIRMWARE",
            None => "ARTIFACT",
        };
        let annotation = |key: &str| {
            record
                .annotations
                .get(key)
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
        };
        let details = record.details.as_ref();
        // The edition, and a media set's version, come from the metadata
        // record the selection reads.
        let reading = details.is_none();
        let metadata = details.and_then(|d| d.as_ref().ok());
        let version = metadata
            .and_then(|d| d.version.clone())
            .or_else(|| record.version.clone())
            .unwrap_or_else(|| if reading { "Reading…" } else { "—" }.into());
        let edition = metadata
            .and_then(|d| d.edition.clone())
            .unwrap_or_else(|| if reading { "Reading…" } else { "—" }.into());
        let head = |ui: &mut egui::Ui| {
            pane::head(
                ui,
                &pane::Tile {
                    title: &title,
                    kind,
                    user_supplied: record.user_supplied,
                },
                |ui| {
                    let icon = group.map_or(Icon::Registry, Group::icon);
                    pane::title(ui, icon, &title, false);
                    pane::meta(
                        ui,
                        format!(
                            "{}/{}:{} · {} · {}",
                            selected.host,
                            selected.repository,
                            selected.tag,
                            short(&record.digest),
                            human_size(record.size)
                        ),
                    );
                    if let Some(description) = annotation(DESCRIPTION) {
                        pane::text(ui, description);
                    }
                },
            );
        };
        let first = |ui: &mut egui::Ui| {
            pane::section(ui, "Annotations, from the manifest");
            pane::facts(ui, "remote-annotations", |ui| {
                pane::fact_text(ui, "Title", title.as_str());
                if let Some(vendor) = annotation(VENDOR) {
                    pane::fact_text(ui, "Vendor", vendor);
                }
                pane::fact_text(ui, "Version", version.as_str());
                pane::fact_text(ui, "Edition", edition.as_str());
                if !record.media.is_empty() {
                    pane::fact_text(ui, "Media", media_line(&record.media));
                }
                pane::fact(ui, "Licence", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        pane::rights(ui, !record.user_supplied);
                        match annotation(LICENSES) {
                            Some(licence) => {
                                ui.add(egui::Label::new(pane::mono(licence)).wrap());
                            }
                            None => {
                                ui.label(RichText::new("no licence recorded").weak());
                            }
                        }
                    });
                });
                if let Some(source) = annotation(SOURCE) {
                    pane::fact_text(ui, "Source", source);
                }
                pane::fact(ui, "Type", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(type_label(&record.artifact_type, &selected.repository));
                        ui.label(pane::mono(&record.artifact_type));
                    });
                });
                pane::fact(ui, "Digest", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(pane::mono(short(&record.digest)))
                            .on_hover_text(record.digest.to_string());
                        ui.label(RichText::new("· Pull uses this digest").weak());
                    });
                });
            });
            if let Some(Err(reason)) = details {
                pane::meta(
                    ui,
                    format!("The metadata record could not be read: {reason}"),
                );
            }
        };
        let second = |ui: &mut egui::Ui| {
            pane::section(ui, "In your library");
            pane::facts(ui, "remote-local", |ui| {
                pane::fact(ui, "Local tag", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.add(egui::Label::new(pane::mono(&row.local_tag)).wrap());
                        if selected.host == DEFAULT_REGISTRY {
                            ui.label(
                                RichText::new("· the default registry keeps short names").weak(),
                            );
                        }
                    });
                });
                pane::fact_text(ui, "Source", selected.host.as_str());
            });
            let sentence = local_sentence(row);
            let colour = match row.local {
                Some(LocalState::InLibrary) => pane::good_colour(ui.visuals()),
                Some(LocalState::OtherImage { .. } | LocalState::NewerAvailable { .. }) => {
                    ui.visuals().warn_fg_color
                }
                _ => ui.visuals().text_color(),
            };
            pane::text(ui, RichText::new(sentence).color(colour));
            pane::meta(
                ui,
                match row.download() {
                    None => "Download: nothing.".to_owned(),
                    Some(download) => format!(
                        "Download: {} to fetch of {}; {} of {} blobs already in the store.",
                        human_size(download.bytes),
                        human_size(record.size),
                        download.present,
                        download.blobs
                    ),
                },
            );
            if selected.tags.len() > 1 {
                pane::section(ui, "Tags of this repository");
                egui::Grid::new("remote-tags")
                    .num_columns(3)
                    .striped(true)
                    .spacing([12.0, 2.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Tag").weak());
                        ui.label(RichText::new("Digest").weak());
                        ui.label(RichText::new("Local state").weak());
                        ui.end_row();
                        for (tag, other) in &selected.tags {
                            let current = *tag == selected.tag;
                            let label = RichText::new(tag).monospace();
                            let label = if current { label.strong() } else { label };
                            if ui.link(label).clicked() && !current {
                                action = Some(PaneAction::Pick(tag.clone()));
                            }
                            let digest = other
                                .as_ref()
                                .and_then(|o| o.record.as_ref().ok())
                                .map_or_else(|| "—".into(), |r| short(&r.digest));
                            ui.label(pane::mono(digest));
                            ui.label(state_text(other.as_ref()));
                            ui.end_row();
                        }
                    });
            }
            pane::meta(
                ui,
                "Read from the registry: the tag list and this manifest. No layer, cover or disk has been fetched.",
            );
        };
        pane::split(ui, 0.55, head, first, second);
        let in_library = row.local == Some(LocalState::InLibrary);
        let refused = refusal(&selected.pick());
        // A tag in the library still makes machines and goes into drives,
        // unless this release cannot run it.
        let blocked = record.not_runnable().is_some() || (refused.is_some() && !in_library);
        let formats = &record.formats;
        let drives = self.stopped_drives();
        let fits = !drives_for(formats, &drives).is_empty();
        let mut why = refused.clone();
        pane::actions(ui, |ui| {
            if in_library {
                let show = pane::primary(ui, "Show in library");
                if ui.add(show).clicked() {
                    action = Some(PaneAction::ShowInLibrary);
                }
            }
            let pull = if in_library {
                egui::Button::new("Pull…")
            } else {
                pane::primary(ui, "Pull…")
            };
            if ui.add_enabled(refused.is_none(), pull).clicked() {
                action = Some(PaneAction::Pull(Mode::Pull));
            }
            if group == Some(Group::Machines) {
                let label = if in_library {
                    "New machine from image…"
                } else {
                    "Pull and create machine…"
                };
                if ui.add_enabled(!blocked, egui::Button::new(label)).clicked() {
                    action = Some(if in_library {
                        PaneAction::NewMachine
                    } else {
                        PaneAction::Pull(Mode::CreateMachine)
                    });
                }
            }
            if group == Some(Group::Media) {
                let label = if in_library {
                    "Insert…"
                } else {
                    "Pull and insert…"
                };
                if ui
                    .add_enabled(!blocked && fits, egui::Button::new(label))
                    .clicked()
                {
                    action = Some(if in_library {
                        PaneAction::Insert
                    } else {
                        PaneAction::Pull(Mode::Insert)
                    });
                }
                if !fits && why.is_none() {
                    why = Some(if formats.iter().any(|f| f == "iso-2048") {
                        "Pull and insert… takes floppy sets only: no machine here takes CD images yet."
                            .to_owned()
                    } else {
                        format!(
                            "Pull and insert… needs a stopped machine with a drive that takes {} disks; none has one.",
                            formats.join(" and ")
                        )
                    });
                }
            }
            if ui.button("Contents…").clicked() {
                action = Some(PaneAction::Contents);
            }
        });
        if let Some(why) = why {
            pane::text(
                ui,
                RichText::new(why).small().color(ui.visuals().warn_fg_color),
            );
        }
        action
    }

    /// Contents… of a remote tag: every descriptor of its manifest, with
    /// the size and redistribution mark it carries and whether the store
    /// holds it, read from the manifest alone.
    pub(in crate::library_view) fn remote_contents(&mut self, ctx: &egui::Context, classic: bool) {
        let Some((host, repository, tag)) = self.remote.contents.clone() else {
            return;
        };
        let row = self
            .remote
            .model
            .as_ref()
            .and_then(|m| m.registry(&host))
            .and_then(|v| v.repository(&repository))
            .and_then(|r| r.row(&tag))
            .cloned();
        let Some((row, Ok(record))) = row.map(|r| {
            let record = r.record.clone();
            (r, record)
        }) else {
            self.remote.contents = None;
            return;
        };
        let mut close = false;
        egui::Modal::new(egui::Id::unique("remote-contents")).show(ctx, |ui| {
            // The mock-up's wide dialog, narrower in a narrow window.
            ui.set_width((ctx.content_rect().width() - 48.0).clamp(320.0, 760.0));
            let heading = format!("Contents — {repository}:{tag}");
            if classic {
                ui_kit::skin::gradient_title(ui, &heading);
            } else {
                ui.heading(&heading);
            }
            pane::style(ui);
            pane::text(
                ui,
                "Every part the manifest names, with the size and redistribution mark its descriptor carries. Read from the manifest alone; nothing here has been fetched.",
            );
            pane::facts(ui, "remote-contents-facts", |ui| {
                pane::fact_text(
                    ui,
                    "From",
                    format!("{host}/{repository}:{tag} · {}", short(&record.digest)),
                );
                let what = if row.local == Some(LocalState::InLibrary) {
                    "in the library"
                } else {
                    "what Pull would write"
                };
                pane::fact_text(ui, "Local tag", format!("{} · {what}", row.local_tag));
            });
            let manifest_stored = matches!(
                row.local,
                Some(LocalState::InLibrary | LocalState::NotPulled { same_as: Some(_) })
            );
            let mark = |open: bool| if open { "open" } else { "user-supplied" };
            let held = |stored: bool| {
                if stored {
                    ("Yes — not fetched again".to_owned(), pane::Tone::Body)
                } else {
                    ("No".to_owned(), pane::Tone::Muted)
                }
            };
            let mut rows = vec![pane::Row {
                head: "Manifest".into(),
                cells: vec![
                    vec![("image manifest".into(), pane::Tone::Body)],
                    vec![(human_size(record.manifest_size), pane::Tone::Body)],
                    vec![(mark(!record.user_supplied).into(), pane::Tone::Body)],
                    vec![held(manifest_stored)],
                ],
                changed: false,
            }];
            for descriptor in &record.descriptors {
                let kind = kind_label(descriptor);
                rows.push(pane::Row {
                    head: descriptor
                        .annotations
                        .get(TITLE)
                        .cloned()
                        .unwrap_or_else(|| kind.clone()),
                    cells: vec![
                        vec![(kind, pane::Tone::Body)],
                        vec![(human_size(descriptor.size), pane::Tone::Body)],
                        vec![(mark(open(descriptor)).into(), pane::Tone::Body)],
                        vec![held(row.stored.contains(&descriptor.digest))],
                    ],
                    changed: false,
                });
            }
            egui::ScrollArea::vertical()
                .id_salt("remote-contents-parts")
                .max_height(340.0)
                .show(ui, |ui| {
                    pane::table(
                        ui,
                        "remote-contents-table",
                        &["Part", "Kind", "Size", "Redistribution", "In the store"],
                        &rows,
                    );
                });
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = ui.button("Close").clicked();
            });
        });
        if close || ctx.input_mut(ui_kit::skin::escape_pressed) {
            self.remote.contents = None;
        }
    }
}

/// The pane of several selected tags: each with its source, size and local
/// state, and Pull… for those it does not leave out.
fn several_tags(ui: &mut egui::Ui, selected: &[Selected]) -> Option<PaneAction> {
    let mut action = None;
    pane::title(
        ui,
        Icon::Registry,
        &format!("{} tags selected", selected.len()),
        false,
    );
    pane::meta(
        ui,
        "Ctrl (⌘ on macOS) adds or removes a row; Shift selects a range.",
    );
    let rows: Vec<pane::Row> = selected
        .iter()
        .map(|s| pane::Row {
            head: format!("{}:{}", s.repository, s.tag),
            cells: vec![
                vec![(s.host.clone(), pane::Tone::Body)],
                vec![(
                    s.row
                        .as_ref()
                        .and_then(|r| r.record.as_ref().ok())
                        .map_or_else(|| "—".into(), |r| human_size(r.size)),
                    pane::Tone::Body,
                )],
                vec![(state_text(s.row.as_ref()), pane::Tone::Body)],
            ],
            changed: false,
        })
        .collect();
    pane::table(
        ui,
        "remote-selected-tags",
        &["Name", "Source", "Size", "Local state"],
        &rows,
    );
    pane::meta(
        ui,
        "Pull… takes them one after another under one progress bar, each by the digest the list showed. Tags already in the library are skipped, and so is a tag whose local name holds another image: pull that one alone, under another tag.",
    );
    let pullable = selected.iter().map(Selected::pick).filter(takes).count();
    pane::actions(ui, |ui| {
        let label = match pullable {
            1 => "Pull 1 tag…".to_owned(),
            n => format!("Pull {n} tags…"),
        };
        let pull = pane::primary(ui, &label);
        if ui.add_enabled(pullable > 0, pull).clicked() {
            action = Some(PaneAction::Pull(Mode::Pull));
        }
        if ui.button("Clear selection").clicked() {
            action = Some(PaneAction::ClearSelection);
        }
    });
    action
}
