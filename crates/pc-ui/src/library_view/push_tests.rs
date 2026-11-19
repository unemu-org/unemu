// SPDX-License-Identifier: AGPL-3.0-or-later
//! Push to… and Push here… drawn headless through the whole library frame,
//! read back through AccessKit, against the in-process mock registry and a
//! real library worker: each state of the approved remotes-push mock-up
//! that has behaviour (`backlog/docs/design/mockups/remotes-push.html`).

use super::{remotes::Node, Filter, ImageRow, LibraryView};
use crate::{
    registry_transfer::{test_support::Lab, PushState, TransferRequest},
    remote_browse::{local_tag_of, LocalState, RepositoryRow, State, TagRow},
};
use egui::{accesskit::Role, Rect};
use image_store::{Descriptor, Digest};
use std::time::{Duration, Instant};

const SIZE: egui::Vec2 = egui::vec2(1400.0, 1000.0);

/// A widget as egui reported it to AccessKit.
#[derive(Clone, Debug)]
struct Widget {
    role: Role,
    label: String,
    rect: Rect,
    enabled: bool,
}

/// What a frame showed: its texts and its AccessKit widgets.
struct Shown {
    texts: Vec<String>,
    widgets: Vec<Widget>,
}

impl Shown {
    fn of(output: &egui::FullOutput) -> Self {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts);
        }
        let widgets = output
            .platform_output
            .accesskit_update
            .as_ref()
            .map(|update| {
                update
                    .nodes
                    .iter()
                    .filter_map(|(_, node)| {
                        let area = node.bounds()?;
                        Some(Widget {
                            role: node.role(),
                            label: node.label().or(node.value()).unwrap_or_default().to_owned(),
                            rect: Rect::from_min_max(
                                egui::pos2(area.x0 as f32, area.y0 as f32),
                                egui::pos2(area.x1 as f32, area.y1 as f32),
                            ),
                            enabled: !node.is_disabled(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { texts, widgets }
    }

    /// The text is painted, or a widget says it.
    fn shows(&self, text: &str) -> bool {
        self.texts.iter().any(|t| t.contains(text))
            || self.widgets.iter().any(|w| w.label.contains(text))
    }

    #[track_caller]
    fn widget(&self, label: &str) -> &Widget {
        self.widgets
            .iter()
            .find(|w| w.label == label)
            .unwrap_or_else(|| {
                let labels: Vec<_> = self.widgets.iter().map(|w| &w.label).collect();
                panic!("no widget {label:?} in {labels:?}")
            })
    }

    fn button(&self, label: &str) -> &Widget {
        self.widgets
            .iter()
            .find(|w| w.label == label && w.role == Role::Button)
            .unwrap_or_else(|| self.widget(label))
    }

    /// The value of the text field after the label `label`.
    fn field(&self, role: Role, below: &str) -> String {
        let top = self.widget(below).rect.center().y;
        self.widgets
            .iter()
            .filter(|w| w.role == role)
            .min_by(|a, b| {
                (a.rect.center().y - top)
                    .abs()
                    .total_cmp(&(b.rect.center().y - top).abs())
            })
            .map(|w| w.label.clone())
            .unwrap_or_default()
    }
}

/// The library frame and its dialogs, frame by frame, as the shell draws
/// them; the requests the dialogs make go to the worker.
struct Screen {
    ctx: egui::Context,
    time: f64,
    file: Option<usize>,
    /// The commands the frame asked the shell for.
    commands: Vec<crate::commands::Command>,
}

impl Screen {
    fn new() -> Self {
        let ctx = egui::Context::default();
        ui_kit::skin::configure_context(&ctx);
        ctx.set_visuals(ui_kit::skin::skin_visuals(true, false));
        ctx.enable_accesskit();
        Self {
            ctx,
            time: 0.0,
            file: None,
            commands: Vec::new(),
        }
    }

    fn frame_with(
        &mut self,
        view: &mut LibraryView,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> Shown {
        self.time += 1.0 / 60.0;
        let mut file = self.file;
        // The modifiers held, as the platform reports them before the events.
        let events = std::iter::once(egui::Event::ModifiersChanged(modifiers))
            .chain(events)
            .collect();
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, SIZE)),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                view.pump_queued();
                let commands = view.show(ui, &[], &mut file);
                self.commands.extend(commands);
                for request in view.modals(ui.ctx(), None, Vec::new(), true, false) {
                    view.send(request);
                }
            },
        );
        self.file = file;
        output.textures_delta.clear();
        Shown::of(&output)
    }

    fn settled(&mut self, view: &mut LibraryView) -> Shown {
        for _ in 0..3 {
            self.frame_with(view, Vec::new(), egui::Modifiers::NONE);
        }
        self.frame_with(view, Vec::new(), egui::Modifiers::NONE)
    }

    /// Click the widget `label`, with `modifiers` held.
    fn click_with(&mut self, view: &mut LibraryView, label: &str, modifiers: egui::Modifiers) {
        let at = self.settled(view).button(label).rect.center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        };
        self.time += 1.0;
        self.frame_with(view, vec![egui::Event::PointerMoved(at)], modifiers);
        self.frame_with(view, vec![button(true)], modifiers);
        self.frame_with(view, vec![button(false)], modifiers);
        self.frame_with(
            view,
            vec![egui::Event::PointerMoved(egui::pos2(1.0, 1.0))],
            egui::Modifiers::NONE,
        );
    }

    fn click(&mut self, view: &mut LibraryView, label: &str) {
        self.click_with(view, label, egui::Modifiers::NONE);
    }

    /// Frames until `done` holds, the worker's answers taken meanwhile.
    #[track_caller]
    fn until(
        &mut self,
        view: &mut LibraryView,
        mut done: impl FnMut(&LibraryView) -> bool,
    ) -> Shown {
        let deadline = Instant::now() + crate::test_wait::HANG_GUARD;
        while !done(view) {
            assert!(
                Instant::now() < deadline,
                "the condition was not reached; rows {:?}, batch {:?}",
                view.snapshot
                    .rows
                    .iter()
                    .map(|r| r.image.reference.clone())
                    .collect::<Vec<_>>(),
                view.push_batch
                    .as_ref()
                    .map(|b| (&b.state, b.error.clone(), b.items.len()))
            );
            self.frame_with(view, Vec::new(), egui::Modifiers::NONE);
            std::thread::sleep(Duration::from_millis(2));
        }
        self.settled(view)
    }
}

/// Tag the fixture image as `tag` too: another name for the same image,
/// which a write here makes.
fn tag(lab: &Lab, tag: &str) -> Descriptor {
    pc_image::test_support::with_synthetic_firmware(|| {
        lab.f
            .store
            .update_tag(tag, &lab.f.row.descriptor, None, &pc_image::PcProvider)
    })
    .unwrap();
    lab.f.store.tag_descriptor(tag).unwrap()
}

/// Select `tag` alone in the Images collection.
fn select(lab: &mut Lab, tag: &str) {
    lab.view.query.filter = Filter::All;
    lab.view.remote.node = None;
    lab.view.machine = None;
    lab.view.library_multi.clear();
    lab.view.selected = Some(ImageRow {
        reference: tag.into(),
        aliases: Vec::new(),
        descriptor: lab.f.store.tag_descriptor(tag).unwrap(),
        summary: None,
        failure: None,
    });
}

/// Open Push to… on `tag` and wait for its dry run.
fn push_to(lab: &mut Lab, screen: &mut Screen, local: &str) -> Shown {
    select(lab, local);
    lab.view.begin_push(lab.accounts.clone());
    screen.until(&mut lab.view, |v| {
        v.push
            .as_ref()
            .is_some_and(|d| matches!(d.state, PushState::Ready))
    })
}

/// Preferences listing the lab registry with a public `unemu/` and a
/// private `private/` namespace, with the login `ci` or none.
fn namespaced(lab: &Lab, login: bool) {
    let host = lab.host();
    std::fs::write(
        lab.accounts.path.as_ref().unwrap(),
        format!(
            "[[registries]]\nhost = \"{host}\"\n{}plain_http = true\nnamespaces = [{{ name = \"unemu\", private = false }}, {{ name = \"private\", private = true }}]\n",
            if login { "user = \"ci\"\n" } else { "" }
        ),
    )
    .unwrap();
}

/// Show `host` in Remotes as listed in this session with `repositories`,
/// each tag naming `digest`.
fn listed(lab: &mut Lab, repositories: &[(&str, &str, Digest)]) {
    let host = lab.host();
    lab.view.attach_remotes(lab.accounts.clone());
    let model = lab.view.remote.model.as_mut().unwrap();
    model.touch();
    let view = model
        .registries
        .iter_mut()
        .find(|v| v.host == host)
        .unwrap();
    view.state = State::Listed;
    view.repositories = repositories
        .iter()
        .map(|(name, tag, digest)| {
            let mut record =
                super::remotes::test_support::record(image_store::media_type::MACHINE, name);
            record.digest = digest.clone();
            RepositoryRow {
                name: (*name).into(),
                tags: vec![(*tag).into()],
                representative: Some((*tag).into()),
                read: vec![TagRow {
                    tag: (*tag).into(),
                    local_tag: local_tag_of(&host, name, tag),
                    record: Ok(record),
                    local: Some(LocalState::NotPulled { same_as: None }),
                    stored: Default::default(),
                    newest_revision: None,
                }],
                error: None,
                newer_needs: Vec::new(),
            }
        })
        .collect();
}

/// The Remotes rule preselects: a tag that names its registry goes there,
/// a short tag pushed before goes where it went last, a short tag made
/// here waits for a choice in the chooser; the destination text and the
/// chooser show the same destination, and the local tag stays.
#[test]
fn push_to_preselects_by_the_remotes_rule_and_the_chooser_follows() {
    let mut lab = Lab::new(true);
    let host = lab.host();
    let mut screen = Screen::new();
    let own = format!("{host}/private/test/at-test-rig:0.3.0");
    tag(&lab, &own);
    let shown = push_to(&mut lab, &mut screen, &own);
    assert!(shows_all(
        &shown,
        &[
            &format!("Push to… — {own}"),
            &format!("The local tag names {host}, so its own registry is preselected."),
            "Pushing never renames it.",
            "Ready to push.",
            "Dry run — every blob in the image, with its redistribution mark",
        ]
    ));
    assert_eq!(lab.view.push.as_ref().unwrap().destination, own);
    assert!(shown.button("Push").enabled);

    // Short, pushed before: where it went last.
    let pushed = "private/test/pushed:0.2.0";
    let image = tag(&lab, pushed);
    let last = format!("{host}/private/test/pushed:0.2.0");
    lab.f
        .store
        .record_push(pushed, &image.digest, "registry.lan.example/x/y:1", 1)
        .unwrap();
    lab.f
        .store
        .record_push(pushed, &image.digest, &last, 2)
        .unwrap();
    let shown = push_to(&mut lab, &mut screen, pushed);
    assert!(shown.shows("Preselected: where this tag was pushed last."));
    assert_eq!(lab.view.push.as_ref().unwrap().destination, last);

    // Short, made here, never pushed: nothing until a choice.
    let bench = "private/test/dos5-bench:0.2.0";
    tag(&lab, bench);
    let shown = push_to(&mut lab, &mut screen, bench);
    assert!(shows_all(
        &shown,
        &[
            "A short local tag names no registry, so nothing is preselected.",
            "Choose a registry and a namespace: a short local tag names no registry.",
            "— no registry chosen",
            "The checks appear once a destination is chosen.",
        ]
    ));
    assert!(lab.view.push.as_ref().unwrap().destination.is_empty());
    assert!(!shown.button("Push").enabled);
    // The chooser picks the registry; the item keeps its whole path and tag.
    screen.click(&mut lab.view, &format!("{host} private, whole registry"));
    let shown = screen.settled(&mut lab.view);
    let wanted = format!("{host}/private/test/dos5-bench:0.2.0");
    assert_eq!(lab.view.push.as_ref().unwrap().destination, wanted);
    assert!(shown.shows(&wanted) && shown.shows("Ready to push."));
    assert_eq!(
        shown.field(Role::TextInput, "Repository"),
        "private/test/dos5-bench"
    );
    assert_eq!(shown.field(Role::TextInput, "Tag"), "0.2.0");
    assert!(shown.button("Push").enabled);
    // Nothing was sent while the dialog worked it out.
    assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());
}

fn shows_all(shown: &Shown, wanted: &[&str]) -> bool {
    for text in wanted {
        assert!(shown.shows(text), "{text:?} not shown in {:?}", shown.texts);
    }
    true
}

/// The guard refuses user-supplied content towards unemu/ with no
/// override and Push disabled; a private namespace wants a login first,
/// with Log in… beside the failed check, and after the login the same
/// destination is ready.
#[test]
fn the_guard_refuses_without_override_and_a_private_namespace_waits_for_the_login() {
    let mut lab = Lab::new(true);
    let host = lab.host();
    namespaced(&lab, true);
    let mut screen = Screen::new();
    let bench = "private/test/dos5-bench:0.2.0";
    tag(&lab, bench);
    push_to(&mut lab, &mut screen, bench);
    screen.click(
        &mut lab.view,
        &format!("unemu/ on {host} open content only"),
    );
    let shown = screen.settled(&mut lab.view);
    assert_eq!(
        lab.view.push.as_ref().unwrap().destination,
        format!("{host}/unemu/private/test/dos5-bench:0.2.0")
    );
    assert!(shows_all(
        &shown,
        &[
            "Push refused.",
            "This image contains user-supplied firmware: push it only to a private registry, never to unemu/.",
            &format!("There is no override for this rule. Choose a private namespace, such as {host}/private/. Nothing was sent."),
            "Push is disabled: the guard refused this destination.",
        ]
    ));
    assert!(!shown.button("Push").enabled);

    // Before the login.
    namespaced(&lab, false);
    let own = format!("{host}/private/test/boot-smoke:nightly");
    tag(&lab, &own);
    let shown = push_to(&mut lab, &mut screen, &own);
    assert!(shows_all(
        &shown,
        &[
            &format!("Not signed in to {host}: a private namespace needs a login before a push, as it does before a listing or a pull"),
            &format!("Log in to {host} to continue. Nothing is sent before the login."),
        ]
    ));
    assert!(!shown.button("Push").enabled);
    screen.click(&mut lab.view, "Log in…");
    assert_eq!(lab.view.login_wanted.as_deref(), Some(host.as_str()));
    // The login is made in Preferences; once they close, the verdict follows.
    lab.view.login_wanted = None;
    namespaced(&lab, true);
    lab.view.preferences_closed();
    let shown = screen.settled(&mut lab.view);
    assert!(
        shown.shows("Ready to push.")
            && shown.shows("Login ci; its secret comes from the credential store")
    );
    assert!(shown.button("Push").enabled);
    assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());
}

/// As this session lists the registry: a version tag that names another
/// image is refused with the next free tag suggested; a tag that is not a
/// version moves only after the move is confirmed with both digests, and
/// nothing is sent before that.
#[test]
fn as_listed_a_version_tag_taken_is_refused_and_a_moving_tag_asks_first() {
    let mut lab = Lab::new(true);
    let host = lab.host();
    let mut screen = Screen::new();
    let other = Digest::of(b"another image");
    listed(
        &mut lab,
        &[
            ("private/test/at-test-rig", "0.3.0", other.clone()),
            ("private/test/boot-smoke", "nightly", other.clone()),
        ],
    );
    let rig = format!("{host}/private/test/at-test-rig:0.3.0");
    tag(&lab, &rig);
    let shown = push_to(&mut lab, &mut screen, &rig);
    assert!(shows_all(
        &shown,
        &[
            "As listed, 0.3.0 names another image on the registry (sha256:",
            "0.3.0 names another image on the registry. Version tags never move: choose another tag, such as 0.3.1.",
            "The registry’s image stays as it is. Nothing was sent.",
            "Push is disabled: this tag cannot move.",
        ]
    ));
    assert!(!shown.button("Push").enabled);

    let smoke = format!("{host}/private/test/boot-smoke:nightly");
    let image = tag(&lab, &smoke);
    let shown = push_to(&mut lab, &mut screen, &smoke);
    assert!(shows_all(
        &shown,
        &[
            "pushing moves the tag, after you confirm the move with both digests",
            "Ready to push, after you confirm the move.",
            "Push asks before it moves nightly.",
        ]
    ));
    screen.click(&mut lab.view, "Push");
    let shown = screen.settled(&mut lab.view);
    assert!(shows_all(
        &shown,
        &[
            "Move the tag to this image?",
            &other.to_string(),
            &image.digest.to_string(),
            "Move tag",
        ]
    ));
    assert!(matches!(
        lab.view.push.as_ref().unwrap().state,
        PushState::ConfirmMove { .. }
    ));
    assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());
}

/// A namespace's pane shows until a tag is selected, with what of the
/// library could go there and Push here…, which lists the library with a
/// verdict for this place per item; those that cannot go here hide behind
/// a check box that says how many they are.
#[test]
fn push_here_lists_the_library_with_a_verdict_per_item() {
    let mut lab = Lab::new(true);
    let host = lab.host();
    namespaced(&lab, true);
    let mut screen = Screen::new();
    tag(&lab, "private/test/dos5-bench:0.2.0");
    listed(&mut lab, &[]);
    let node = |namespace: &str| Node {
        host: host.clone(),
        namespace: Some(Some(namespace.into())),
        group: None,
    };
    lab.view.remote.node = Some(node("unemu"));
    let shown = screen.until(&mut lab.view, |v| v.push_plans.is_some());
    assert!(shows_all(
        &shown,
        &[
            &format!("{host}/unemu/"),
            "NAMESPACE",
            "Takes only images whose every blob is open, with their source archives and terms.",
            "0 of 2 items can go here. Each keeps its local tag.",
        ]
    ));
    screen.click(&mut lab.view, "Push here…");
    let shown = screen.until(&mut lab.view, |v| {
        v.push_batch
            .as_ref()
            .is_some_and(|b| b.state == library_model::push_batch::BatchState::Ready)
    });
    assert!(shows_all(
        &shown,
        &[
            &format!("Push here — {host}/unemu/"),
            "Nothing in the library can go here.",
            "and the 2 items that cannot go here, with the reason",
            "Select the items to push. Nothing has been sent.",
        ]
    ));
    screen.click(
        &mut lab.view,
        "and the 2 items that cannot go here, with the reason",
    );
    let shown = screen.settled(&mut lab.view);
    assert!(shown.shows("Refused") && shown.shows("Cannot go here: "));
    assert!(!shown.button("Push 0 items").enabled);
    lab.view.push_batch = None;

    lab.view.remote.node = Some(node("private"));
    let shown = screen.settled(&mut lab.view);
    assert!(shown.shows("2 of 2 items can go here. Each keeps its local tag."));
    screen.click(&mut lab.view, "Push here…");
    let shown = screen.until(&mut lab.view, |v| {
        v.push_batch
            .as_ref()
            .is_some_and(|b| b.state == library_model::push_batch::BatchState::Ready)
    });
    assert!(shows_all(
        &shown,
        &[
            &format!("Push here — {host}/private/"),
            "Can go here",
            "Each selected item goes to",
        ]
    ));
    // Ctrl selects several.
    screen.click(&mut lab.view, "private/test/dos5-bench:0.2.0");
    screen.click_with(&mut lab.view, "test:source", egui::Modifiers::COMMAND);
    let shown = screen.settled(&mut lab.view);
    assert_eq!(lab.view.push_batch.as_ref().unwrap().picked.len(), 2);
    assert!(shown.button("Push 2 items").enabled);
    assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());
}

/// Several items go one after another under one progress bar: a refused
/// one is skipped with its reason, a failure on the way marks its row and
/// the next one goes; the end line counts them. After the push the
/// Remotes row of a pushed tag reads Pushed from here, as the local tag,
/// and the library's Source names the registry, neither after a refresh;
/// the local tags are as they were.
#[test]
fn several_items_push_in_order_skip_and_fail_by_row_and_show_where_they_went() {
    let mut lab = Lab::new(true);
    let host = lab.host();
    let mut screen = Screen::new();
    let first = "private/test/first:1.0.0";
    let refused = "other.example/private/test/elsewhere:1.0.0";
    let failing = "private/fail/broken:1.0.0";
    let last = "private/test/last:1.0.0";
    for local in [first, refused, failing, last] {
        tag(&lab, local);
    }
    listed(&mut lab, &[]);
    lab.registry
        .fail_requests("PUT /v2/private/fail/broken/manifests/", 500);
    lab.view.query.filter = Filter::All;
    lab.view
        .send(crate::library_view::Request::Query(lab.view.query.clone()));
    screen.until(&mut lab.view, |v| v.snapshot.rows.len() >= 5);
    // Ctrl (⌘ on macOS) adds each row to the selection.
    let shown = screen.settled(&mut lab.view);
    let _ = shown;
    for (index, local) in [first, refused, failing, last].into_iter().enumerate() {
        let modifiers = if index == 0 {
            egui::Modifiers::NONE
        } else {
            egui::Modifiers::COMMAND
        };
        let label = row_label(&lab.view, local);
        screen.click_with(&mut lab.view, &label, modifiers);
    }
    let shown = screen.settled(&mut lab.view);
    assert_eq!(
        lab.view.library_multi.len(),
        4,
        "{:?}",
        lab.view.library_multi
    );
    assert!(shows_all(&shown, &["4 items selected", "Push 4 items to…"]));
    screen.commands.clear();
    screen.click(&mut lab.view, "Push 4 items to…");
    // The pane asks the shell for Push to…, which opens it.
    assert_eq!(screen.commands, [crate::commands::Command::Push]);
    lab.view.begin_push(lab.accounts.clone());
    let shown = screen.until(&mut lab.view, |v| {
        v.push_batch
            .as_ref()
            .is_some_and(|b| b.state == library_model::push_batch::BatchState::Ready)
    });
    assert!(shows_all(
        &shown,
        &[
            "Push 4 items to…",
            "For the 3 short tags",
            "Pick a registry and a namespace on the left",
            "Skipped: other.example is not in your registry list. Add it in Tools › Preferences › Registries first.",
        ]
    ));
    screen.click(&mut lab.view, &format!("{host} private, whole registry"));
    let shown = screen.settled(&mut lab.view);
    assert!(shown.shows("1 will be skipped. Nothing has been sent."));
    screen.click(&mut lab.view, "Push 3 items");
    let shown = screen.until(&mut lab.view, |v| {
        v.push_batch.as_ref().is_some_and(|b| b.finished())
    });
    let batch = lab.view.push_batch.as_ref().unwrap();
    let stages: Vec<&str> = batch
        .queue
        .iter()
        .map(|&i| batch.stage(i).label())
        .collect();
    assert_eq!(stages, ["Pushed ✓", "Skipped", "Failed ✗", "Pushed ✓"]);
    assert!(shows_all(
        &shown,
        &[
            "Done — 2 of 3 pushed · 1 skipped · 1 failed",
            "Pushed 2 of 4 items.",
            "1 skipped, 1 failed; each row says why.",
            "Failed: ",
            "Push to… on a failed item again resumes its accepted chunks.",
            &format!("Source in the library names {host}; neither needed a refresh."),
            "Local tags are as they were: pushing never renames.",
        ]
    ));
    let image = lab.f.row.descriptor.digest.clone();
    assert_eq!(
        lab.registry.tag_digest("private/test/first", "1.0.0"),
        Some(image.clone())
    );
    assert_eq!(
        lab.registry.tag_digest("private/test/last", "1.0.0"),
        Some(image.clone())
    );
    assert_eq!(
        lab.registry.tag_digest("private/fail/broken", "1.0.0"),
        None
    );

    // Remotes shows the pushed tags at once, without a request.
    lab.registry.clear_log();
    screen.click(&mut lab.view, "Show in Remotes");
    let shown = screen.settled(&mut lab.view);
    assert!(
        shown.shows("Pushed from here · as private/test/first:1.0.0"),
        "{:?}",
        shown.texts
    );
    assert!(lab.registry.log().is_empty(), "{:?}", lab.registry.log());
    // The library's Source reads local → the registry; the local tags stay.
    lab.view.remote.node = None;
    lab.view.query.filter = Filter::All;
    lab.view.library_multi.clear();
    let shown = screen.until(&mut lab.view, |v| {
        v.snapshot.rows.iter().any(|r| {
            library_model::listing::source_cell(&r.image.descriptor) == format!("local → {host}")
        })
    });
    assert!(shown.shows(&format!("local → {host}")), "{:?}", shown.texts);
    for local in [first, last] {
        assert_eq!(lab.f.store.tag_descriptor(local).unwrap().digest, image);
        let named = format!("{host}/{local}");
        assert!(lab.f.store.tag_descriptor(&named).is_err(), "{named}");
    }
    let pushed = lab
        .f
        .store
        .list_tags(None, 100)
        .unwrap()
        .tags
        .into_iter()
        .find(|t| t.name == first)
        .unwrap()
        .pushed_to();
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].reference, format!("{host}/{first}"));
    let _ = TransferRequest::PlanAll { id: 0 };
}

/// The label of the Images row of `tag`: the name cell, which is the tag
/// until the summary names the image.
fn row_label(view: &LibraryView, tag: &str) -> String {
    view.snapshot
        .rows
        .iter()
        .find(|r| r.image.reference == tag)
        .map(|r| r.image.references())
        .unwrap_or_else(|| tag.into())
}

/// The dry runs of the whole library read every local manifest's marks
/// once for all images, not once per image: a namespace's pane asks for
/// them merely by being shown.
#[test]
fn one_dry_run_of_the_library_reads_the_marks_once() {
    let lab = Lab::new(true);
    let images = 6;
    for index in 1..images {
        tag(&lab, &format!("private/test/copy-{index}:1.0.0"));
    }
    let digest = lab.f.row.descriptor.digest.clone();
    let before = image_store::small_reads(&digest);
    let events = std::sync::Mutex::new(Vec::new());
    pc_image::test_support::with_synthetic_firmware(|| {
        let providers: [&dyn image_store::ConfigValidator; 1] = [&pc_image::PcProvider];
        let pc = crate::pc_desktop::PcDesktop::new(&providers, &[]);
        library_model::registry_transfer::run(
            &lab.f.store,
            TransferRequest::PlanAll { id: 1 },
            &[&pc],
            &|event| events.lock().unwrap().push(event),
            &|| {},
        );
    });
    let reads = image_store::small_reads(&digest) - before;
    // One scan of the six tags, then each image's own two reads; a scan per
    // image would read the manifest 6 × (6 + 2) = 48 times.
    assert!(
        (images..=3 * images).contains(&reads),
        "{reads} reads of the manifest"
    );
    let events = events.into_inner().unwrap();
    let [crate::registry_transfer::TransferEvent::PlannedAll {
        result: Ok(planned),
        ..
    }] = events.as_slice()
    else {
        panic!("one answer with the dry runs")
    };
    assert_eq!(planned.len(), images);
    assert!(planned.iter().all(|(_, plan)| plan.is_ok()));
}

/// A namespace's pane asks for the library's dry runs once; a failed
/// answer stands until the library's tags change, and is not asked again
/// every frame.
#[test]
fn a_failed_dry_run_of_the_library_is_not_asked_again_every_frame() {
    use super::remotes::test_support::{Rig, LAN};
    let mut rig = Rig::new();
    rig.view.snapshot = std::sync::Arc::new(super::Snapshot {
        read: true,
        ..Default::default()
    });
    rig.view.remote.node = Some(Node {
        host: LAN.into(),
        namespace: Some(Some("private".into())),
        group: None,
    });
    let mut screen = Screen::new();
    let plans = |rig: &Rig| -> Vec<u64> {
        rig.sent()
            .into_iter()
            .filter_map(|request| match request {
                crate::library_view::Request::Transfer(TransferRequest::PlanAll { id }) => Some(id),
                _ => None,
            })
            .collect()
    };
    screen.settled(&mut rig.view);
    let asked = plans(&rig);
    assert_eq!(asked.len(), 1, "one request for the dry runs");
    rig.view.on_event(crate::library_worker::Event::Transfer(
        crate::registry_transfer::TransferEvent::PlannedAll {
            id: asked[0],
            result: Err("The library could not be read".into()),
        },
    ));
    for _ in 0..5 {
        screen.settled(&mut rig.view);
    }
    assert!(plans(&rig).is_empty(), "a failure is not asked again");
}

/// While several rows are selected, the row selected alone before stays
/// out of every command of one item: Start, Delete tag…, Settings… and
/// Layers… take nothing, and a double click selects its row alone first.
#[test]
fn several_selected_rows_leave_every_command_of_one_item_out() {
    let mut lab = Lab::new(true);
    let mut screen = Screen::new();
    let (a, b, c) = (
        "private/test/a:1.0.0",
        "private/test/b:1.0.0",
        "private/test/c:1.0.0",
    );
    for local in [a, b, c] {
        tag(&lab, local);
    }
    lab.view.query.filter = Filter::All;
    lab.view
        .send(crate::library_view::Request::Query(lab.view.query.clone()));
    screen.until(&mut lab.view, |v| v.snapshot.rows.len() >= 4);
    for (local, modifiers) in [
        (a, egui::Modifiers::NONE),
        (b, egui::Modifiers::COMMAND),
        (c, egui::Modifiers::COMMAND),
        (a, egui::Modifiers::COMMAND),
    ] {
        let label = row_label(&lab.view, local);
        screen.click_with(&mut lab.view, &label, modifiers);
    }
    let shown = screen.settled(&mut lab.view);
    assert_eq!(lab.view.library_multi, [b, c]);
    assert!(shows_all(&shown, &["2 items selected", "Push 2 items to…"]));
    assert!(lab.view.several_selected());
    // The row clicked first is still the single selection behind them, and
    // no command of one item reaches it.
    lab.view
        .launch(crate::library_worker::WorkspaceKind::Machine);
    lab.view.begin_delete();
    lab.view.open_settings();
    lab.view.open_layers();
    lab.view.begin_new_from_image();
    assert!(lab.view.delete.is_none() && lab.view.layers.is_none());
    assert!(lab.view.new_machine.is_none() && !lab.view.busy);
    // A double click selects its row alone before anything acts on it.
    let label = row_label(&lab.view, c);
    // The list's cell, above the pane, which lists the selected tags too.
    let at = screen
        .settled(&mut lab.view)
        .widgets
        .iter()
        .filter(|w| w.label == label)
        .min_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
        .unwrap()
        .rect
        .center();
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    screen.commands.clear();
    screen.frame_with(
        &mut lab.view,
        vec![egui::Event::PointerMoved(at)],
        egui::Modifiers::NONE,
    );
    for pressed in [true, false, true, false] {
        screen.frame_with(&mut lab.view, vec![button(pressed)], egui::Modifiers::NONE);
    }
    assert!(lab.view.library_multi.is_empty());
    assert_eq!(
        lab.view.selected.as_ref().map(|r| r.reference.as_str()),
        Some(c)
    );
}
