// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Remotes group drawn headless, frame by frame, with a worker that
//! records every request instead of contacting a registry; and the Source
//! column of the library lists.

use super::*;
use crate::{
    library_view::{Filter, Snapshot},
    library_worker::{Event, MachineJob, Worker, WorkspaceKind},
    remote_browse::{local_tag_of, representative, Job},
};
use egui::accesskit::Role;
use image_registry::{
    client::Challenge, credential_store::SessionStore, credentials::EnvCredentials,
};
use image_store::Digest;
use std::sync::{atomic::Ordering, mpsc, Arc};

const LAN: &str = "registry.lan.example";
const NAS: &str = "nas.example:5000";
/// Wide enough that the list's columns fit without scrolling sideways.
const WIDE: egui::Vec2 = egui::vec2(1400.0, 900.0);
const LOOKS: [(bool, bool); 4] = [(true, false), (true, true), (false, false), (false, true)];

/// Preferences that list `registry.lan.example` with a private namespace
/// and no account, and `nas.example:5000`, a whole private registry with
/// an account; the family default registry is listed without an entry.
fn preferences() -> (assembly::test_support::TempDir, Accounts) {
    let temp = assembly::test_support::TempDir::new("remotes-view");
    let path = temp.path().join("preferences.toml");
    std::fs::write(
        &path,
        format!(
            "[[registries]]\nhost = \"{LAN}\"\nnamespaces = [{{ name = \"private\", private = true }}]\n\
             [[registries]]\nhost = \"{NAS}\"\nprivate = true\nuser = \"maintainer\"\n"
        ),
    )
    .unwrap();
    let accounts = Accounts {
        path: Some(path),
        store: Arc::new(SessionStore::default()),
        env: EnvCredentials::default(),
        default_override: None,
    };
    (temp, accounts)
}

/// A library view whose worker records the requests it gets.
struct Rig {
    view: LibraryView,
    requests: mpsc::Receiver<Request>,
    /// Dropping it is the worker going away.
    events: Option<mpsc::SyncSender<Event>>,
    _preferences: assembly::test_support::TempDir,
}

impl Rig {
    fn new() -> Self {
        let (temp, accounts) = preferences();
        let mut view = LibraryView::new(None);
        let (worker, requests, events) = Worker::recording();
        view.worker = Some(worker);
        view.attach_remotes(accounts);
        Self {
            view,
            requests,
            events: Some(events),
            _preferences: temp,
        }
    }

    /// Every request sent since the last call.
    fn sent(&self) -> Vec<Request> {
        self.requests.try_iter().collect()
    }

    /// The browsing requests sent since the last call.
    fn browsing(&self) -> Vec<BrowseRequest> {
        self.sent()
            .into_iter()
            .filter_map(|request| match request {
                Request::Browse(browse) => Some(browse),
                _ => None,
            })
            .collect()
    }

    /// The model, for a change the test makes to it directly.
    fn model(&mut self) -> &mut Remotes {
        let model = self.view.remote.model.as_mut().expect("remotes attached");
        model.touch();
        model
    }

    fn registry(&mut self, host: &str) -> &mut RegistryView {
        self.model()
            .registries
            .iter_mut()
            .find(|v| v.host == host)
            .unwrap_or_else(|| panic!("no registry {host}"))
    }

    /// `registry.unemu.dev` with the sample listing, listed.
    fn listed(&mut self) {
        let registry = self.registry(DEFAULT_REGISTRY);
        registry.repositories = sample(DEFAULT_REGISTRY);
        registry.state = State::Listed;
    }

    fn show(&mut self, node: Node) {
        self.view.remote.node = Some(node);
    }
}

fn job_of(request: &BrowseRequest) -> &Job {
    match request {
        BrowseRequest::List { job, .. } | BrowseRequest::Read { job, .. } => job,
        BrowseRequest::Local { .. } => panic!("a local-state request has no job"),
    }
}

fn record(artifact_type: &str, title: &str) -> TagRecord {
    TagRecord {
        digest: Digest::of(title.as_bytes()),
        artifact_type: artifact_type.into(),
        title: Some(title.into()),
        version: None,
        formats: Vec::new(),
        min_app_version: None,
        platform: Some(pc_image::PLATFORM.into()),
        size: 3 * 1024 * 1024 + 512 * 1024,
        user_supplied: false,
        annotations: Default::default(),
        details: None,
        manifest_size: 1024,
        descriptors: Vec::new(),
        media: Vec::new(),
    }
}

fn read(host: &str, repository: &str, tag: &str, record: TagRecord, local: LocalState) -> TagRow {
    TagRow {
        tag: tag.into(),
        local_tag: local_tag_of(host, repository, tag),
        record: Ok(record),
        local: Some(local),
        stored: Default::default(),
        newest_revision: None,
    }
}

fn repository(name: &str, tags: &[&str], read: Vec<TagRow>) -> RepositoryRow {
    let tags: Vec<String> = tags.iter().map(|t| (*t).to_owned()).collect();
    RepositoryRow {
        name: name.into(),
        representative: representative(&tags),
        tags,
        read,
        error: None,
        newer_needs: Vec::new(),
    }
}

/// The sample listing: three machine images (one with three tags), two
/// installation media sets and a firmware package this release cannot
/// run, in every local state.
fn sample(host: &str) -> Vec<RepositoryRow> {
    let mut future = record(mt::FIRMWARE, "Future BIOS");
    future.min_app_version = Some("99.0.0".into());
    let mut freedos = record(mt::MEDIA_SET, "FreeDOS");
    freedos.formats = vec!["1.44M".into()];
    freedos.media = vec![
        ("disk-1".into(), "1.44M".into()),
        ("disk-2".into(), "1.44M".into()),
    ];
    let mut keen = record(mt::MEDIA_SET, "Commander Keen 4");
    keen.user_supplied = true;
    vec![
        repository(
            "unemu/open-at",
            &["1.0.0", "latest", "1.2.0"],
            vec![read(
                host,
                "unemu/open-at",
                "1.2.0",
                record(mt::MACHINE, "Open AT 286"),
                LocalState::InLibrary,
            )],
        ),
        repository(
            "unemu/open-at-games",
            &["1.2.0"],
            vec![read(
                host,
                "unemu/open-at-games",
                "1.2.0",
                record(mt::MACHINE, "Open AT 286 for games"),
                LocalState::NewerAvailable {
                    have: "open-at-games:1.0.0".into(),
                },
            )],
        ),
        repository(
            "unemu/clash",
            &["1.0.0"],
            vec![read(
                host,
                "unemu/clash",
                "1.0.0",
                record(mt::MACHINE, "Clash"),
                LocalState::OtherImage {
                    current: Digest::of(b"another"),
                },
            )],
        ),
        repository(
            "unemu/media/freedos",
            &["1.4-720k", "1.4"],
            vec![read(
                host,
                "unemu/media/freedos",
                "1.4",
                freedos,
                LocalState::NotPulled { same_as: None },
            )],
        ),
        repository(
            "unemu/media/keen4",
            &["1.4"],
            vec![read(
                host,
                "unemu/media/keen4",
                "1.4",
                keen,
                LocalState::NotPulled {
                    same_as: Some("keen4:1.4".into()),
                },
            )],
        ),
        repository(
            "unemu/firmware/future-bios",
            &["1"],
            vec![read(
                host,
                "unemu/firmware/future-bios",
                "1",
                future,
                LocalState::NotPulled { same_as: None },
            )],
        ),
    ]
}

fn unemu(group: Option<Group>) -> Node {
    Node {
        host: DEFAULT_REGISTRY.into(),
        namespace: Some(Some(OFFICIAL_NAMESPACE.into())),
        group,
    }
}

/// A text as it was painted: what, where, the clip and its first colour.
#[derive(Clone, Debug, PartialEq)]
struct Text {
    text: String,
    rect: Rect,
    clip: Rect,
    colour: Color32,
}

/// A widget as egui reported it to AccessKit.
#[derive(Clone, Debug, PartialEq)]
struct Widget {
    role: Role,
    label: String,
    rect: Rect,
}

/// What a frame painted: texts, AccessKit widgets and stroke colours.
struct Shown {
    texts: Vec<Text>,
    widgets: Vec<Widget>,
    strokes: Vec<(Rect, Color32)>,
}

impl Shown {
    fn of(output: &egui::FullOutput) -> Self {
        fn walk(shape: &egui::Shape, clip: Rect, shown: &mut Shown) {
            match shape {
                egui::Shape::Text(text) => {
                    if text.galley.text().trim().is_empty() {
                        return;
                    }
                    let rect = text
                        .galley
                        .rows
                        .iter()
                        .filter(|row| !row.glyphs.is_empty())
                        .map(|row| row.rect_without_leading_space())
                        .reduce(|a, b| a.union(b))
                        .unwrap_or(text.galley.rect)
                        .translate(text.pos.to_vec2());
                    let colour = text
                        .galley
                        .job
                        .sections
                        .first()
                        .map_or(text.fallback_color, |s| s.format.color);
                    shown.texts.push(Text {
                        text: text.galley.text().to_owned(),
                        rect,
                        clip,
                        colour: if colour == Color32::PLACEHOLDER {
                            text.fallback_color
                        } else {
                            colour
                        },
                    });
                }
                egui::Shape::LineSegment { points, stroke } => shown
                    .strokes
                    .push((Rect::from_two_pos(points[0], points[1]), stroke.color)),
                egui::Shape::Path(path) => {
                    if let egui::epaint::ColorMode::Solid(colour) = path.stroke.color {
                        shown
                            .strokes
                            .push((Rect::from_points(&path.points), colour));
                    }
                }
                egui::Shape::Rect(rect) => shown.strokes.push((rect.rect, rect.stroke.color)),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, clip, shown)),
                _ => (),
            }
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
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut shown = Self {
            texts: Vec::new(),
            widgets,
            strokes: Vec::new(),
        };
        for clipped in &output.shapes {
            walk(&clipped.shape, clipped.clip_rect, &mut shown);
        }
        shown
    }

    fn has(&self, text: &str) -> bool {
        self.texts.iter().any(|t| t.text == text)
    }

    fn text(&self, text: &str) -> &Text {
        self.texts
            .iter()
            .find(|t| t.text == text)
            .unwrap_or_else(|| {
                let all: Vec<_> = self.texts.iter().map(|t| &t.text).collect();
                panic!("{text:?} not painted in {all:?}")
            })
    }

    /// The widget labelled `label`, or whose label starts with it and a space.
    fn widget(&self, label: &str) -> &Widget {
        self.widgets
            .iter()
            .find(|w| w.label == label)
            .or_else(|| {
                self.widgets
                    .iter()
                    .find(|w| w.label.starts_with(&format!("{label} ")))
            })
            .unwrap_or_else(|| {
                let labels: Vec<_> = self.widgets.iter().map(|w| &w.label).collect();
                panic!("no widget {label:?} in {labels:?}")
            })
    }

    fn has_widget(&self, label: &str) -> bool {
        self.widgets.iter().any(|w| w.label == label)
    }

    /// The text is painted, or a widget says it: a label the details pane
    /// scrolled out of view says it without painting it.
    fn shows(&self, text: &str) -> bool {
        self.has(text) || self.has_widget(text)
    }

    /// The list's cells, row by row from the top, each row's labels left to
    /// right: the labels of the list's cells under the header.
    fn rows(&self) -> Vec<Vec<String>> {
        let header = self.widget("Name").rect;
        let mut cells: Vec<&Widget> = self
            .widgets
            .iter()
            .filter(|w| {
                w.role == Role::Label
                    && w.rect.top() >= header.bottom() - 0.5
                    && w.rect.left() >= header.left() - 0.5
                    && (w.rect.height() - ROW).abs() < 0.5
            })
            .collect();
        cells.sort_by(|a, b| {
            a.rect
                .top()
                .total_cmp(&b.rect.top())
                .then(a.rect.left().total_cmp(&b.rect.left()))
        });
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut top = f32::NAN;
        for cell in cells {
            if cell.rect.top() != top {
                rows.push(Vec::new());
                top = cell.rect.top();
            }
            rows.last_mut().unwrap().push(cell.label.clone());
        }
        rows
    }
}

/// One context in one skin and theme, driven frame by frame.
struct Screen {
    ctx: egui::Context,
    size: egui::Vec2,
    time: f64,
    file: Option<usize>,
}

impl Screen {
    fn new(classic: bool, dark: bool, size: egui::Vec2) -> Self {
        let ctx = egui::Context::default();
        crate::shell::configure_context(&ctx);
        ctx.set_visuals(crate::shell::skin_visuals(classic, dark));
        ctx.enable_accesskit();
        Self {
            ctx,
            size,
            time: 0.0,
            file: None,
        }
    }

    fn frame(&mut self, view: &mut LibraryView, events: Vec<egui::Event>) -> Shown {
        self.time += 1.0 / 60.0;
        let mut file = self.file;
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, self.size)),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                view.show(ui, &[], &mut file);
            },
        );
        self.file = file;
        output.textures_delta.clear();
        Shown::of(&output)
    }

    /// The view once its layout has settled.
    fn settled(&mut self, view: &mut LibraryView) -> Shown {
        for _ in 0..3 {
            self.frame(view, Vec::new());
        }
        self.frame(view, Vec::new())
    }

    /// Click the widget labelled `label` as the settled view shows it, then
    /// move the pointer away.
    fn click(&mut self, view: &mut LibraryView, label: &str) {
        let at = self.settled(view).widget(label).rect.center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.time += 1.0;
        self.frame(view, vec![egui::Event::PointerMoved(at)]);
        self.frame(view, vec![button(true)]);
        self.frame(view, vec![button(false)]);
        self.frame(view, vec![egui::Event::PointerMoved(egui::pos2(1.0, 1.0))]);
    }

    /// Press and release `key` with no modifiers.
    fn key(&mut self, view: &mut LibraryView, key: egui::Key) {
        let event = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        self.frame(view, vec![event(true)]);
        self.frame(view, vec![event(false)]);
    }
}

/// The tree shows every root, the default registry first, with its kind;
/// nothing is contacted until a registry's node opens. An open registry
/// shows its namespaces, a private one with a lock while no login lists it,
/// and an open namespace one group per artifact type with repositories,
/// each with its count. Opening a registry's node sends one listing
/// request, and only the first time.
#[test]
fn the_tree_lists_registries_namespaces_locks_and_type_groups_and_opening_lists() {
    let mut rig = Rig::new();
    let mut screen = Screen::new(true, false, WIDE);
    let shown = screen.settled(&mut rig.view);
    for (host, mark) in [
        (DEFAULT_REGISTRY, "official, public"),
        (LAN, "private"),
        (NAS, "private"),
    ] {
        assert!(shown.has_widget(&format!("{host} {mark}")), "{host}");
        assert!(shown.has(host) && shown.has(mark), "{host}");
    }
    let order: Vec<f32> = [DEFAULT_REGISTRY, LAN, NAS]
        .map(|host| shown.text(host).rect.top())
        .to_vec();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    assert!(shown.text("Remotes").rect.top() < order[0]);
    assert!(
        rig.sent().is_empty(),
        "nothing is contacted before a node opens"
    );

    // Opening a registry lists it; its entry names its namespaces at once.
    screen.click(&mut rig.view, &format!("Expand {LAN}"));
    let requests = rig.browsing();
    assert_eq!(requests.len(), 1);
    assert!(matches!(
        &requests[0],
        BrowseRequest::List { next: None, .. }
    ));
    assert_eq!(job_of(&requests[0]).host, LAN);
    let shown = screen.settled(&mut rig.view);
    let private = shown
        .widget("private/ on registry.lan.example locked until you log in")
        .rect;
    let warn = crate::shell::skin_visuals(true, false).warn_fg_color;
    assert!(
        shown
            .strokes
            .iter()
            .any(|(rect, colour)| private.contains_rect(*rect) && *colour == warn),
        "the private namespace shows no lock"
    );
    // Opening it again, or closing it, contacts nothing.
    screen.click(&mut rig.view, &format!("Collapse {LAN}"));
    screen.click(&mut rig.view, &format!("Expand {LAN}"));
    assert!(rig.browsing().is_empty());

    // A listing fills the default registry's namespace and its groups.
    rig.listed();
    rig.view.remote.open.insert(DEFAULT_REGISTRY.into());
    screen.click(&mut rig.view, "Expand unemu/");
    let shown = screen.settled(&mut rig.view);
    for label in [
        "unemu/ on registry.unemu.dev (6)",
        "Machine images in unemu/ on registry.unemu.dev (3)",
        "Installation media in unemu/ on registry.unemu.dev (2)",
        "Firmware in unemu/ on registry.unemu.dev (1)",
    ] {
        assert!(shown.has_widget(label), "{label}");
    }
    let groups: Vec<f32> = ["Machine images", "Installation media", "Firmware"]
        .map(|g| shown.text(g).rect.top())
        .to_vec();
    assert!(groups.windows(2).all(|w| w[0] < w[1]), "{groups:?}");
    // The namespace's lines are the model's, locks included.
    let lines = tree_lines(
        rig.view.remote.model.as_ref().unwrap(),
        &rig.view.remote.open,
    );
    let lock = |label: &str| {
        lines
            .iter()
            .find(|l| l.label == label)
            .is_some_and(|l| l.tail.iter().any(|t| matches!(t, Tail::Lock)))
    };
    assert!(lock("private/"));
    assert!(!lock("unemu/"));

    // Selecting a group lists its repositories only.
    screen.click(
        &mut rig.view,
        "Installation media in unemu/ on registry.unemu.dev (2)",
    );
    assert_eq!(rig.view.remote.node, Some(unemu(Some(Group::Media))));
    let rows = screen.settled(&mut rig.view).rows();
    let names: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
    assert_eq!(names, ["unemu/media/freedos (2 tags)", "unemu/media/keen4"]);
    // The default registry listed already: selecting it sends nothing.
    assert!(rig.browsing().is_empty());
}

/// The list draws the model's rows in the mock-up's columns: the name with
/// its tag count, the registry, the title, the tag the row stands for, the
/// type ("Machine template" under unemu/), the licence, the size and the
/// local state in its colour. A tag this release cannot run says "Needs
/// unemupc 99.0.0 or newer" and its row is muted. Expanding a repository
/// reads its other tags and lists them newest first, aliases last.
#[test]
fn the_list_draws_columns_local_states_and_expands_a_repository() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(None));
    let mut screen = Screen::new(false, false, WIDE);
    let shown = screen.settled(&mut rig.view);
    for label in COLUMNS.labels {
        assert!(
            shown.has_widget(label) || shown.has_widget(&format!("{label} ▲")),
            "{label}"
        );
    }
    let rows = shown.rows();
    let row = |name: &str| {
        rows.iter()
            .find(|r| r[0] == name)
            .unwrap_or_else(|| panic!("{name} not in {rows:?}"))
    };
    assert_eq!(
        row("unemu/open-at (3 tags)"),
        &[
            "unemu/open-at (3 tags)",
            DEFAULT_REGISTRY,
            "Open AT 286",
            "1.2.0",
            "Machine template",
            "open",
            "3.5 MiB",
            "In library · 1 of 3 tags",
        ]
    );
    assert_eq!(
        row("unemu/open-at-games")[7],
        "Newer available · you have 1.0.0"
    );
    assert_eq!(row("unemu/clash")[7], "⚠ Local tag names another image");
    assert_eq!(row("unemu/media/freedos (2 tags)")[4], "Installation media");
    assert_eq!(row("unemu/media/freedos (2 tags)")[3], "1.4");
    assert_eq!(row("unemu/media/keen4")[5], "user-supplied");
    assert_eq!(
        row("unemu/media/keen4")[7],
        "Not pulled · same image as keen4:1.4"
    );
    assert_eq!(row("unemu/firmware/future-bios")[4], "Firmware package");
    assert_eq!(
        row("unemu/firmware/future-bios")[7],
        "Needs unemupc 99.0.0 or newer"
    );
    // Sorted by type, as the mock-up opens: machine images first.
    let names: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
    assert_eq!(
        names,
        [
            "unemu/clash",
            "unemu/open-at (3 tags)",
            "unemu/open-at-games",
            "unemu/media/freedos (2 tags)",
            "unemu/media/keen4",
            "unemu/firmware/future-bios",
        ]
    );
    // Colours: the states in theirs, the muted row's name muted.
    let visuals = crate::shell::skin_visuals(false, false);
    assert_eq!(
        shown.text("In library · 1 of 3 tags").colour,
        pane::good_colour(&visuals)
    );
    assert_eq!(
        shown.text("Needs unemupc 99.0.0 or newer").colour,
        visuals.warn_fg_color
    );
    assert_eq!(
        shown.text("unemu/firmware/future-bios").colour,
        visuals.weak_text_color()
    );
    assert_eq!(shown.text("unemu/clash").colour, visuals.text_color());

    // Expanding reads the tags not read yet, in one request.
    screen.click(&mut rig.view, "Expand unemu/open-at (3 tags)");
    let requests = rig.browsing();
    assert_eq!(requests.len(), 1);
    match &requests[0] {
        BrowseRequest::Read {
            repository, tags, ..
        } => {
            assert_eq!(repository, "unemu/open-at");
            assert_eq!(tags, &["1.0.0", "latest"]);
        }
        _ => panic!("expected a read"),
    }
    let rows = screen.settled(&mut rig.view).rows();
    let at = rows
        .iter()
        .position(|r| r[0] == "unemu/open-at (3 tags)")
        .unwrap();
    let children: Vec<(&str, &str)> = rows[at + 1..at + 4]
        .iter()
        .map(|r| (r[0].as_str(), r[7].as_str()))
        .collect();
    assert_eq!(
        children,
        [
            (":1.2.0", "In library"),
            (":1.0.0", "Reading…"),
            (":latest", "Reading…")
        ]
    );
    assert!(screen
        .settled(&mut rig.view)
        .has_widget("Collapse unemu/open-at (3 tags)"));
}

/// Every state of the model draws the mock-up's strip above the rows or
/// its message instead of them, with the model's wording and the action it
/// offers, and that action does what it says.
#[test]
fn every_registry_state_draws_its_strip_or_message_and_action() {
    let host = DEFAULT_REGISTRY;
    let failures: Vec<(&str, State, bool)> = vec![
        ("listing, no rows", State::Listing, false),
        ("listing", State::Listing, true),
        ("stopped", State::Stopped, true),
        ("stopped, no rows", State::Stopped, false),
        (
            "unreachable",
            State::Unreachable {
                reason: "The connection timed out".into(),
                stale: false,
            },
            false,
        ),
        (
            "unreachable after a listing",
            State::Unreachable {
                reason: "The connection timed out".into(),
                stale: true,
            },
            true,
        ),
        (
            "server error",
            State::ServerError {
                status: Some(503),
                reason: "Service Unavailable".into(),
            },
            false,
        ),
        (
            "no catalog",
            State::ServerError {
                status: Some(404),
                reason: "Not Found".into(),
            },
            false,
        ),
        (
            "login required",
            State::LoginRequired {
                challenge: Some(Challenge::Basic {
                    realm: Some("registry".into()),
                }),
                refused: false,
            },
            false,
        ),
        (
            "token login",
            State::LoginRequired {
                challenge: Some(Challenge::Bearer {
                    realm: "https://auth.example/token".into(),
                    service: None,
                    scope: None,
                }),
                refused: false,
            },
            false,
        ),
        (
            "login refused",
            State::LoginRequired {
                challenge: None,
                refused: true,
            },
            false,
        ),
        ("empty", State::Listed, false),
        (
            "local failure",
            State::Failed("The library could not be read".into()),
            false,
        ),
    ];
    for (what, state, rows) in failures {
        let mut rig = Rig::new();
        let registry = rig.registry(host);
        registry.state = state.clone();
        if rows {
            registry.repositories = sample(host);
            registry.expected = 9;
        }
        let (strip, message) = (registry.banner(), registry.message());
        rig.show(Node::registry(host));
        let mut screen = Screen::new(true, false, WIDE);
        let shown = screen.settled(&mut rig.view);
        let drawn = strip
            .as_ref()
            .or(message.as_ref())
            .unwrap_or_else(|| panic!("{what}"));
        if strip.is_some() {
            assert!(rows, "{what}: a strip over no rows");
            assert!(
                shown.has(&format!("{} {}", drawn.heading, drawn.text)),
                "{what}: no strip {drawn:?}"
            );
            assert!(
                shown.has_widget("Name"),
                "{what}: the rows stay under the strip"
            );
        } else {
            assert!(
                shown.has(&drawn.heading) && shown.has(&drawn.text),
                "{what}: {drawn:?}"
            );
            assert!(!shown.has_widget("Name"), "{what}: rows under a message");
        }
        let Some(action) = drawn.action else {
            for label in ["Stop", "Refresh", "Try again", "Log in…"] {
                assert!(!shown.has_widget(label), "{what}: {label}");
            }
            continue;
        };
        rig.sent();
        let configured = rig.registry(host).configured;
        screen.click(&mut rig.view, action_label(action, configured));
        match action {
            Action::Stop => {}
            Action::Refresh | Action::TryAgain => {
                let requests = rig.browsing();
                assert!(
                    matches!(
                        requests.as_slice(),
                        [BrowseRequest::List { next: None, .. }]
                    ),
                    "{what}: {action:?} lists again"
                );
                assert_eq!(rig.registry(host).state, State::Listing, "{what}");
            }
            Action::LogIn => {
                assert_eq!(rig.view.login_wanted.as_deref(), Some(host), "{what}");
                assert!(rig.browsing().is_empty(), "{what}");
                // Closing Preferences lists the registry again.
                rig.view.preferences_closed();
                assert_eq!(rig.browsing().len(), 1, "{what}");
            }
        }
    }
}

/// Stop in the strip of a running listing stops it at its next read: its
/// flag is set and the rows read so far stay under the stopped strip.
#[test]
fn stop_stops_a_running_listing_and_keeps_its_rows() {
    let mut rig = Rig::new();
    let mut file = None;
    rig.view
        .select_remote(Node::registry(DEFAULT_REGISTRY), &mut file);
    let requests = rig.browsing();
    let job = job_of(&requests[0]).clone();
    rig.view.on_browse(BrowseEvent::Page {
        id: job.id,
        repositories: vec!["unemu/open-at".into(), "unemu/media/freedos".into()],
        next: None,
    });
    rig.view.on_browse(BrowseEvent::Repository {
        id: job.id,
        row: sample(DEFAULT_REGISTRY).remove(0),
    });
    let mut screen = Screen::new(false, true, WIDE);
    let shown = screen.settled(&mut rig.view);
    assert!(shown.has(&format!(
        "Listing {DEFAULT_REGISTRY}… 1 of 2 repositories read; rows appear as their manifests arrive."
    )));
    screen.click(&mut rig.view, "Stop");
    assert!(job.cancel.load(Ordering::SeqCst));
    assert_eq!(rig.registry(DEFAULT_REGISTRY).state, State::Stopped);
    let shown = screen.settled(&mut rig.view);
    assert!(
        shown.has("Listing stopped after 1 of 2 repositories. Refresh lists the registry again.")
    );
    assert_eq!(shown.rows().len(), 1);
    // Answers of the stopped job change nothing.
    rig.view.on_browse(BrowseEvent::Repository {
        id: job.id,
        row: sample(DEFAULT_REGISTRY).remove(3),
    });
    assert_eq!(rig.registry(DEFAULT_REGISTRY).repositories.len(), 1);
}

/// The paging line counts the node's repositories; while the catalog has
/// more, Next 50 › reads its next page and adds to the rows.
#[test]
fn the_paging_line_offers_next_50_while_the_catalog_has_more() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(Some(Group::Machines)));
    let mut screen = Screen::new(true, true, WIDE);
    let shown = screen.settled(&mut rig.view);
    assert!(shown.has("Repositories 1–3 of 3. The catalog is read 50 repositories a page."));
    assert!(!shown.has_widget("Next 50 ›"));
    rig.registry(DEFAULT_REGISTRY).next = Some("/v2/_catalog?n=50&last=unemu%2Fopen-at".into());
    let shown = screen.settled(&mut rig.view);
    assert!(shown.has("Repositories 1–3. The catalog is read 50 repositories a page."));
    screen.click(&mut rig.view, "Next 50 ›");
    let requests = rig.browsing();
    assert!(
        matches!(
            requests.as_slice(),
            [BrowseRequest::List { next: Some(next), .. }] if next.contains("last=unemu%2Fopen-at")
        ),
        "Next 50 › reads the next page"
    );
    // The rows read so far stay while the next page is read.
    let shown = screen.settled(&mut rig.view);
    assert_eq!(shown.rows().len(), 3);
}

/// F5 (View › Refresh remote) and the toolbar's Refresh list the shown
/// registry again; neither applies while the library shows a collection.
#[test]
fn refresh_lists_the_shown_registry_again() {
    let mut rig = Rig::new();
    rig.view.refresh_remote();
    assert!(rig.sent().is_empty(), "no registry is shown");
    rig.listed();
    rig.show(unemu(None));
    rig.view.refresh_remote();
    let requests = rig.browsing();
    assert!(matches!(
        requests.as_slice(),
        [BrowseRequest::List { next: None, .. }]
    ));
    assert_eq!(job_of(&requests[0]).host, DEFAULT_REGISTRY);
    assert_eq!(rig.registry(DEFAULT_REGISTRY).state, State::Listing);
    // F5 is View › Refresh remote while the library shows, never the guest's.
    use crate::shell_model::{hotkey, Command, State as Run};
    use winit::keyboard::KeyCode;
    assert_eq!(
        hotkey(KeyCode::F5, [false; 8], Run::Stopped, false, false),
        Some(Command::RefreshRemote)
    );
    assert_eq!(
        hotkey(KeyCode::F5, [false; 8], Run::Stopped, false, true),
        Some(Command::RefreshRemote)
    );
    assert_eq!(
        hotkey(KeyCode::F5, [false; 8], Run::Running, true, false),
        None
    );
    assert_eq!(
        hotkey(KeyCode::F5, [false; 8], Run::Running, false, false),
        None
    );
}

/// Every request that sends a machine to run stops every registry's
/// listing and reads first, so a slow registry never delays a Start; other
/// requests leave them running.
#[test]
fn a_launch_stops_every_registry_first() {
    let row = || crate::library_view::ImageRow {
        reference: "test:source".into(),
        aliases: Vec::new(),
        descriptor: image_store::Descriptor::new(mt::MANIFEST, Digest::of(b"image"), 1),
        summary: None,
        failure: None,
    };
    type Launch = Box<dyn Fn() -> Request>;
    let launches: Vec<(&str, Launch)> = vec![
        (
            "Start of a machine",
            Box::new(|| Request::LaunchMachine("games".into(), false)),
        ),
        (
            "Gaming mode",
            Box::new(|| Request::LaunchMachine("games".into(), true)),
        ),
        (
            "Gaming mode on an image",
            Box::new(move || Request::Launch(row(), WorkspaceKind::Disposable, None)),
        ),
        (
            "Start on an image without a machine",
            Box::new(move || {
                Request::Machine(MachineJob::Create {
                    image: Box::new(row()),
                    name: None,
                    start: true,
                })
            }),
        ),
    ];
    for (what, request) in launches {
        let mut rig = Rig::new();
        let mut file = None;
        rig.view
            .select_remote(Node::registry(DEFAULT_REGISTRY), &mut file);
        rig.view.select_remote(Node::registry(NAS), &mut file);
        let jobs: Vec<Job> = rig.browsing().iter().map(|r| job_of(r).clone()).collect();
        assert_eq!(jobs.len(), 2);
        // Another request leaves them running.
        rig.view.send(Request::FirmwarePackages);
        assert!(
            jobs.iter().all(|job| !job.cancel.load(Ordering::SeqCst)),
            "{what}"
        );
        rig.view.send(request());
        assert!(
            jobs.iter().all(|job| job.cancel.load(Ordering::SeqCst)),
            "{what}: a listing still runs"
        );
        for host in [DEFAULT_REGISTRY, NAS] {
            assert_eq!(rig.registry(host).state, State::Stopped, "{what}");
        }
        assert!(
            rig.sent().iter().any(Request::launches),
            "{what}: the launch was sent after the stop"
        );
    }
    // The library's own Start of an image without a machine goes the same way.
    let mut rig = Rig::new();
    let mut file = None;
    rig.view
        .select_remote(Node::registry(DEFAULT_REGISTRY), &mut file);
    let job = job_of(&rig.browsing()[0]).clone();
    rig.view.start_image(row());
    assert!(job.cancel.load(Ordering::SeqCst));
}

/// The arrow keys move between the list's rows as in the library's lists,
/// Space selects the focused row and Enter expands a repository of several
/// tags.
#[test]
fn arrow_keys_move_between_rows_and_enter_expands() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(Some(Group::Machines)));
    let mut screen = Screen::new(true, false, WIDE);
    // The rows sort by type, then name: clash, open-at, open-at-games.
    screen.click(&mut rig.view, "unemu/clash");
    assert_eq!(
        rig.view.remote.selected,
        Some((
            DEFAULT_REGISTRY.into(),
            "unemu/clash".into(),
            "1.0.0".into()
        ))
    );
    let selected = rig.browsing();
    assert!(
        matches!(
            selected.as_slice(),
            [BrowseRequest::Read { details: true, .. }]
        ),
        "selecting a tag reads its metadata record"
    );
    screen.key(&mut rig.view, egui::Key::ArrowDown);
    screen.key(&mut rig.view, egui::Key::Enter);
    let key = (DEFAULT_REGISTRY.to_owned(), "unemu/open-at".to_owned());
    assert!(rig.view.remote.expanded.contains(&key), "Enter expands");
    assert!(matches!(
        rig.browsing().as_slice(),
        [BrowseRequest::Read { details: false, .. }]
    ));
    // Down again reaches the first tag of the expanded repository.
    screen.key(&mut rig.view, egui::Key::ArrowDown);
    screen.key(&mut rig.view, egui::Key::Space);
    assert_eq!(
        rig.view.remote.selected,
        Some((
            DEFAULT_REGISTRY.into(),
            "unemu/open-at".into(),
            "1.2.0".into()
        ))
    );
    // Enter on the expanded repository closes it again.
    screen.key(&mut rig.view, egui::Key::ArrowUp);
    screen.key(&mut rig.view, egui::Key::Enter);
    assert!(!rig.view.remote.expanded.contains(&key), "Enter collapses");
}

/// File › Get templates… and the empty library's button select Remotes ›
/// the default registry › unemu/ › Machine images: the tree opens to it,
/// the registry lists, the node is marked, and the list holds its machine
/// images only. The library's selection goes, so Start has no target.
#[test]
fn get_templates_selects_the_unemu_machine_images_node() {
    let mut rig = Rig::new();
    let (_temp, accounts) = preferences();
    rig.view.query.filter = Filter::Machines;
    let mut file = Some(0);
    rig.view.show_templates(accounts, &mut file);
    assert_eq!(file, None);
    assert_eq!(rig.view.remote.node, Some(unemu(Some(Group::Machines))));
    let requests = rig.browsing();
    assert_eq!(requests.len(), 1);
    assert_eq!(job_of(&requests[0]).host, DEFAULT_REGISTRY);
    rig.listed();
    let mut screen = Screen::new(false, false, WIDE);
    let shown = screen.settled(&mut rig.view);
    let node = shown
        .widget("Machine images in unemu/ on registry.unemu.dev (3)")
        .rect;
    // Only the node is marked: the Library's collections are not.
    let selection = crate::shell::skin_visuals(false, false)
        .selection
        .stroke
        .color;
    assert_eq!(shown.text("Machine images").colour, selection);
    assert_ne!(shown.text("Machines (0)").colour, selection);
    assert!(shown.widget("unemu/ on registry.unemu.dev (6)").rect.top() < node.top());
    let names: Vec<String> = shown.rows().into_iter().map(|r| r[0].clone()).collect();
    assert_eq!(
        names,
        [
            "unemu/clash",
            "unemu/open-at (3 tags)",
            "unemu/open-at-games"
        ]
    );
    // A Library collection takes the list back.
    screen.click(&mut rig.view, "Machines (0)");
    assert_eq!(rig.view.remote.node, None);
}

/// A tag's selection never becomes the library's: the details pane shows
/// the remote tag, and without a selection it says how to make one.
#[test]
fn a_remote_node_keeps_the_library_selection_apart() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(None));
    let mut screen = Screen::new(true, false, WIDE);
    let shown = screen.settled(&mut rig.view);
    assert!(shown.has(
        "Select a tag for its details and Pull…. Ctrl (⌘ on macOS) adds a tag to the selection; Shift selects a range."
    ));
    screen.click(&mut rig.view, "unemu/clash");
    assert!(rig.view.selected.is_none() && rig.view.machine.is_none());
    let shown = screen.settled(&mut rig.view);
    assert!(shown.has("Annotations, from the manifest") && shown.has("In your library"));
}

/// The list and the tree lay out in both skins and both themes at the
/// window widths the library is checked at: nothing moves or grows from
/// frame to frame, no text spills out of the window or its clip, and in the
/// tree, the strip and the paging line no text is cut at a side. Where the
/// pane is too narrow for the columns, the list scrolls sideways in its own
/// area, as the mock-up's grid does.
#[test]
fn remotes_lay_out_in_every_look_without_growing() {
    for (classic, dark) in LOOKS {
        for width in [760.0, 1000.0, 1400.0, 2000.0] {
            let what = format!("classic {classic}, dark {dark}, {width} px");
            let mut rig = Rig::new();
            rig.listed();
            rig.view.remote.open.insert(DEFAULT_REGISTRY.into());
            rig.view
                .remote
                .open
                .insert(namespace_key(DEFAULT_REGISTRY, Some(OFFICIAL_NAMESPACE)));
            rig.view.remote.open.insert(LAN.into());
            rig.view
                .remote
                .expanded
                .insert((DEFAULT_REGISTRY.into(), "unemu/open-at".into()));
            // A stale listing: the strip over the rows.
            rig.registry(DEFAULT_REGISTRY).state = State::Unreachable {
                reason: "The connection timed out".into(),
                stale: true,
            };
            rig.show(unemu(None));
            rig.view.classic = classic;
            let size = egui::vec2(width, 900.0);
            let mut screen = Screen::new(classic, dark, size);
            let first = screen.settled(&mut rig.view);
            let later = {
                screen.frame(&mut rig.view, Vec::new());
                screen.frame(&mut rig.view, Vec::new())
            };
            assert_eq!(first.texts, later.texts, "{what}: the texts moved");
            assert_eq!(first.widgets, later.widgets, "{what}: the widgets moved");
            let window = Rect::from_min_size(egui::Pos2::ZERO, size);
            let header = first.widget("Name").rect;
            let fits = width >= 1000.0;
            for text in &first.texts {
                let visible = text.rect.intersect(text.clip);
                if visible.is_positive() {
                    assert!(
                        window.expand(0.5).contains_rect(visible),
                        "{what}: {:?} leaves the window",
                        text.text
                    );
                }
                // The tree, the strip and the paging line keep every text
                // whole; the rows do too where the columns fit.
                let in_rows = text.rect.top() >= header.top() - 1.0
                    && text.rect.left() >= header.left() - 1.0
                    && !text.text.starts_with("Repositories");
                if !in_rows || fits {
                    assert!(
                        text.rect.left() >= text.clip.left() - 1.0
                            && text.rect.right() <= text.clip.right() + 1.0,
                        "{what}: {:?} is cut at a side ({:?} in {:?})",
                        text.text,
                        text.rect,
                        text.clip
                    );
                }
            }
            // The tree's lines stay inside the tree pane.
            let tree = first.text("Remotes").clip;
            for label in [
                DEFAULT_REGISTRY,
                "unemu/",
                "Machine images",
                LAN,
                "private/",
            ] {
                let line = first.text(label);
                assert!(
                    tree.contains_rect(line.rect),
                    "{what}: {label} {:?}",
                    line.rect
                );
            }
            assert!(first.has(&format!(
                "{DEFAULT_REGISTRY} does not answer. These rows are from an earlier listing in this session. Pull is unavailable until the registry answers."
            )));
        }
    }
}

/// A message instead of the rows stays in the list's area in every look and
/// keeps its place frame after frame.
#[test]
fn a_message_stays_in_place_in_every_look() {
    for (classic, dark) in LOOKS {
        for width in [760.0, 1400.0] {
            let what = format!("classic {classic}, dark {dark}, {width} px");
            let mut rig = Rig::new();
            rig.registry(LAN).state = State::LoginRequired {
                challenge: Some(Challenge::Basic { realm: None }),
                refused: false,
            };
            rig.show(Node::registry(LAN));
            rig.view.classic = classic;
            let size = egui::vec2(width, 700.0);
            let mut screen = Screen::new(classic, dark, size);
            let first = screen.settled(&mut rig.view);
            let later = screen.settled(&mut rig.view);
            assert_eq!(first.texts, later.texts, "{what}");
            let heading = first.text(&format!("{LAN} asks for a login"));
            let button = first.widget("Log in…").rect;
            for rect in [heading.rect, button] {
                assert!(
                    rect.left() >= 230.0 && rect.right() <= width && rect.bottom() <= 700.0,
                    "{what}: {rect:?}"
                );
            }
            assert!(heading.rect.right() <= heading.clip.right() + 1.0, "{what}");
        }
    }
}

/// The library lists' Source column reads what the store records on a tag:
/// the registry a pull wrote it from, "local" for a tag a write here made,
/// and a dash for a tag with neither record, written before the store kept
/// them, whose source the window never guesses, whatever its name.
#[test]
fn the_source_column_names_the_registry_local_or_unknown() {
    use image_store::{MADE_HERE, PULL_REGISTRY};
    let descriptor = |record: Option<(&str, &str)>| {
        let mut d = image_store::Descriptor::new(mt::MANIFEST, Digest::of(b"x"), 1);
        if let Some((key, value)) = record {
            d.annotations.insert(key.into(), value.into());
        }
        d
    };
    let source = |record| super::super::source(&descriptor(record)).text();
    assert_eq!(source(Some((PULL_REGISTRY, NAS))), NAS);
    assert_eq!(
        source(Some((PULL_REGISTRY, DEFAULT_REGISTRY))),
        DEFAULT_REGISTRY
    );
    assert_eq!(source(Some((MADE_HERE, "true"))), "local");
    assert_eq!(source(None), "—");
    // A record that is not a registry host, or not the mark, is no record.
    assert_eq!(source(Some((PULL_REGISTRY, "Not A Host"))), "—");
    assert_eq!(source(Some((MADE_HERE, "false"))), "—");

    // The Images list draws it after the name: a pull's record, the mark of
    // a write here, and a tag from before either.
    let (temp, store, images) = super::super::tests::catalogue(3);
    let entry = |name: &str, image: &image_store::Descriptor, record: Option<(&str, &str)>| {
        let record = record
            .map(|(key, value)| format!(",\"{key}\":\"{value}\""))
            .unwrap_or_default();
        format!(
            "{{\"mediaType\":\"{}\",\"digest\":\"{}\",\"size\":{},\"annotations\":{{\"org.opencontainers.image.ref.name\":\"{name}\"{record}}}}}",
            mt::MANIFEST,
            image.digest,
            image.size
        )
    };
    std::fs::write(
        store.root().join("images/index.json"),
        format!(
            "{{\"schemaVersion\":2,\"mediaType\":\"{}\",\"manifests\":[{},{}]}}",
            mt::INDEX,
            entry(
                "open-at:1.2.0",
                &images[0],
                Some((PULL_REGISTRY, DEFAULT_REGISTRY))
            ),
            entry("nas.example:5000/private/older:1.0", &images[1], None),
        ),
    )
    .unwrap();
    // A tag the store writes here carries the mark.
    store
        .update_tag(
            "made-here:1.0",
            &images[2],
            None,
            &super::super::tests::Provider,
        )
        .unwrap();
    let cache = super::super::firmware::tests::warm(&store);
    let mut view = LibraryView::new(None);
    view.query.filter = Filter::All;
    view.snapshot = Arc::new(super::super::snapshot(&cache, &view.query));
    let mut sources: Vec<(String, String)> = view
        .snapshot
        .rows
        .iter()
        .map(|row| {
            (
                row.image.reference.clone(),
                super::super::row_cells(row, 0)[1].clone(),
            )
        })
        .collect();
    sources.sort();
    assert_eq!(
        sources,
        [
            ("made-here:1.0".into(), "local".into()),
            ("nas.example:5000/private/older:1.0".into(), "—".into()),
            ("open-at:1.2.0".into(), DEFAULT_REGISTRY.into()),
        ]
    );
    let mut screen = Screen::new(true, false, WIDE);
    let shown = screen.settled(&mut view);
    assert!(shown.has_widget("Source") || shown.has_widget("Source ▲"));
    for text in [DEFAULT_REGISTRY, "local", "—"] {
        assert!(shown.has_widget(text), "{text}");
    }
    drop(temp);
}

/// Leaving a snapshot whose tags differ asks the worker to compare the read
/// tags with the library again; an unchanged one asks nothing.
#[test]
fn a_changed_library_compares_the_read_tags_again() {
    let mut rig = Rig::new();
    rig.listed();
    let tags = vec![("open-at:1.2.0".to_owned(), Digest::of(b"Open AT 286"))];
    let snapshot = |tags: Vec<(String, Digest)>| {
        Event::Snapshot(Arc::new(Snapshot {
            tags,
            ..Snapshot::default()
        }))
    };
    rig.view.on_event(snapshot(tags.clone()));
    assert!(matches!(
        rig.browsing().as_slice(),
        [BrowseRequest::Local { .. }]
    ));
    rig.view.on_event(snapshot(tags));
    assert!(rig.browsing().is_empty());
}

fn image_row() -> crate::library_view::ImageRow {
    crate::library_view::ImageRow {
        reference: "test:source".into(),
        aliases: Vec::new(),
        descriptor: image_store::Descriptor::new(mt::MANIFEST, Digest::of(b"image"), 1),
        summary: None,
        failure: None,
    }
}

/// Listings of two registries, running, with their jobs.
fn two_listings(rig: &mut Rig) -> Vec<Job> {
    let mut file = None;
    rig.view
        .select_remote(Node::registry(DEFAULT_REGISTRY), &mut file);
    rig.view.select_remote(Node::registry(NAS), &mut file);
    let jobs: Vec<Job> = rig.browsing().iter().map(|r| job_of(r).clone()).collect();
    assert_eq!(jobs.len(), 2);
    jobs
}

/// Start or Gaming mode on a machine whose readiness is not known yet asks
/// for a check first, which waits behind a registry listing on the worker:
/// every way a user starts a machine stops the registries before anything
/// is sent.
#[test]
fn every_start_stops_the_registries_before_the_worker_is_asked() {
    type Start = Box<dyn Fn(&mut LibraryView)>;
    let unchecked = |kind: WorkspaceKind| -> Start {
        Box::new(move |view: &mut LibraryView| {
            view.machine = Some("games".into());
            view.selected = Some(image_row());
            view.readiness = None;
            view.launch(kind);
        })
    };
    let starts: Vec<(&str, Start)> = vec![
        ("Start", unchecked(WorkspaceKind::Regular)),
        ("Gaming mode", unchecked(WorkspaceKind::Disposable)),
        (
            "Image › Gaming mode",
            Box::new(|view: &mut LibraryView| {
                view.selected = Some(image_row());
                view.image_gaming();
            }),
        ),
        (
            "a power cycle",
            Box::new(|view: &mut LibraryView| {
                view.resume(image_row(), "games".into(), WorkspaceKind::Machine)
            }),
        ),
    ];
    for (what, start) in starts {
        let mut rig = Rig::new();
        let jobs = two_listings(&mut rig);
        start(&mut rig.view);
        assert!(
            jobs.iter().all(|job| job.cancel.load(Ordering::SeqCst)),
            "{what}: a listing still runs"
        );
        let sent = rig.sent();
        assert!(
            sent.iter()
                .any(|r| r.launches() || matches!(r, Request::Check(..))),
            "{what}: nothing was asked of the worker"
        );
    }
}

/// When the library worker goes away, every running registry job ends with
/// its reason instead of listing forever.
#[test]
fn a_worker_that_goes_away_ends_every_registry_job() {
    let mut rig = Rig::new();
    two_listings(&mut rig);
    rig.events = None;
    rig.view.poll(&egui::Context::default());
    for host in [DEFAULT_REGISTRY, NAS] {
        let state = &rig.registry(host).state;
        assert!(
            matches!(state, State::Failed(reason) if reason.contains("worker stopped")),
            "{host}: {state:?}"
        );
    }
}

/// An expanded repository whose tags are not read yet collapses when its
/// reads stop or a new listing replaces its row, instead of reading
/// "Reading…" for ever: on F5, Try again, Stop and a machine's start.
#[test]
fn unread_expanded_repositories_collapse_when_their_reads_cannot_finish() {
    let key = (DEFAULT_REGISTRY.to_owned(), "unemu/open-at".to_owned());
    type Stop = Box<dyn Fn(&mut Rig)>;
    let stops: Vec<(&str, Stop)> = vec![
        ("F5", Box::new(|rig: &mut Rig| rig.view.refresh_remote())),
        (
            "Stop",
            Box::new(|rig: &mut Rig| {
                rig.view.apply_list(
                    DEFAULT_REGISTRY,
                    ListAnswer {
                        action: Some(Action::Stop),
                        ..ListAnswer::default()
                    },
                )
            }),
        ),
        (
            "Try again",
            Box::new(|rig: &mut Rig| {
                rig.view.apply_list(
                    DEFAULT_REGISTRY,
                    ListAnswer {
                        action: Some(Action::TryAgain),
                        ..ListAnswer::default()
                    },
                )
            }),
        ),
        ("a start", Box::new(|rig: &mut Rig| rig.view.stop_remotes())),
    ];
    for (what, stop) in stops {
        let mut rig = Rig::new();
        rig.listed();
        rig.show(unemu(None));
        let mut screen = Screen::new(true, false, WIDE);
        screen.click(&mut rig.view, "Expand unemu/open-at (3 tags)");
        assert!(rig.view.remote.expanded.contains(&key), "{what}");
        stop(&mut rig);
        assert!(!rig.view.remote.expanded.contains(&key), "{what}");
        let shown = screen.settled(&mut rig.view);
        assert!(!shown.has("Reading…"), "{what}");
    }
    // A repository whose tags were all read stays open.
    let mut rig = Rig::new();
    rig.listed();
    let registry = rig.registry(DEFAULT_REGISTRY);
    for tag in ["1.0.0", "latest"] {
        registry.repositories[0].read.push(read(
            DEFAULT_REGISTRY,
            "unemu/open-at",
            tag,
            record(mt::MACHINE, "Open AT 286"),
            LocalState::NotPulled { same_as: None },
        ));
    }
    rig.view.remote.expanded.insert(key.clone());
    rig.view.stop_remotes();
    assert!(rig.view.remote.expanded.contains(&key));
}

/// On a selected node the mark and the count after the name take the
/// selection's text colour, as the name does; elsewhere they are muted.
#[test]
fn a_selected_node_draws_its_mark_and_count_in_the_selection_colour() {
    for (classic, dark) in LOOKS {
        let mut rig = Rig::new();
        rig.listed();
        rig.view.remote.open.insert(DEFAULT_REGISTRY.into());
        let mut screen = Screen::new(classic, dark, WIDE);
        let visuals = crate::shell::skin_visuals(classic, dark);
        let selection = visuals.selection.stroke.color;
        let shown = screen.settled(&mut rig.view);
        assert_eq!(
            shown.text("official, public").colour,
            visuals.weak_text_color()
        );
        assert_eq!(shown.text("(6)").colour, visuals.weak_text_color());
        rig.show(Node::registry(DEFAULT_REGISTRY));
        let shown = screen.settled(&mut rig.view);
        assert_eq!(shown.text("official, public").colour, selection);
        rig.show(unemu(None));
        let shown = screen.settled(&mut rig.view);
        assert_eq!(shown.text("(6)").colour, selection);
    }
}

/// The Licence badge reads as the library's: open in the good colour,
/// user-supplied as a warning.
#[test]
fn the_licence_badge_takes_the_library_colours() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(Some(Group::Media)));
    let mut screen = Screen::new(false, false, WIDE);
    let shown = screen.settled(&mut rig.view);
    let visuals = crate::shell::skin_visuals(false, false);
    assert_eq!(shown.text("open").colour, pane::good_colour(&visuals));
    assert_eq!(shown.text("user-supplied").colour, visuals.warn_fg_color);
}

/// Showing a machine, as Start… and a created machine do, leaves Remotes
/// for the Machines collection.
#[test]
fn showing_a_machine_leaves_remotes() {
    let mut rig = Rig::new();
    rig.show(unemu(None));
    rig.view.show_machine("games".into(), image_row());
    assert_eq!(rig.view.remote.node, None);
    assert!(rig.view.query.filter.is_machines());
}

/// Remotes take the accounts they are given again: another preferences
/// file names other registries.
#[test]
fn attaching_again_takes_the_new_accounts() {
    let mut rig = Rig::new();
    let temp = assembly::test_support::TempDir::new("remotes-other-preferences");
    let path = temp.path().join("preferences.toml");
    std::fs::write(&path, "[[registries]]\nhost = \"other.example\"\n").unwrap();
    let (_, mut accounts) = preferences();
    accounts.path = Some(path);
    rig.view.attach_remotes(accounts);
    let hosts: Vec<&str> = rig
        .view
        .remote
        .model
        .as_ref()
        .unwrap()
        .registries
        .iter()
        .map(|v| v.host.as_str())
        .collect();
    assert_eq!(hosts, [DEFAULT_REGISTRY, "other.example"]);
}

/// The list's sorted lines and the tree's lines are kept until the
/// registries, the node, the sort or what is open change: a frame that
/// changes nothing sorts nothing again.
#[test]
fn the_list_and_the_tree_are_derived_once_per_change() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(Some(Group::Machines)));
    let mut screen = Screen::new(true, false, WIDE);
    let first = |shown: &Shown| shown.rows()[0][0].clone();
    assert_eq!(first(&screen.settled(&mut rig.view)), "unemu/clash");
    // A change the model does not count keeps the sorted order: the row
    // renamed past the others stays first.
    rig.view.remote.model.as_mut().unwrap().registries[0].repositories[2].name =
        "unemu/zz-clash".into();
    assert_eq!(first(&screen.settled(&mut rig.view)), "unemu/zz-clash");
    // A counted change sorts again.
    rig.model();
    assert_eq!(
        first(&screen.settled(&mut rig.view)),
        "unemu/open-at (3 tags)"
    );
}

/// Every tree line has an accessible name that says what its marks show:
/// a lock, a running listing, and which registry a namespace lies on, so
/// the same namespace on two registries reads apart.
#[test]
fn tree_lines_have_distinct_accessible_names() {
    let mut rig = Rig::new();
    rig.view.remote.open.insert(LAN.into());
    rig.view.remote.open.insert(NAS.into());
    rig.registry(DEFAULT_REGISTRY).state = State::Listing;
    let nas = rig.registry(NAS);
    nas.repositories = sample(NAS)
        .into_iter()
        .map(|mut r| {
            r.name = r.name.replacen("unemu/", "private/", 1);
            r
        })
        .collect();
    nas.state = State::Listed;
    let mut screen = Screen::new(false, true, WIDE);
    let shown = screen.settled(&mut rig.view);
    for label in [
        "registry.unemu.dev listing",
        "private/ on registry.lan.example locked until you log in",
        "private/ on nas.example:5000 (6)",
        "Expand private/ on nas.example:5000",
    ] {
        assert!(shown.has_widget(label), "{label}");
    }
}

/// A repository row shows the selection of any of its tags while it is
/// collapsed.
#[test]
fn a_collapsed_repository_shows_the_selection_of_its_tag() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(Some(Group::Machines)));
    rig.view.remote.selected = Some((
        DEFAULT_REGISTRY.into(),
        "unemu/open-at".into(),
        "1.0.0".into(),
    ));
    let mut screen = Screen::new(true, false, WIDE);
    let shown = screen.settled(&mut rig.view);
    let selection = crate::shell::skin_visuals(true, false)
        .selection
        .stroke
        .color;
    assert_eq!(shown.text("unemu/open-at (3 tags)").colour, selection);
    assert_ne!(shown.text("unemu/clash").colour, selection);
}

/// Click the widget labelled `label` with `modifiers` held.
fn click_with(
    screen: &mut Screen,
    view: &mut LibraryView,
    label: &str,
    modifiers: egui::Modifiers,
) {
    let at = screen.settled(view).widget(label).rect.center();
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    // egui keeps the modifiers from the event that changes them.
    screen.time += 1.0;
    screen.frame(
        view,
        vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::PointerMoved(at),
        ],
    );
    screen.frame(view, vec![button(true)]);
    screen.frame(view, vec![button(false)]);
    screen.frame(
        view,
        vec![
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            egui::Event::PointerMoved(egui::pos2(1.0, 1.0)),
        ],
    );
}

/// A record as the registry's manifest and metadata record give it: the
/// annotations the pane shows, a config and one layer, and the edition and
/// version the selection read.
fn rich(artifact_type: &str, title: &str, layer: &str) -> TagRecord {
    let mut record = record(artifact_type, title);
    for (key, value) in [
        ("org.opencontainers.image.title", title),
        ("org.opencontainers.image.vendor", "unemu"),
        (
            "org.opencontainers.image.description",
            "An IBM 5170-class 80286 with open firmware only.",
        ),
        ("org.opencontainers.image.licenses", "GPL-3.0-or-later"),
        (
            "org.opencontainers.image.source",
            "Three source archives travel in the image.",
        ),
    ] {
        record.annotations.insert(key.into(), value.into());
    }
    record.details = Some(Ok(crate::remote_browse::Details {
        edition: Some("Floppy Edition".into()),
        version: Some("1.4".into()),
    }));
    let config = image_store::Descriptor::new(mt::CONFIG, Digest::of(title.as_bytes()), 2048);
    let mut payload =
        image_store::Descriptor::new(mt::FIRMWARE, Digest::of(layer.as_bytes()), 1 << 20);
    payload
        .annotations
        .insert("org.opencontainers.image.title".into(), layer.into());
    record.descriptors = vec![config, payload];
    record.manifest_size = 1024;
    record.size = 1024 + 2048 + (1 << 20);
    record
}

/// The details pane of a machine image, a media set and a firmware package:
/// the manifest's annotations, digest and type, the edition and version
/// the metadata record adds, the local tag a pull writes and its source,
/// the local state, the download with the blobs the store holds left out,
/// the repository's tags, and the buttons each kind offers, with the reason
/// a disabled one gives.
#[test]
fn the_details_pane_shows_a_machine_image_a_media_set_and_a_firmware_package() {
    let host = DEFAULT_REGISTRY;
    let machine = rich(mt::MACHINE, "Open AT 286", "unemu AT BIOS ROM");
    let mut machine_row = read(
        host,
        "unemu/open-at",
        "1.2.0",
        machine.clone(),
        LocalState::NotPulled { same_as: None },
    );
    // The firmware ROM is in the store already: only the rest is fetched.
    machine_row
        .stored
        .insert(machine.descriptors[1].digest.clone());
    let mut set = rich(mt::MEDIA_SET, "FreeDOS", "Disk 1");
    set.media = (1..=7)
        .map(|i| (format!("disk-{i}"), "1.44M".to_owned()))
        .collect();
    set.formats = vec!["1.44M".into()];
    let package = rich(mt::FIRMWARE, "XTIDE Universal BIOS", "XUB ROM");
    let mut rig = Rig::new();
    let registry = rig.registry(host);
    registry.repositories = vec![
        repository("unemu/open-at", &["1.0.0", "1.2.0"], vec![machine_row]),
        repository(
            "unemu/media/freedos",
            &["1.4"],
            vec![read(
                host,
                "unemu/media/freedos",
                "1.4",
                set,
                LocalState::NotPulled { same_as: None },
            )],
        ),
        repository(
            "unemu/firmware/xub",
            &["1"],
            vec![read(
                host,
                "unemu/firmware/xub",
                "1",
                package,
                LocalState::InLibrary,
            )],
        ),
    ];
    registry.state = State::Listed;
    rig.show(unemu(None));
    let mut screen = Screen::new(true, false, WIDE);
    let select = |rig: &mut Rig, repository: &str, tag: &str| {
        rig.view.remote.selected = Some((host.into(), repository.into(), tag.into()));
    };

    select(&mut rig, "unemu/open-at", "1.2.0");
    let shown = screen.settled(&mut rig.view);
    for text in [
        "Annotations, from the manifest",
        "Open AT 286",
        "unemu",
        "An IBM 5170-class 80286 with open firmware only.",
        "1.4",
        "Floppy Edition",
        "GPL-3.0-or-later",
        "Three source archives travel in the image.",
        "Machine template",
        mt::MACHINE,
        "· Pull uses this digest",
        "In your library",
        "open-at:1.2.0",
        "· the default registry keeps short names",
        "registry.unemu.dev",
        "Not in the library under this name.",
        "Download: 3.0 KiB to fetch of 1.0 MiB; 1 of 2 blobs already in the store.",
        "Tags of this repository",
        "Read from the registry: the tag list and this manifest. No layer, cover or disk has been fetched.",
    ] {
        assert!(shown.shows(text), "machine image: {text:?}");
    }
    for button in ["Pull…", "Pull and create machine…", "Contents…"] {
        assert!(shown.has_widget(button), "machine image: {button}");
    }
    assert!(!shown.has_widget("Pull and insert…"));

    select(&mut rig, "unemu/media/freedos", "1.4");
    let shown = screen.settled(&mut rig.view);
    for text in [
        "7 × 1.44M diskettes",
        "Installation media",
        "unemu/media/freedos:1.4",
        // No machine has a drive for the set yet.
        "Pull and insert… needs a stopped machine with a drive that takes 1.44M disks; none has one.",
    ] {
        assert!(shown.shows(text), "media set: {text:?}");
    }
    assert!(shown.has_widget("Pull and insert…"));
    assert!(!shown.has_widget("Pull and create machine…"));
    screen.click(&mut rig.view, "Pull and insert…");
    assert!(rig.view.remote_pull.is_none(), "disabled without a drive");

    select(&mut rig, "unemu/firmware/xub", "1");
    let shown = screen.settled(&mut rig.view);
    for text in [
        "Firmware package",
        "In the library under this name.",
        "Download: nothing.",
        "Already in the library under this name; Pull has nothing to do.",
    ] {
        assert!(shown.shows(text), "firmware package: {text:?}");
    }
    assert!(shown.has_widget("Show in library"));
    assert!(!shown.has_widget("Pull and create machine…") && !shown.has_widget("Pull and insert…"));
    screen.click(&mut rig.view, "Pull…");
    assert!(rig.view.remote_pull.is_none(), "nothing to pull");
    // Show in library selects the package in the Firmware collection once
    // the library lists it.
    screen.click(&mut rig.view, "Show in library");
    assert_eq!(rig.view.remote.node, None);
    assert_eq!(rig.view.query.filter, Filter::Firmware);
    assert_eq!(
        rig.view.firmware_reselect.as_deref(),
        Some("unemu/firmware/xub:1")
    );

    // Contents… lists every descriptor, with what the store holds.
    rig.show(unemu(None));
    select(&mut rig, "unemu/open-at", "1.2.0");
    screen.click(&mut rig.view, "Contents…");
    assert_eq!(
        rig.view.remote.contents,
        Some((host.into(), "unemu/open-at".into(), "1.2.0".into()))
    );
    let ctx = egui::Context::default();
    let (_, texts) = crate::registry_transfer_view::tests::render(&ctx, Vec::new(), |ctx| {
        rig.view.modals(ctx, None, Vec::new(), true, false)
    });
    for text in [
        "Contents — unemu/open-at:1.2.0",
        "Manifest",
        "image manifest",
        "machine config",
        "unemu AT BIOS ROM",
        "firmware",
        "1.0 MiB",
        "Yes — not fetched again",
        "No",
        "open-at:1.2.0 · what Pull would write",
    ] {
        assert!(
            crate::registry_transfer_view::tests::shows(&texts, text),
            "Contents…: {text:?} in {texts:?}"
        );
    }
}

/// Ctrl (⌘ on macOS) adds a tag to the selection or takes it out, Shift
/// selects the rows between the last click and this one; the pane lists
/// them and Pull… takes those it can, leaving the others out with the
/// reason.
#[test]
fn ctrl_and_shift_select_several_tags_for_one_pull() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(None));
    let mut screen = Screen::new(true, false, WIDE);
    screen.click(&mut rig.view, "unemu/media/freedos");
    click_with(
        &mut screen,
        &mut rig.view,
        "unemu/firmware/future-bios",
        egui::Modifiers::SHIFT,
    );
    let ids = |rig: &Rig| -> Vec<String> {
        rig.view
            .remote
            .selection()
            .into_iter()
            .map(|(repository, tag)| format!("{repository}:{tag}"))
            .collect()
    };
    assert_eq!(
        ids(&rig),
        [
            "unemu/media/freedos:1.4",
            "unemu/media/keen4:1.4",
            "unemu/firmware/future-bios:1"
        ]
    );
    let shown = screen.settled(&mut rig.view);
    assert!(shown.has("3 tags selected"));
    assert!(
        shown.has_widget("Pull 2 tags…"),
        "the future BIOS is left out"
    );
    click_with(
        &mut screen,
        &mut rig.view,
        "unemu/media/keen4",
        egui::Modifiers::COMMAND,
    );
    assert_eq!(
        ids(&rig),
        ["unemu/media/freedos:1.4", "unemu/firmware/future-bios:1"]
    );
    click_with(
        &mut screen,
        &mut rig.view,
        "unemu/open-at-games",
        egui::Modifiers::COMMAND,
    );
    let shown = screen.settled(&mut rig.view);
    assert!(shown.has("3 tags selected"));
    assert!(rig.view.can_pull_remote());
    screen.click(&mut rig.view, "Pull 2 tags…");
    let dialog = rig.view.remote_pull.as_ref().expect("Pull… opens");
    assert!(dialog.several);
    let skipped: Vec<Option<&str>> = dialog.items.iter().map(|i| i.skip.as_deref()).collect();
    assert_eq!(skipped, [None, Some("needs unemupc 99.0.0 or newer"), None]);
    // A plain click selects one tag again.
    rig.view.remote_pull = None;
    screen.click(&mut rig.view, "unemu/clash");
    assert_eq!(ids(&rig), ["unemu/clash:1.0.0"]);
}

/// Pull and insert… is offered while a stopped machine has a drive that
/// takes the set; once the set is pulled, its first disk waits in that
/// drive for the machine's next Start. A running machine's drives are not
/// offered.
#[test]
fn pull_and_insert_leaves_the_first_disk_pending_in_a_stopped_machine() {
    use crate::{
        library_view::machines::tests::row,
        registry_transfer::{TransferEvent, TransferRequest},
    };
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(None));
    rig.view.machines = Arc::new(vec![row("games", "Games machine", "games", &[])]);
    rig.view.active_machine = Some(("games".into(), "Games machine".into()));
    let mut screen = Screen::new(true, false, WIDE);
    screen.click(&mut rig.view, "unemu/media/freedos");
    screen.click(&mut rig.view, "Pull and insert…");
    assert!(rig.view.remote_pull.is_none(), "the machine runs");
    rig.view.active_machine = None;
    screen.click(&mut rig.view, "Pull and insert…");
    let dialog = rig
        .view
        .remote_pull
        .as_mut()
        .expect("Pull and insert… opens");
    assert_eq!(dialog.mode, crate::remote_pull::Mode::Insert);
    assert_eq!(
        dialog.drives.iter().map(|d| d.label()).collect::<Vec<_>>(),
        ["Games machine — A: 1.44M"]
    );
    let requests = dialog.start();
    let [TransferRequest::Pull(job)] = requests.as_slice() else {
        panic!("one pull")
    };
    rig.sent();
    rig.view.on_event(Event::Transfer(TransferEvent::Pulled {
        id: job.id,
        result: Ok(image_registry::pull::PullReport {
            local_tag: job.local_tag.clone(),
            image: image_store::Descriptor::new(mt::MANIFEST, Digest::of(b"FreeDOS"), 1024),
            fetched: Vec::new(),
            present: Vec::new(),
            outcome: image_registry::pull::PullOutcome::Pulled,
            alias: None,
        }),
    }));
    let pending: Vec<_> = rig
        .sent()
        .into_iter()
        .filter_map(|request| match request {
            Request::Machine(MachineJob::SetPending { id, drive, insert }) => {
                Some((id, drive, insert))
            }
            _ => None,
        })
        .collect();
    let [(id, drive, Some(insert))] = pending.as_slice() else {
        panic!("{pending:?}")
    };
    assert_eq!((id.as_str(), drive.as_str()), ("games", "A:"));
    assert_eq!(insert.reference, "unemu/media/freedos:1.4");
    assert_eq!(insert.medium, "disk-1");
    assert!(insert.all, "both disks join the catalogue");
    assert_eq!(
        rig.view.status.as_deref(),
        Some("Pulled unemu/media/freedos:1.4")
    );
}

/// Pull and create machine… pulls, then New machine from image… opens on
/// the pulled image with the manifest's title as the name.
#[test]
fn pull_and_create_machine_opens_new_machine_from_image_after_the_pull() {
    use crate::registry_transfer::{TransferEvent, TransferRequest};
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(None));
    let mut screen = Screen::new(true, false, WIDE);
    screen.click(&mut rig.view, "unemu/open-at-games");
    screen.click(&mut rig.view, "Pull and create machine…");
    let dialog = rig.view.remote_pull.as_mut().expect("the dialog opens");
    let requests = dialog.start();
    let [TransferRequest::Pull(job)] = requests.as_slice() else {
        panic!("one pull")
    };
    rig.view.on_event(Event::Transfer(TransferEvent::Pulled {
        id: job.id,
        result: Ok(image_registry::pull::PullReport {
            local_tag: job.local_tag.clone(),
            image: image_store::Descriptor::new(mt::MANIFEST, Digest::of(b"x"), 1024),
            fetched: Vec::new(),
            present: Vec::new(),
            outcome: image_registry::pull::PullOutcome::Pulled,
            alias: None,
        }),
    }));
    let ctx = egui::Context::default();
    crate::registry_transfer_view::tests::click(&ctx, "New machine from image…", |ctx| {
        rig.view.modals(ctx, None, Vec::new(), true, false)
    });
    assert!(rig.view.remote_pull.is_none());
    assert_eq!(rig.view.remote.node, None, "the image is shown in Images");
    let selected = rig.view.selected.as_ref().unwrap();
    assert_eq!(selected.reference, "open-at-games:1.2.0");
    let Some(crate::library_view::machines::Dialog::Name(dialog)) = &rig.view.machine_dialog else {
        panic!("New machine from image… opens")
    };
    assert_eq!(dialog.title, "New machine from image");
    assert_eq!(dialog.value, "Open AT 286 for games");
}

/// A login on a registry preferences list is Log in…; on one they do not
/// list it is Add registry…, which adds the entry first. Either asks the
/// shell for Preferences on that registry.
#[test]
fn a_registry_without_an_entry_offers_add_registry_instead_of_log_in() {
    for (host, label) in [(LAN, "Log in…"), (DEFAULT_REGISTRY, "Add registry…")] {
        let mut rig = Rig::new();
        rig.registry(host).state = State::LoginRequired {
            challenge: Some(Challenge::Basic { realm: None }),
            refused: false,
        };
        rig.show(Node::registry(host));
        let mut screen = Screen::new(true, false, WIDE);
        let shown = screen.settled(&mut rig.view);
        assert!(shown.has_widget(label), "{host}");
        screen.click(&mut rig.view, label);
        assert_eq!(rig.view.login_wanted.as_deref(), Some(host));
    }
}

/// Pull… from Remotes against a registry, through the library worker. One
/// tag is pulled by the digest the list showed and its local tag is written
/// last, after every blob; the row then turns In library from the store,
/// without a new listing. A tag whose local name holds another image is
/// offered another local tag; the clashing one is refused, and the local tag
/// never moves. Several tags pull one after another, the ones Pull cannot
/// take left out; Cancel stops the running pull at its next read and the
/// waiting one before it contacts the registry.
#[test]
fn pulling_from_remotes_writes_the_local_tag_last_and_never_moves_one() {
    use crate::{
        registry_transfer::tests::{retitled, wait_pumping, Lab},
        remote_pull::{Mode, Stage},
    };
    let mut lab = Lab::new(true);
    let host = lab.registry.host();
    let source = lab.f.row.descriptor.clone();
    lab.push_through("test:source", &format!("{host}/private/test:1.0.0"));
    let copy = retitled(&lab.f, "test:copy");
    lab.push_through("test:copy", &format!("{host}/private/copy:1.0.0"));
    let third = retitled(&lab.f, "test:third");
    lab.push_through("test:third", &format!("{host}/private/third:1.0.0"));
    let local = |repository: &str| format!("{host}/private/{repository}:1.0.0");

    let listed = |dir: &str| {
        let target = assembly::test_support::TempDir::new(dir);
        let store = image_store::Store::open(target.path().join("store")).unwrap();
        let mut view = LibraryView::synthetic(store.root().to_owned());
        view.attach_remotes(lab.accounts.clone());
        let mut file = None;
        view.select_remote(Node::registry(&host), &mut file);
        view.pump_until(|v| {
            v.remote
                .model
                .as_ref()
                .unwrap()
                .registry(&host)
                .unwrap()
                .state
                == State::Listed
        });
        (target, store, view)
    };
    let row = |view: &LibraryView, repository: &str| -> TagRow {
        view.remote
            .model
            .as_ref()
            .unwrap()
            .registry(&host)
            .unwrap()
            .repository(&format!("private/{repository}"))
            .unwrap()
            .representative_row()
            .unwrap()
            .clone()
    };
    let select = |view: &mut LibraryView, repositories: &[&str]| {
        let ids: Vec<(String, String)> = repositories
            .iter()
            .map(|r| (format!("private/{r}"), "1.0.0".to_owned()))
            .collect();
        view.remote.selected = Some((host.clone(), ids[0].0.clone(), ids[0].1.clone()));
        view.remote.multi = if ids.len() > 1 { ids } else { Vec::new() };
    };
    let start = |view: &mut LibraryView| {
        let requests = view.remote_pull.as_mut().unwrap().start();
        assert!(!requests.is_empty());
        for request in requests {
            view.send(Request::Transfer(request));
        }
    };
    let finished = |v: &LibraryView| v.remote_pull.as_ref().is_some_and(|d| d.finished());

    // One tag: the local tag waits for every blob.
    let (_target, store, mut view) = listed("remotes-pull");
    assert_eq!(
        row(&view, "test").local,
        Some(LocalState::NotPulled { same_as: None })
    );
    select(&mut view, &["test"]);
    view.begin_remote_pull(Mode::Pull);
    lab.registry.clear_log();
    lab.registry.hold_next("GET /v2/private/test/blobs/");
    start(&mut view);
    let registry = &lab.registry;
    wait_pumping(&mut view, || registry.held());
    assert!(matches!(
        store.tag_descriptor(&local("test")),
        Err(image_store::Error::TagNotFound(_))
    ));
    lab.registry.release();
    view.pump_until(finished);
    assert_eq!(
        store.tag_descriptor(&local("test")).unwrap().digest,
        source.digest
    );
    assert_eq!(view.status, Some(format!("Pulled {}", local("test"))));
    view.pump_until(|v| row(v, "test").local == Some(LocalState::InLibrary));
    assert!(
        !lab.registry
            .log()
            .iter()
            .any(|line| line.contains("_catalog")),
        "no new listing: {:?}",
        lab.registry.log()
    );

    // The copy's local name holds another image: Pull offers another tag
    // and never moves that one.
    pc_image::test_support::with_synthetic_firmware(|| {
        store.update_tag(&local("copy"), &source, None, &pc_image::PcProvider)
    })
    .unwrap();
    view.send(Request::Query(view.query.clone()));
    view.pump_until(|v| matches!(row(v, "copy").local, Some(LocalState::OtherImage { .. })));
    view.remote_pull = None;
    select(&mut view, &["copy"]);
    view.begin_remote_pull(Mode::Pull);
    let dialog = view.remote_pull.as_mut().unwrap();
    let another = format!("{host}/private/copy-new:1.0.0");
    assert_eq!(dialog.items[0].local_tag, another);
    dialog.items[0].local_tag = local("copy");
    assert!(dialog
        .problem()
        .unwrap()
        .ends_with("Pull moves no local tag but a release alias: choose another one."));
    assert!(dialog.start().is_empty());
    dialog.items[0].local_tag = another.clone();
    start(&mut view);
    view.pump_until(finished);
    assert_eq!(store.tag_descriptor(&another).unwrap().digest, copy.digest);
    assert_eq!(
        store.tag_descriptor(&local("copy")).unwrap().digest,
        source.digest,
        "the clashing local tag never moved"
    );

    // Several: the ones in the library or whose name holds another image
    // are left out.
    view.remote_pull = None;
    select(&mut view, &["test", "copy", "third"]);
    view.begin_remote_pull(Mode::Pull);
    let skipped: Vec<Option<String>> = view
        .remote_pull
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(|i| i.skip.clone())
        .collect();
    assert_eq!(
        skipped,
        [
            Some("already in the library".to_owned()),
            Some("its local tag names another image; pull it alone, under another tag".to_owned()),
            None
        ]
    );
    start(&mut view);
    view.pump_until(finished);
    assert_eq!(
        store.tag_descriptor(&local("third")).unwrap().digest,
        third.digest
    );
    assert_eq!(
        view.remote_pull
            .as_ref()
            .unwrap()
            .summary()
            .unwrap()
            .heading,
        "Pulled 1 tag."
    );

    // Cancel: the running pull stops at its next read, the waiting one never
    // asks the registry.
    let (_fresh, fresh, mut view) = listed("remotes-pull-cancel");
    select(&mut view, &["test", "third"]);
    view.begin_remote_pull(Mode::Pull);
    lab.registry.clear_log();
    lab.registry.hold_next("GET /v2/private/test/blobs/");
    start(&mut view);
    let registry = &lab.registry;
    wait_pumping(&mut view, || registry.held());
    view.remote_pull.as_mut().unwrap().cancel();
    lab.registry.release();
    view.pump_until(finished);
    let dialog = view.remote_pull.as_ref().unwrap();
    assert_eq!(dialog.stage(0), Stage::Cancelled);
    assert_eq!(dialog.stage(1), Stage::NotStarted);
    assert!(
        !lab.registry
            .log()
            .iter()
            .any(|line| line.contains("/v2/private/third/")),
        "{:?}",
        lab.registry.log()
    );
    for repository in ["test", "third"] {
        assert!(fresh.tag_descriptor(&local(repository)).is_err());
    }
    // Pulled again, both come, in the order chosen.
    view.remote_pull = None;
    view.begin_remote_pull(Mode::Pull);
    lab.registry.clear_log();
    start(&mut view);
    view.pump_until(finished);
    let manifests: Vec<String> = lab
        .registry
        .log()
        .into_iter()
        .filter(|line| line.starts_with("GET ") && line.contains("/manifests/"))
        .collect();
    let first = |repository: &str| manifests.iter().position(|l| l.contains(repository));
    assert!(
        first("private/test") < first("private/third"),
        "{manifests:?}"
    );
    assert_eq!(view.remote_pull.as_ref().unwrap().pulled().len(), 2);
}

/// The details pane of one tag and of several lays out in both skins and
/// both themes at the window widths the library is checked at: nothing
/// moves or grows from frame to frame, and no text leaves the window or is
/// cut at a side of its clip.
#[test]
fn the_details_pane_lays_out_in_every_look_without_growing() {
    for (classic, dark) in LOOKS {
        for width in [760.0, 1000.0, 1400.0, 2000.0] {
            for several in [false, true] {
                let what = format!("classic {classic}, dark {dark}, {width} px, several {several}");
                let mut rig = Rig::new();
                rig.listed();
                let open_at = rig
                    .registry(DEFAULT_REGISTRY)
                    .repositories
                    .iter_mut()
                    .find(|r| r.name == "unemu/open-at")
                    .unwrap();
                let rich = rich(mt::MACHINE, "Open AT 286", "unemu AT BIOS ROM");
                open_at.read[0].record = Ok(rich);
                open_at.newer_needs =
                    vec![("2.0.0".into(), "Needs unemupc 99.0.0 or newer".into())];
                rig.show(unemu(None));
                rig.view.remote.selected = Some((
                    DEFAULT_REGISTRY.into(),
                    "unemu/open-at".into(),
                    "1.2.0".into(),
                ));
                if several {
                    rig.view.remote.multi = vec![
                        ("unemu/open-at".into(), "1.2.0".into()),
                        ("unemu/clash".into(), "1.0.0".into()),
                        ("unemu/media/freedos".into(), "1.4".into()),
                    ];
                }
                rig.view.classic = classic;
                let size = egui::vec2(width, 900.0);
                let mut screen = Screen::new(classic, dark, size);
                // The pane is taller than its area: its scroll bar takes its
                // room over egui's animation time first.
                for _ in 0..30 {
                    screen.frame(&mut rig.view, Vec::new());
                }
                let first = screen.settled(&mut rig.view);
                let later = {
                    screen.frame(&mut rig.view, Vec::new());
                    screen.frame(&mut rig.view, Vec::new())
                };
                assert_eq!(first.texts, later.texts, "{what}: the texts moved");
                assert_eq!(first.widgets, later.widgets, "{what}: the widgets moved");
                let heading = if several {
                    "3 tags selected"
                } else {
                    "In your library"
                };
                let pane_top = first.text(heading).rect.top() - 40.0;
                let window = Rect::from_min_size(egui::Pos2::ZERO, size);
                for text in first.texts.iter().filter(|t| t.rect.top() >= pane_top) {
                    let visible = text.rect.intersect(text.clip);
                    if !visible.is_positive() {
                        continue;
                    }
                    assert!(
                        window.expand(0.5).contains_rect(visible),
                        "{what}: {:?} leaves the window",
                        text.text
                    );
                    assert!(
                        text.rect.left() >= text.clip.left() - 1.0
                            && text.rect.right() <= text.clip.right() + 1.0,
                        "{what}: {:?} is cut at a side ({:?} in {:?})",
                        text.text,
                        text.rect,
                        text.clip
                    );
                }
            }
        }
    }
}

/// A machine image of another family under the templates' node is muted
/// with the reason and can be neither pulled nor made into a machine.
#[test]
fn another_familys_image_is_muted_and_cannot_be_pulled() {
    let mut c64 = record(mt::MACHINE, "Commodore 64");
    c64.platform = Some("c64".into());
    let mut rig = Rig::new();
    let registry = rig.registry(DEFAULT_REGISTRY);
    registry.repositories = vec![repository(
        "unemu/c64",
        &["1.0.0"],
        vec![read(
            DEFAULT_REGISTRY,
            "unemu/c64",
            "1.0.0",
            c64,
            LocalState::NotPulled { same_as: None },
        )],
    )];
    registry.state = State::Listed;
    rig.show(unemu(Some(Group::Machines)));
    let mut screen = Screen::new(true, false, WIDE);
    let reason = "A c64 machine image: this release runs IBM PC compatibles";
    let shown = screen.settled(&mut rig.view);
    let rows = shown.rows();
    assert_eq!(rows[0].last().map(String::as_str), Some(reason));
    let muted = crate::shell::skin_visuals(true, false).weak_text_color();
    assert_eq!(shown.text("unemu/c64").colour, muted);
    screen.click(&mut rig.view, "unemu/c64");
    let shown = screen.settled(&mut rig.view);
    assert!(shown.shows(&format!("{reason}.")));
    assert!(!rig.view.can_pull_remote());
    for button in ["Pull…", "Pull and create machine…"] {
        screen.click(&mut rig.view, button);
        assert!(rig.view.remote_pull.is_none(), "{button}");
    }
}

/// Enter on a tag's row opens Pull… for it: egui clicks a focused row on
/// Enter, and the tag is selected and pulled; a repository of several tags
/// expands instead.
#[test]
fn enter_on_a_tag_row_opens_pull() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(None));
    let mut screen = Screen::new(true, false, WIDE);
    screen.click(&mut rig.view, "unemu/clash");
    screen.click(&mut rig.view, "unemu/open-at-games");
    assert!(rig.view.remote_pull.is_none());
    screen.key(&mut rig.view, egui::Key::Enter);
    let dialog = rig.view.remote_pull.as_ref().expect("Enter opens Pull…");
    assert_eq!(dialog.items.len(), 1);
    assert_eq!(dialog.items[0].name(), "unemu/open-at-games:1.2.0");
    assert_eq!(
        rig.view.remote.selection(),
        [("unemu/open-at-games".to_owned(), "1.2.0".to_owned())]
    );
}

/// A tag selected under one node is no longer the selection once another
/// node is shown, so Pull… never takes a tag the list does not show.
#[test]
fn another_node_drops_the_selection() {
    let mut rig = Rig::new();
    rig.listed();
    let mut file = None;
    rig.view
        .select_remote(unemu(Some(Group::Machines)), &mut file);
    let mut screen = Screen::new(true, false, WIDE);
    screen.click(&mut rig.view, "unemu/open-at-games");
    assert!(rig.view.can_pull_remote());
    rig.view
        .select_remote(unemu(Some(Group::Firmware)), &mut file);
    assert!(rig.view.remote.selection().is_empty());
    assert!(!rig.view.can_pull_remote());
    let shown = screen.settled(&mut rig.view);
    assert!(shown.has(
        "Select a tag for its details and Pull…. Ctrl (⌘ on macOS) adds a tag to the selection; Shift selects a range."
    ));
    // Showing the same node again keeps it.
    screen.click(&mut rig.view, "unemu/firmware/future-bios");
    rig.view
        .select_remote(unemu(Some(Group::Firmware)), &mut file);
    assert_eq!(rig.view.remote.selection().len(), 1);
}

/// Official templates are the machine images one name under `unemu/`; a
/// deeper repository there lists under the same node as a machine image.
#[test]
fn only_a_name_right_under_unemu_is_a_template() {
    assert_eq!(type_label(mt::MACHINE, "unemu/open-at"), "Machine template");
    assert_eq!(
        type_label(mt::MACHINE, "unemu/staging/open-at"),
        "Machine image"
    );
    assert_eq!(type_label(mt::MACHINE, "unemu"), "Machine image");
    assert_eq!(type_label(mt::MACHINE, "private/open-at"), "Machine image");
    assert_eq!(
        type_label(mt::MEDIA_SET, "unemu/freedos"),
        "Installation media"
    );
    let mut rig = Rig::new();
    rig.listed();
    rig.registry(DEFAULT_REGISTRY).repositories.push(repository(
        "unemu/staging/open-at",
        &["1.3.0"],
        vec![read(
            DEFAULT_REGISTRY,
            "unemu/staging/open-at",
            "1.3.0",
            record(mt::MACHINE, "Open AT 286, staged"),
            LocalState::NotPulled { same_as: None },
        )],
    ));
    rig.show(unemu(Some(Group::Machines)));
    let mut screen = Screen::new(true, false, WIDE);
    let rows = screen.settled(&mut rig.view).rows();
    let staged = rows
        .iter()
        .find(|r| r[0] == "unemu/staging/open-at")
        .expect("listed under the node");
    assert_eq!(staged[4], "Machine image");
}

/// A selected tag whose record is not read yet is listed and counted as
/// left out, the same in the pane and in the dialog.
#[test]
fn an_unread_tag_of_several_is_left_out_with_the_reason() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(None));
    rig.view.remote.selected = Some((
        DEFAULT_REGISTRY.into(),
        "unemu/open-at".into(),
        "1.0.0".into(),
    ));
    rig.view.remote.multi = vec![
        ("unemu/open-at".into(), "1.0.0".into()),
        ("unemu/open-at-games".into(), "1.2.0".into()),
    ];
    let mut screen = Screen::new(true, false, WIDE);
    let shown = screen.settled(&mut rig.view);
    assert!(shown.has("2 tags selected"));
    assert!(shown.shows("Not read yet"));
    assert!(shown.has_widget("Pull 1 tag…"));
    screen.click(&mut rig.view, "Pull 1 tag…");
    let dialog = rig.view.remote_pull.as_ref().expect("Pull… opens");
    let skipped: Vec<Option<&str>> = dialog.items.iter().map(|i| i.skip.as_deref()).collect();
    assert_eq!(skipped, [Some("it is not read yet"), None]);
    assert_eq!(dialog.go_label(), "Pull 1 tag");
}

/// Pull and insert… names what waits in a drive for the next Start already,
/// which the pulled set's first disk replaces.
#[test]
fn pull_and_insert_says_what_it_replaces() {
    use crate::library_view::machines::tests::row;
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(None));
    let mut games = row("games", "Games machine", "games", &[]);
    games.created.push(crate::created_media::CreatedInsert {
        drive: "A:".into(),
        title: "Saved games".into(),
        disks: Vec::new(),
        journal: false,
        first: None,
    });
    rig.view.machines = Arc::new(vec![games]);
    let mut screen = Screen::new(true, false, WIDE);
    screen.click(&mut rig.view, "unemu/media/freedos");
    screen.click(&mut rig.view, "Pull and insert…");
    let dialog = rig
        .view
        .remote_pull
        .as_ref()
        .expect("Pull and insert… opens");
    assert_eq!(
        dialog.drives.iter().map(|d| d.label()).collect::<Vec<_>>(),
        ["Games machine — A: 1.44M — replaces Saved games"]
    );
}

/// Contents… keeps inside a window at its narrowest width, 760 px, with a
/// gutter at its sides, and cuts no text.
#[test]
fn contents_fits_a_narrow_window() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(None));
    rig.view.remote.contents = Some((
        DEFAULT_REGISTRY.into(),
        "unemu/open-at".into(),
        "1.2.0".into(),
    ));
    let open_at = rig
        .registry(DEFAULT_REGISTRY)
        .repositories
        .iter_mut()
        .find(|r| r.name == "unemu/open-at")
        .unwrap();
    open_at.read[0].record = Ok(rich(mt::MACHINE, "Open AT 286", "unemu AT BIOS ROM"));
    for (classic, dark) in LOOKS {
        let size = egui::vec2(760.0, 500.0);
        let ctx = egui::Context::default();
        crate::shell::configure_context(&ctx);
        ctx.set_visuals(crate::shell::skin_visuals(classic, dark));
        let mut shown = None;
        for frame in 0..4 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                    time: Some(f64::from(frame) / 60.0),
                    ..Default::default()
                },
                |ui| {
                    rig.view.modals(ui.ctx(), None, Vec::new(), classic, false);
                },
            );
            output.textures_delta.clear();
            shown = Some(Shown::of(&output));
        }
        let shown = shown.unwrap();
        assert!(shown.has("Contents — unemu/open-at:1.2.0"));
        // The dialog keeps a gutter inside the window, as at its wide size.
        let window = Rect::from_min_size(egui::Pos2::ZERO, size).shrink(8.0);
        for text in &shown.texts {
            let visible = text.rect.intersect(text.clip);
            if visible.is_positive() {
                assert!(
                    window.expand(0.5).contains_rect(visible),
                    "classic {classic}, dark {dark}: {:?} leaves the window's gutter ({:?})",
                    text.text,
                    text.rect
                );
                assert!(
                    text.rect.left() >= text.clip.left() - 1.0
                        && text.rect.right() <= text.clip.right() + 1.0,
                    "classic {classic}, dark {dark}: {:?} is cut at a side ({:?} in {:?})",
                    text.text,
                    text.rect,
                    text.clip
                );
            }
        }
    }
}

/// An expanded repository lists a set's or package's tags by release: the
/// alias of each release first, as the name that stands for its newest
/// revision, then its revisions highest first; a machine image's by
/// version, then `latest`, then its other tags.
#[test]
fn expanded_tags_list_by_release_with_the_alias_before_its_revisions() {
    let listed = |name: &str, tags: &[&str], artifact_type: &str| {
        let shown =
            representative(&tags.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>()).unwrap();
        let row = repository(
            name,
            tags,
            vec![read(
                DEFAULT_REGISTRY,
                name,
                &shown,
                record(artifact_type, "x"),
                LocalState::NotPulled { same_as: None },
            )],
        );
        newest_first(&row)
    };
    assert_eq!(
        listed(
            "unemu/media/ibm-pc/freedos",
            &[
                "1.4-720k-r1",
                "1.4-r1",
                "1.4",
                "1.4-720k",
                "1.4-r2",
                "1.4-1.2m-r1"
            ],
            mt::MEDIA_SET
        ),
        [
            "1.4",
            "1.4-r2",
            "1.4-r1",
            "1.4-1.2m-r1",
            "1.4-720k",
            "1.4-720k-r1"
        ]
    );
    assert_eq!(
        listed(
            "private/media/nes/contra",
            &["usa-rev1-r1", "usa-r2", "usa", "usa-r1", "japan-r1"],
            mt::MEDIA_SET
        ),
        ["japan-r1", "usa", "usa-r2", "usa-r1", "usa-rev1-r1"]
    );
    assert_eq!(
        listed(
            "private/firmware/ibm-pc/ibm-5170-bios-1984-01-10",
            &["4x16k-r1", "r1", "2x32k-r1", "r2"],
            mt::FIRMWARE
        ),
        ["r2", "r1", "2x32k-r1", "4x16k-r1"]
    );
    assert_eq!(
        listed(
            "unemu/open-at",
            &["scratch", "1.0.0", "latest", "1.2.0"],
            mt::MACHINE
        ),
        ["1.2.0", "1.0.0", "latest", "scratch"]
    );
}
