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

use super::{type_label, Group, RemotesAction, RemotesContext, RemotesView};
use egui::RichText;
use image_registry::DEFAULT_REGISTRY;
use image_store::{license, media_type as mt, Descriptor, Digest};
use library_model::{
    listing::human_size,
    registry_transfer::kind_label,
    remote_browse::{LocalState, TagRow},
    remote_pull::{drives_for, refusal, Mode, Pick, RemotePull},
    PlatformFacts,
};
use library_model::{push_destination::Place, remote_browse::State};
use ui_kit::{pane, shell_icons::Icon};

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
    /// The platforms this program runs, to tell a tag it cannot run.
    facts: PlatformFacts,
}

impl Selected {
    fn pick(&self) -> Pick<'_> {
        Pick {
            host: &self.host,
            repository: &self.repository,
            tag: &self.tag,
            row: self.row.as_ref(),
            blocked: self.blocked.clone(),
            facts: &self.facts,
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
    format!("sha256:{}", library_model::listing::short(digest.hex()))
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
fn state_text(row: Option<&TagRow>, facts: &PlatformFacts) -> String {
    let Some(row) = row else {
        return "Not read yet".into();
    };
    let Ok(record) = &row.record else {
        return "Could not be read".into();
    };
    if let Some(reason) = record.not_runnable(facts) {
        return reason;
    }
    match &row.local {
        Some(LocalState::InLibrary) => "In library".into(),
        Some(LocalState::NewerAvailable { have }) => format!("Newer available · you have {have}"),
        Some(LocalState::NewerRelease { version, .. }) => {
            format!("Newer available · you have {version}")
        }
        Some(LocalState::LocalNewer { have }) => format!("Local is newer: {have}"),
        Some(LocalState::OtherImage { .. }) => "⚠ Local tag names another image".into(),
        Some(LocalState::NotPulled { same_as: Some(tag) }) => {
            format!("Not pulled · same image as {tag}")
        }
        Some(LocalState::NotPulled { same_as: None }) | None => "Not pulled".into(),
        Some(LocalState::PushedFromHere { tag }) => format!("Pushed from here · as {tag}"),
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
        Some(
            LocalState::NewerAvailable { have } | LocalState::NewerRelease { have, .. },
        ) => format!(
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
        Some(LocalState::PushedFromHere { tag }) => format!(
            "Pushed from this library, where it is {tag}. Pushing did not rename it. Pull adds the name {} beside it without fetching anything.",
            row.local_tag
        ),
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

impl RemotesView {
    /// The selected tags of the shown registry as the model read them;
    /// with `tags`, each one's repository's tags for the pane's table.
    fn remote_selected(&self, tags: bool, facts: &PlatformFacts) -> Vec<Selected> {
        let (Some(model), Some(node)) = (&self.model, &self.node) else {
            return Vec::new();
        };
        let Some(registry) = model.registry(&node.host) else {
            return Vec::new();
        };
        let blocked = registry.pull_blocked();
        self.selection()
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
                    facts: facts.clone(),
                })
            })
            .collect()
    }

    /// Whether Pull… has something to pull: a selected tag it can pull, or
    /// of several, one it does not leave out.
    pub fn can_pull_remote(&self, context: &RemotesContext<'_>) -> bool {
        if self.node.is_none() || self.model.is_none() {
            return false;
        }
        let selected = self.remote_selected(false, context.facts);
        let picks: Vec<Pick<'_>> = selected.iter().map(Selected::pick).collect();
        match picks.as_slice() {
            [] => false,
            [one] => refusal(one).is_none(),
            several => several.iter().any(takes),
        }
    }

    /// Pull…, Pull and create machine… or Pull and insert… of the selected
    /// tags: the dialog opens in `dialog`, and pulls once the user confirms.
    /// Before the shell hands over the logins, `dialog` stays as it is.
    pub fn begin_remote_pull(
        &self,
        mode: Mode,
        context: &RemotesContext<'_>,
        dialog: &mut Option<RemotePull>,
    ) {
        let Some(accounts) = self.model.as_ref().map(|m| m.accounts().clone()) else {
            return;
        };
        let selected = self.remote_selected(false, context.facts);
        let picks: Vec<Pick<'_>> = selected.iter().map(Selected::pick).collect();
        *dialog = RemotePull::open(
            accounts,
            mode,
            &picks,
            context.tags.to_vec(),
            context.drives,
        );
    }

    /// The details pane while a remote node is shown.
    pub fn remote_details(
        &mut self,
        ui: &mut egui::Ui,
        context: &RemotesContext<'_>,
    ) -> Vec<RemotesAction> {
        let selected = self.remote_selected(true, context.facts);
        let action = match selected.as_slice() {
            // A node's own pane shows until a tag is selected.
            [] => return self.node_pane(ui, context),
            [one] => self.one_tag(ui, one, context),
            several => several_tags(ui, several),
        };
        let Some(action) = action else {
            return Vec::new();
        };
        match action {
            PaneAction::Pull(mode) => vec![RemotesAction::BeginPull(mode)],
            PaneAction::Contents => {
                if let [one] = selected.as_slice() {
                    self.contents =
                        Some((one.host.clone(), one.repository.clone(), one.tag.clone()));
                }
                Vec::new()
            }
            PaneAction::Pick(tag) => {
                // As a click on the tag's row: the repository opens and
                // reads its other tags, and the tag reads its metadata.
                let [one] = selected.as_slice() else {
                    return Vec::new();
                };
                let mut requests = Vec::new();
                if let Some(model) = &mut self.model {
                    requests.push(model.select(&one.host, &one.repository, &tag));
                    requests.push(model.expand(&one.host, &one.repository));
                }
                self.expanded
                    .insert((one.host.clone(), one.repository.clone()));
                self.pick(
                    &one.host,
                    one.repository.clone(),
                    tag,
                    egui::Modifiers::NONE,
                );
                requests.into_iter().flat_map(super::browse).collect()
            }
            PaneAction::ClearSelection => {
                self.multi.clear();
                Vec::new()
            }
            PaneAction::ShowInLibrary | PaneAction::NewMachine | PaneAction::Insert => {
                let [one] = selected.as_slice() else {
                    return Vec::new();
                };
                let Some(Ok(record)) = one.row.as_ref().map(|r| &r.record) else {
                    return Vec::new();
                };
                // A tag pushed from here is in the library under the tag it
                // was pushed from: pushing never renames.
                let local_tag = one
                    .row
                    .as_ref()
                    .map(|r| match &r.local {
                        Some(LocalState::PushedFromHere { tag }) => tag.clone(),
                        _ => r.local_tag.clone(),
                    })
                    .unwrap_or_default();
                let image =
                    Descriptor::new(mt::MANIFEST, record.digest.clone(), record.manifest_size);
                match action {
                    PaneAction::ShowInLibrary => vec![RemotesAction::ShowInLibrary {
                        tag: local_tag,
                        artifact_type: record.artifact_type.clone(),
                        image,
                    }],
                    PaneAction::NewMachine => {
                        let name = record.title.clone().unwrap_or_else(|| local_tag.clone());
                        vec![RemotesAction::NewMachine {
                            tag: local_tag,
                            image,
                            name,
                        }]
                    }
                    _ => {
                        // Insert… of a set in the library: the media picker on
                        // the first stopped drive that takes it, the set chosen.
                        drives_for(&record.formats, context.drives)
                            .first()
                            .map(|drive| RemotesAction::Insert {
                                machine: drive.machine.clone(),
                                unit: drive.unit,
                                set: record.digest.clone(),
                            })
                            .into_iter()
                            .collect()
                    }
                }
            }
        }
    }

    /// The place Push here… of a node pushes to: a namespace node's
    /// namespace; for a registry's node its only namespace, or none, the
    /// whole registry, or a registry of several namespaces, where the
    /// dialog asks which one, as the chooser does.
    pub fn node_place(&self, node: &super::Node) -> Option<Place> {
        let view = self.model.as_ref()?.registry(&node.host)?;
        let namespace = match &node.namespace {
            Some(namespace) => namespace.clone(),
            None => match view.entry.namespaces.as_slice() {
                [only] => Some(only.name.clone()),
                _ => None,
            },
        };
        Some(Place {
            host: node.host.clone(),
            namespace,
        })
    }

    /// Why Push here… waits: the registry does not answer.
    pub fn push_blocked(&self, host: &str) -> Option<String> {
        let view = self.model.as_ref()?.registry(host)?;
        matches!(view.state, State::Unreachable { .. }).then(|| {
            format!("{host} does not answer; Push here… waits until Try again reaches it.")
        })
    }

    /// The pane of the shown node while no tag is selected: the registry's
    /// or the namespace's, with Push here…, Refresh, Registries… and, for a
    /// private one before a login, Log in….
    fn node_pane(&mut self, ui: &mut egui::Ui, context: &RemotesContext<'_>) -> Vec<RemotesAction> {
        let Some(node) = self.node.clone() else {
            return Vec::new();
        };
        let Some(view) = self.model.as_ref().and_then(|m| m.registry(&node.host)) else {
            return Vec::new();
        };
        let host = view.host.clone();
        let official = host == DEFAULT_REGISTRY;
        // The environment's login pair for this host counts, as pushes and
        // pulls use it before the stored login.
        let env = self
            .model
            .as_ref()
            .map(|m| m.accounts().env.clone())
            .unwrap_or_default();
        let signed_in = env
            .login_for(&host)
            .map(str::to_owned)
            .or_else(|| view.entry.user.clone());
        let blocked = self.push_blocked(&host);
        let listed =
            matches!(view.state, State::Listed | State::Listing | State::Stopped) || view.stale();
        let tail = !view.repositories.is_empty();
        let clicked = match &node.namespace {
            None => {
                let kind = super::kind(view);
                let whole = view.entry.namespaces.is_empty();
                let namespaces: Vec<String> = view
                    .namespaces()
                    .iter()
                    .map(|n| {
                        let name = n
                            .name
                            .as_deref()
                            .map_or("(top level)".into(), |n| format!("{n}/"));
                        if n.private {
                            format!("{name} — private; login required to list and pull")
                        } else {
                            format!("{name} — public; open content only")
                        }
                    })
                    .collect();
                let state = match &view.state {
                    State::Idle => "Not listed in this session".to_owned(),
                    State::Listing => "Listing…".into(),
                    State::Listed if view.repositories.is_empty() => {
                        "Listed: nothing published yet".into()
                    }
                    State::Listed => "Listed in this session".into(),
                    State::Stopped => "Listing stopped".into(),
                    State::Unreachable { stale: true, .. } => {
                        "Unreachable now; rows from an earlier listing".into()
                    }
                    State::Unreachable { reason, .. } => format!("Unreachable: {reason}"),
                    State::LoginRequired { .. } => "Login required (401)".into(),
                    State::ServerError { status, reason } => match status {
                        Some(status) => format!("The catalog request failed: {status} {reason}"),
                        None => format!("The catalog request failed: {reason}"),
                    },
                    State::Failed(message) => message.clone(),
                };
                pane::head(
                    ui,
                    &pane::Tile {
                        title: &host,
                        kind: "REGISTRY",
                        user_supplied: false,
                    },
                    |ui| {
                        ui.horizontal_wrapped(|ui| {
                            pane::title(ui, Icon::Registry, &host, false);
                            pane::badge(ui, kind, true);
                            if official {
                                pane::badge(ui, "default", true);
                            }
                        });
                        pane::facts(ui, "registry-pane", |ui| {
                            pane::fact_text(
                                ui,
                                "Namespaces",
                                if whole {
                                    format!(
                                        "Whole registry; namespaces from the first path components the catalog returns: {}",
                                        namespaces.join(", ")
                                    )
                                } else {
                                    namespaces.join("\n")
                                },
                            );
                            pane::fact_text(
                                ui,
                                "Login",
                                match (official, &signed_in) {
                                    (true, Some(user)) => format!(
                                        "Not needed to list or pull unemu/; signed in as {user} for a push"
                                    ),
                                    (true, None) => {
                                        "Not needed to list or pull unemu/; a push needs one".into()
                                    }
                                    (false, Some(user)) => format!("Signed in as {user}"),
                                    (false, None) => "Not signed in".into(),
                                },
                            );
                            pane::fact_text(
                                ui,
                                "Local tags",
                                if official {
                                    "Short names: open-at:1.2.0, unemu/media/freedos:1.4".to_owned()
                                } else {
                                    format!("With the host: {host}/…:tag")
                                },
                            );
                            pane::fact_text(
                                ui,
                                "Push here",
                                if official {
                                    "unemu/ takes only images whose every blob is open"
                                } else if kind == "private" {
                                    "Takes images with user-supplied content: marked private. Before the first upload to a repository, anonymous reads are tried and must be refused"
                                } else {
                                    "Takes only images whose every blob is open"
                                },
                            );
                            pane::fact_text(ui, "Listing", state);
                            pane::fact_text(
                                ui,
                                "Read",
                                "The registry catalog, tag lists and manifests only, on the library worker. Nothing is written to disk.",
                            );
                        });
                    },
                );
                let login = kind == "private" && signed_in.is_none();
                node_buttons(ui, login, true, blocked.as_deref())
            }
            Some(namespace) => {
                let name = namespace.as_deref().unwrap_or("");
                let namespaces = view.namespaces();
                let found = namespaces.iter().find(|n| n.name == *namespace);
                let private = found.map_or(view.entry.private, |n| n.private);
                let repositories = found.map_or(0, |n| n.repositories.len());
                let groups: Vec<String> = super::Group::ALL
                    .into_iter()
                    .filter_map(|group| {
                        let count = found.map_or(0, |n| {
                            n.repositories
                                .iter()
                                .filter(|r| super::group_of(r) == Some(group))
                                .count()
                        });
                        (count > 0).then(|| format!("{count} {}", group.label().to_lowercase()))
                    })
                    .collect();
                let locked = private && signed_in.is_none();
                let label = format!("{host}/{name}/");
                pane::head(
                    ui,
                    &pane::Tile {
                        title: &format!("{name}/"),
                        kind: "NAMESPACE",
                        user_supplied: private,
                    },
                    |ui| {
                        ui.horizontal_wrapped(|ui| {
                            pane::title(ui, Icon::Namespace, &label, false);
                            pane::badge(ui, if private { "private" } else { "public" }, true);
                        });
                        pane::meta(
                            ui,
                            format!(
                                "A namespace of {host} · {}",
                                if private {
                                    "a login is required to list, pull and push"
                                } else {
                                    "anyone may list and pull; a push needs a login"
                                }
                            ),
                        );
                        pane::facts(ui, "namespace-pane", |ui| {
                            pane::fact_text(
                                ui,
                                "Repositories",
                                if listed && !locked {
                                    let mut text = format!("{repositories} listed in this session");
                                    if !groups.is_empty() {
                                        text += &format!(": {}", groups.join(", "));
                                    }
                                    text
                                } else if locked {
                                    "Not listed: log in first".into()
                                } else {
                                    "Not listed in this session".into()
                                },
                            );
                            pane::fact_text(
                                ui,
                                "Login",
                                match &signed_in {
                                    Some(user) => format!("Signed in as {user}"),
                                    None => "Not signed in".into(),
                                },
                            );
                            pane::fact_text(
                                ui,
                                "Push here",
                                if name == image_registry::reference::OFFICIAL_NAMESPACE {
                                    "Takes only images whose every blob is open, with their source archives and terms. Images with user-supplied content are refused, with no override."
                                } else if private {
                                    "Takes images with user-supplied content: the namespace is marked private and is not unemu/. Before the first upload to a repository, anonymous reads are tried and must be refused."
                                } else {
                                    "Takes only images whose every blob is open: the namespace is not marked private."
                                },
                            );
                            if let Some(counts) = &context.here {
                                let mut text =
                                    format!("{} of {} items can go here", counts.can, counts.total);
                                if signed_in.is_none() {
                                    text += " after you log in";
                                }
                                if counts.same > 0 {
                                    text += &format!(
                                        "; {} {} here already",
                                        counts.same,
                                        if counts.same == 1 { "is" } else { "are" }
                                    );
                                }
                                text += ". Each keeps its local tag.";
                                pane::fact_text(ui, "From your library", text);
                            }
                        });
                    },
                );
                node_buttons(ui, locked, !locked, blocked.as_deref())
            }
        };
        if tail {
            pane::meta(
                ui,
                "Select a tag for its details and Pull…. Ctrl (⌘ on macOS) adds a tag to the selection; Shift selects a range.",
            );
        }
        let Some(clicked) = clicked else {
            return Vec::new();
        };
        match clicked {
            NodeButton::LogIn => {
                self.login = Some(host.clone());
                vec![RemotesAction::LogIn(host)]
            }
            NodeButton::PushHere => self
                .node_place(&node)
                .map(RemotesAction::PushHere)
                .into_iter()
                .collect(),
            NodeButton::Refresh => {
                let request = self.model.as_mut().and_then(|m| m.refresh(&host));
                self.collapse_unread(Some(&host));
                super::browse(request)
            }
            NodeButton::Registries => vec![RemotesAction::Registries],
        }
    }

    /// The pane of one selected tag.
    fn one_tag(
        &self,
        ui: &mut egui::Ui,
        selected: &Selected,
        context: &RemotesContext<'_>,
    ) -> Option<PaneAction> {
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
        let pushed = match &row.local {
            Some(LocalState::PushedFromHere { tag }) => Some(tag.clone()),
            _ => None,
        };
        let second = |ui: &mut egui::Ui| {
            pane::section(ui, "In your library");
            pane::facts(ui, "remote-local", |ui| {
                pane::fact(ui, "Local tag", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.add(
                            egui::Label::new(pane::mono(
                                pushed.as_deref().unwrap_or(&row.local_tag),
                            ))
                            .wrap(),
                        );
                        if pushed.is_some() {
                            ui.label(RichText::new("· pushed from here, not renamed").weak());
                        }
                        if selected.host == DEFAULT_REGISTRY {
                            ui.label(
                                RichText::new("· the default registry keeps short names").weak(),
                            );
                        }
                    });
                });
                match &pushed {
                    Some(_) => pane::fact_text(ui, "Source", format!("local → {}", selected.host)),
                    None => pane::fact_text(ui, "Source", selected.host.as_str()),
                }
            });
            let sentence = local_sentence(row);
            let colour = match row.local {
                Some(LocalState::InLibrary | LocalState::PushedFromHere { .. }) => {
                    pane::good_colour(ui.visuals())
                }
                Some(
                    LocalState::OtherImage { .. }
                    | LocalState::NewerAvailable { .. }
                    | LocalState::NewerRelease { .. },
                ) => ui.visuals().warn_fg_color,
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
                            ui.label(state_text(other.as_ref(), &selected.facts));
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
        let pushed_here = matches!(row.local, Some(LocalState::PushedFromHere { .. }));
        let refused = refusal(&selected.pick());
        // A tag in the library still makes machines and goes into drives,
        // unless this release cannot run it.
        let blocked =
            record.not_runnable(&selected.facts).is_some() || (refused.is_some() && !in_library);
        let formats = &record.formats;
        let fits = !drives_for(formats, context.drives).is_empty();
        let mut why = refused.clone();
        pane::actions(ui, |ui| {
            if in_library || pushed_here {
                let show = pane::primary(ui, "Show in library");
                if ui.add(show).clicked() {
                    action = Some(PaneAction::ShowInLibrary);
                }
            }
            let pull = if in_library || pushed_here {
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
    pub fn remote_contents(&mut self, ctx: &egui::Context, context: &RemotesContext<'_>) {
        let Some((host, repository, tag)) = self.contents.clone() else {
            return;
        };
        let row = self
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
            self.contents = None;
            return;
        };
        let mut close = false;
        let facts = context.facts;
        let classic = context.classic;
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
                let kind = kind_label(facts, descriptor);
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
            self.contents = None;
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
                vec![(state_text(s.row.as_ref(), &s.facts), pane::Tone::Body)],
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

/// A button of a node's pane.
enum NodeButton {
    LogIn,
    PushHere,
    Refresh,
    Registries,
}

/// The node pane's buttons: Log in… first and as the default while a
/// private place has no login, else Push here…; Refresh; Registries….
fn node_buttons(
    ui: &mut egui::Ui,
    login: bool,
    push_first: bool,
    blocked: Option<&str>,
) -> Option<NodeButton> {
    let mut clicked = None;
    pane::actions(ui, |ui| {
        if login && ui.add(pane::primary(ui, "Log in…")).clicked() {
            clicked = Some(NodeButton::LogIn);
        }
        let push = if push_first && !login {
            pane::primary(ui, "Push here…")
        } else {
            egui::Button::new("Push here…")
        };
        if ui.add_enabled(blocked.is_none(), push).clicked() {
            clicked = Some(NodeButton::PushHere);
        }
        if ui.button("Refresh").clicked() {
            clicked = Some(NodeButton::Refresh);
        }
        if ui.button("Registries…").clicked() {
            clicked = Some(NodeButton::Registries);
        }
    });
    if let Some(why) = blocked {
        pane::text(
            ui,
            RichText::new(why).small().color(ui.visuals().warn_fg_color),
        );
    }
    clicked
}
