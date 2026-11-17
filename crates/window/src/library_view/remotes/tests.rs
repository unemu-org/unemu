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
        size: 3 * 1024 * 1024 + 512 * 1024,
        user_supplied: false,
        annotations: Default::default(),
        details: None,
    }
}

fn read(host: &str, repository: &str, tag: &str, record: TagRecord, local: LocalState) -> TagRow {
    TagRow {
        tag: tag.into(),
        local_tag: local_tag_of(host, repository, tag),
        record: Ok(record),
        local: Some(local),
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
        screen.click(&mut rig.view, action_label(action));
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

/// The library's details pane says what a remote node offers yet, and a
/// tag's selection never becomes the library's.
#[test]
fn a_remote_node_keeps_the_library_selection_apart() {
    let mut rig = Rig::new();
    rig.listed();
    rig.show(unemu(None));
    let mut screen = Screen::new(true, false, WIDE);
    screen.click(&mut rig.view, "unemu/clash");
    assert!(rig.view.selected.is_none() && rig.view.machine.is_none());
    let shown = screen.settled(&mut rig.view);
    assert!(shown.has(
        "Details and Pull… of remote tags are not built yet. Image › Pull… pulls a tag by its reference."
    ));
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
