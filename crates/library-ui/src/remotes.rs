// SPDX-License-Identifier: AGPL-3.0-or-later
//! The library's Remotes group, drawn from the headless
//! [`library_model::remote_browse`] model as the approved remotes mock-up,
//! revision 2 (`backlog/docs/design/mockups/remotes.html`), draws it;
//! machine-images §3 (local tags) and §5.1 (registries).
//!
//! The sidebar lists Remotes under the Library group: one node per registry
//! of Tools › Preferences › Registries, the default registry first and the
//! family default registry listed even when preferences do not name it.
//! Under a registry are its namespaces, those its entry names or, for a
//! whole-registry entry, the first path components the catalog returns; a
//! private namespace shows a lock until a login lists it. Under a namespace
//! is one group per artifact type that has repositories: Machine images,
//! Installation media, Firmware. Nothing contacts a registry until its node
//! is opened or selected.
//!
//! A node's list is the library's grid with the mock-up's columns: Name,
//! Source, Title, Latest, Type, Licence, Size and Local state. A repository
//! row stands for the tag the model chose (the newest release, else the
//! newest set tag or firmware revision this release runs); a repository of
//! several tags expands into them, newest first. A tag this release cannot
//! run is muted and says "Needs unemupc X or newer" (machine-images §5.6); a
//! repository is muted only when none of its versions runs here, and
//! otherwise its row names the newer versions that need a newer release.
//! Above the rows a strip says a listing runs, stopped, or answers no more;
//! instead of the rows a message says why nothing is listed, with the action
//! the model offers. Next 50 › reads the catalog's next page; F5 and the
//! toolbar's Refresh list the registry again. Every request runs on the
//! library worker, so the window never waits for a registry.
//!
//! A click selects a tag; Ctrl (⌘ on macOS) adds or removes one and Shift
//! selects the rows between the last click and this one, so Pull… takes
//! several. Enter on a tag's row opens Pull…. The details pane of the
//! selection is drawn by `details`; File › Get templates… and the empty
//! library's button show Remotes › the default registry › `unemu/` ›
//! Machine images.
//!
//! Remotes draws without the library frame around it: the frame lends it
//! what it reads of the library for one call ([`RemotesContext`]) and
//! applies, in their order, what Remotes asks of the library
//! ([`RemotesAction`]): the browsing requests to send, the node to show
//! instead of a collection, a tag to show in the library, New machine from
//! image…, Pull…, Log in… and the media picker.

use crate::table::{fit_columns, sort_header, table_width, Cell, Columns, BADGE_PADDING};
use egui::{AtomExt, Color32, Rect, RichText, Sense, Stroke, Vec2};
use image_registry::{reference::OFFICIAL_NAMESPACE, DEFAULT_REGISTRY};
use image_store::{media_type as mt, Descriptor, Digest};
use library_model::{
    listing::human_size,
    registry_transfer::Accounts,
    remote_browse::{
        Action, BrowseEvent, BrowseRequest, End, Failure, LocalState, Message, RegistryView,
        Remotes, RepositoryRow, State, TagRecord, TagRow,
    },
    remote_pull::{Drive, Mode},
    PlatformFacts,
};
use std::collections::BTreeSet;
use ui_kit::{pane, shell_icons::Icon};

/// An artifact-type group under a namespace, as the mock-up's tree groups
/// repositories by their manifest's artifact type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Machines,
    Media,
    Firmware,
}

impl Group {
    const ALL: [Self; 3] = [Self::Machines, Self::Media, Self::Firmware];

    fn of(artifact_type: &str) -> Option<Self> {
        match artifact_type {
            mt::MACHINE => Some(Self::Machines),
            mt::MEDIA_SET => Some(Self::Media),
            mt::FIRMWARE => Some(Self::Firmware),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Machines => "Machine images",
            Self::Media => "Installation media",
            Self::Firmware => "Firmware",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Machines => Icon::Image,
            Self::Media => Icon::Media,
            Self::Firmware => Icon::Firmware,
        }
    }
}

/// A node of the Remotes tree: a registry, one of its namespaces, or one
/// artifact-type group of a namespace.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Node {
    pub host: String,
    /// `None` for the registry's own node; else the configured namespace path
    /// (or a catalog first component), `None` inside for top-level repositories.
    pub namespace: Option<Option<String>>,
    pub group: Option<Group>,
}

impl Node {
    pub fn registry(host: &str) -> Self {
        Self {
            host: host.into(),
            namespace: None,
            group: None,
        }
    }

    /// The repository is listed under this node.
    fn holds(&self, view: &RegistryView, repository: &RepositoryRow) -> bool {
        let namespace = match &self.namespace {
            Some(namespace) if view.entry.namespaces.is_empty() => {
                first_component(&repository.name) == namespace.as_deref()
            }
            Some(namespace) => {
                matches!(view.entry.scope(&repository.name), image_registry::config::Scope::Namespace { name, .. } if Some(name.as_str()) == namespace.as_deref())
            }
            None => true,
        };
        namespace && (self.group.is_none() || group_of(repository) == self.group)
    }
}

/// The first path component of a repository with more than one.
fn first_component(repository: &str) -> Option<&str> {
    repository.split_once('/').map(|(first, _)| first)
}

/// The artifact type of the tag a repository row stands for, once read.
fn group_of(repository: &RepositoryRow) -> Option<Group> {
    repository
        .representative_row()?
        .record
        .as_ref()
        .ok()
        .and_then(|record| Group::of(&record.artifact_type))
}

/// The Remotes group's state beside the library's own: the model, the node
/// the list shows, what the tree has open and what the list has expanded
/// and selected.
pub struct RemotesView {
    /// `None` until the shell hands over the logins of Preferences ›
    /// Registries.
    pub model: Option<Remotes>,
    /// The node the list shows instead of a library collection.
    pub node: Option<Node>,
    /// Open registries by host and open namespaces by their host and name
    /// (`namespace_key`).
    pub open: BTreeSet<String>,
    /// Expanded repositories by host and name.
    pub expanded: BTreeSet<(String, String)>,
    /// The selected tag, the one the details pane shows when one is:
    /// host, repository and tag.
    pub selected: Option<(String, String, String)>,
    /// Several tags of the shown registry selected with Ctrl or Shift, by
    /// repository and tag in the order they were added; empty, or one tag
    /// alone, when `selected` stands for the selection.
    pub multi: Vec<(String, String)>,
    /// Where Shift selects from: the row clicked last, by repository and tag.
    anchor: Option<(String, String)>,
    /// Contents… while it is open: host, repository and tag.
    pub contents: Option<(String, String, String)>,
    sort: usize,
    descending: bool,
    /// Log in… opened Preferences on this registry; it lists again once
    /// they close.
    login: Option<String>,
    /// The tree's lines, until the registries or what is open change.
    tree: Option<TreeCache>,
    /// The shown node's sorted lines, until the registries, the node, the
    /// sort or what is expanded change.
    list: Option<ListCache>,
}

struct TreeCache {
    revision: u64,
    open: BTreeSet<String>,
    lines: Vec<TreeLine>,
}

struct ListCache {
    revision: u64,
    node: Node,
    sort: (usize, bool),
    expanded: BTreeSet<(String, String)>,
    lines: Vec<Line>,
}

impl RemotesView {
    /// A click on a tag's row of `host`: alone, it selects the tag; with
    /// Ctrl (⌘ on macOS) it adds the tag to the selection or takes it out;
    /// with Shift it selects every row between the last click and this one.
    fn pick(&mut self, host: &str, repository: String, tag: String, modifiers: egui::Modifiers) {
        let clicked = (repository, tag);
        let current = self
            .selected
            .as_ref()
            .filter(|(h, _, _)| h == host)
            .map(|(_, r, t)| (r.clone(), t.clone()));
        if modifiers.command {
            if self.multi.len() < 2 {
                self.multi = current.into_iter().collect();
            }
            match self.multi.iter().position(|id| *id == clicked) {
                Some(index) => {
                    self.multi.remove(index);
                }
                None => self.multi.push(clicked.clone()),
            }
        } else if let (true, Some(anchor)) = (modifiers.shift, self.anchor.clone()) {
            let rows = self.shown_rows();
            let at = |id: &(String, String)| rows.iter().position(|row| row == id);
            self.multi = match (at(&anchor), at(&clicked)) {
                (Some(a), Some(b)) => {
                    let mut range: Vec<(String, String)> = Vec::new();
                    for row in &rows[a.min(b)..=a.max(b)] {
                        if !range.contains(row) {
                            range.push(row.clone());
                        }
                    }
                    range
                }
                _ => vec![clicked.clone()],
            };
        } else {
            self.multi.clear();
        }
        if !modifiers.shift {
            self.anchor = Some(clicked.clone());
        }
        let focus = match self.multi.as_slice() {
            [only] => only.clone(),
            _ => clicked,
        };
        if self.multi.len() < 2 {
            self.multi.clear();
        }
        self.selected = Some((host.into(), focus.0, focus.1));
    }

    /// The shown list's rows from the top, each by its repository and the
    /// tag it stands for, as the list last drew them.
    fn shown_rows(&self) -> Vec<(String, String)> {
        let (Some(model), Some(cache)) = (&self.model, &self.list) else {
            return Vec::new();
        };
        let Some(view) = model.registry(&cache.node.host) else {
            return Vec::new();
        };
        cache
            .lines
            .iter()
            .filter_map(|line| {
                let repository = view.repositories.get(line.repository)?;
                let tag = line
                    .tag
                    .clone()
                    .or_else(|| repository.representative.clone())?;
                Some((repository.name.clone(), tag))
            })
            .collect()
    }

    /// The selected tags of the shown registry, by repository and tag: the
    /// several Ctrl or Shift chose, else the one selected.
    pub fn selection(&self) -> Vec<(String, String)> {
        let Some(node) = &self.node else {
            return Vec::new();
        };
        if self.multi.len() > 1 {
            return self.multi.clone();
        }
        self.selected
            .iter()
            .filter(|(host, _, _)| *host == node.host)
            .map(|(_, repository, tag)| (repository.clone(), tag.clone()))
            .collect()
    }

    pub fn new() -> Self {
        Self {
            model: None,
            node: None,
            open: BTreeSet::new(),
            expanded: BTreeSet::new(),
            selected: None,
            multi: Vec::new(),
            anchor: None,
            contents: None,
            // The mock-up's list opens sorted by type.
            sort: TYPE_COLUMN,
            descending: false,
            login: None,
            tree: None,
            list: None,
        }
    }

    /// Collapse the repositories of `host`, or of every registry, whose tags
    /// are not all read: their reads stopped, or a new listing replaces
    /// their rows, so nothing would ever fill them in.
    fn collapse_unread(&mut self, host: Option<&str>) {
        let Some(model) = &self.model else {
            return;
        };
        self.expanded.retain(|(h, name)| {
            if host.is_some_and(|host| host != h) {
                return true;
            }
            model
                .registry(h)
                .and_then(|view| view.repository(name))
                .is_some_and(|r| r.tags.iter().all(|tag| r.row(tag).is_some()))
        });
    }
}

impl Default for RemotesView {
    fn default() -> Self {
        Self::new()
    }
}

fn namespace_key(host: &str, namespace: Option<&str>) -> String {
    format!("{host}/{}", namespace.unwrap_or_default())
}

/// The Remotes list's columns, as the mock-up's list draws them.
const COLUMNS: Columns<8> = Columns {
    labels: [
        "Name",
        "Source",
        "Title",
        "Latest",
        "Type",
        "Licence",
        "Size",
        "Local state",
    ],
    widths: [200.0, 140.0, 160.0, 78.0, 128.0, 100.0, 78.0, 170.0],
    cells: [
        Cell::Text,
        Cell::Text,
        Cell::Text,
        Cell::Mono,
        Cell::Text,
        Cell::Badge,
        Cell::Number,
        Cell::Text,
    ],
    samples: [
        "",
        "",
        "",
        "",
        "Machine template",
        "user-supplied",
        "999.9 MiB",
        "",
    ],
};
const TYPE_COLUMN: usize = 4;
/// A list row's height, as the library's rows.
const ROW: f32 = 20.0;
/// The mock-up's expand box (`.tw`): 11 px square.
const TWISTY: f32 = 11.0;
/// The room an expand box and its gap take before a row's icon.
const TWISTY_ROOM: f32 = 15.0;
/// The room the icon and its gap take before a row's name.
const ICON_ROOM: f32 = 20.0;
/// A tag row's set-in, as the mock-up sets in a child row's name.
const TAG_INDENT: f32 = 28.0;
/// The set-in of each tree level.
const TREE_INDENT: f32 = 14.0;
/// The mock-up's 11 px meta text: counts and registry marks in the tree.
const META_SIZE: f32 = 11.0;

/// What a tree line shows after its name.
pub enum Tail {
    /// The registry's kind or its state, muted or as a warning.
    Mark(&'static str, bool),
    /// A listing runs.
    Spinner,
    /// A private namespace before a login lists it.
    Lock,
    Count(usize),
}

/// One line of the Remotes tree.
pub struct TreeLine {
    depth: u8,
    /// Open or not, and its key in [`RemotesView::open`]; `None` keeps the
    /// expand box's room.
    twisty: Option<(bool, String)>,
    icon: Icon,
    pub label: String,
    /// The line's name for a screen reader: the label with the registry and
    /// namespace it lies in, so the same namespace on two registries reads
    /// apart.
    name: String,
    pub tail: Vec<Tail>,
    node: Node,
}

impl TreeLine {
    /// The node's accessible label: its name, then its mark, lock, spinner
    /// or count in words.
    fn spoken(&self) -> String {
        let mut spoken = self.name.clone();
        for tail in &self.tail {
            spoken.push(' ');
            match tail {
                Tail::Mark(text, _) => spoken.push_str(text),
                Tail::Spinner => spoken.push_str("listing"),
                Tail::Lock => spoken.push_str("locked until you log in"),
                Tail::Count(count) => spoken.push_str(&format!("({count})")),
            }
        }
        spoken
    }
}

/// The registry's kind, as the mock-up marks it beside its host.
fn kind(view: &RegistryView) -> &'static str {
    if view.host == DEFAULT_REGISTRY {
        "official, public"
    } else if view.entry.private || view.entry.namespaces.iter().any(|n| n.private) {
        "private"
    } else {
        "public"
    }
}

fn registry_tail(view: &RegistryView) -> Tail {
    match &view.state {
        State::Listing => Tail::Spinner,
        State::Unreachable { .. } => Tail::Mark("unreachable", true),
        State::ServerError { .. } | State::Failed(_) => Tail::Mark("error", true),
        _ => Tail::Mark(kind(view), false),
    }
}

/// A private namespace stays locked until a login lists it: while no
/// account is stored for the registry, or while it asks for a login.
fn locked(view: &RegistryView) -> bool {
    view.entry.user.is_none() || matches!(view.state, State::LoginRequired { .. })
}

/// Whether the registry's counts mean anything: it listed, or lists now.
fn listed(view: &RegistryView) -> bool {
    !view.repositories.is_empty()
        || matches!(view.state, State::Listed | State::Listing | State::Stopped)
}

fn tree_lines(model: &Remotes, open: &BTreeSet<String>) -> Vec<TreeLine> {
    let mut lines = Vec::new();
    for view in &model.registries {
        let host = &view.host;
        let registry_open = open.contains(host);
        lines.push(TreeLine {
            depth: 0,
            twisty: Some((registry_open, host.clone())),
            icon: Icon::Registry,
            label: host.clone(),
            name: host.clone(),
            tail: vec![registry_tail(view)],
            node: Node::registry(host),
        });
        if !registry_open {
            continue;
        }
        for namespace in view.namespaces() {
            let name = namespace.name.as_deref();
            let lock = namespace.private && locked(view);
            let groups: Vec<(Group, usize)> = Group::ALL
                .into_iter()
                .map(|group| {
                    let count = namespace
                        .repositories
                        .iter()
                        .filter(|r| group_of(r) == Some(group))
                        .count();
                    (group, count)
                })
                .filter(|(_, count)| *count > 0)
                .collect();
            let key = namespace_key(host, name);
            let namespace_open = open.contains(&key);
            let mut tail = Vec::new();
            if lock {
                tail.push(Tail::Lock);
            } else if listed(view) {
                tail.push(Tail::Count(namespace.repositories.len()));
            }
            let node = Node {
                host: host.clone(),
                namespace: Some(namespace.name.clone()),
                group: None,
            };
            let label = name.map_or_else(|| "(top level)".into(), |name| format!("{name}/"));
            let spoken = format!("{label} on {host}");
            lines.push(TreeLine {
                depth: 1,
                twisty: (!groups.is_empty()).then_some((namespace_open, key)),
                icon: Icon::Namespace,
                label,
                name: spoken.clone(),
                tail,
                node: node.clone(),
            });
            if !namespace_open {
                continue;
            }
            for (group, count) in groups {
                lines.push(TreeLine {
                    depth: 2,
                    twisty: None,
                    icon: group.icon(),
                    label: group.label().into(),
                    name: format!("{} in {spoken}", group.label()),
                    tail: vec![Tail::Count(count)],
                    node: Node {
                        group: Some(group),
                        ..node.clone()
                    },
                });
            }
        }
    }
    lines
}

/// The mock-up's expand box: a field-coloured square in the shadow colour
/// with − when open and + when closed, drawn as lines.
fn paint_twisty(ui: &egui::Ui, rect: Rect, open: bool, ink: Color32) {
    let painter = ui.painter();
    let visuals = ui.visuals();
    painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, pane::shadow_colour(visuals)),
        egui::StrokeKind::Inside,
    );
    let centre = rect.center();
    let stroke = Stroke::new(1.0, ink);
    painter.line_segment(
        [centre - Vec2::new(2.5, 0.0), centre + Vec2::new(2.5, 0.0)],
        stroke,
    );
    if !open {
        painter.line_segment(
            [centre - Vec2::new(0.0, 2.5), centre + Vec2::new(0.0, 2.5)],
            stroke,
        );
    }
}

/// An expand box of its own, named for its line.
fn twisty(ui: &mut egui::Ui, open: bool, name: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(TWISTY), Sense::click());
    let label = format!("{} {name}", if open { "Collapse" } else { "Expand" });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &label));
    let ink = ui.style().interact(&response).text_color();
    paint_twisty(ui, rect, open, ink);
    response
}

/// One tree line: its set-in, the expand box or its room 2 px before the
/// node, then the node as the Library group draws its collections, with the
/// mark, lock or count in the mock-up's meta size after the name, muted, or
/// in the selection's text colour on a selected node. A name too long for
/// the pane ends in an ellipsis, as the mock-up's do, and the node's tooltip
/// names it whole. Returns whether the box and whether the node were
/// clicked.
fn tree_line(ui: &mut egui::Ui, line: &TreeLine, current: bool) -> (bool, bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if line.depth > 0 {
            ui.add_space(f32::from(line.depth) * TREE_INDENT - 2.0);
        }
        let toggled = match &line.twisty {
            Some((open, _)) => twisty(ui, *open, &line.name).clicked(),
            None => {
                ui.add_space(TWISTY);
                false
            }
        };
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        let mut atoms = egui::Atoms::new((
            ui_kit::shell_icons::atom(line.icon),
            RichText::new(&line.label).atom_shrink(true),
        ));
        let visuals = ui.visuals();
        let (muted, warn) = if current {
            let ink = visuals.selection.stroke.color;
            (ink, ink)
        } else {
            (visuals.weak_text_color(), visuals.warn_fg_color)
        };
        for tail in &line.tail {
            match tail {
                Tail::Mark(text, warning) => {
                    let ink = if *warning { warn } else { muted };
                    atoms.push_right(RichText::new(*text).size(META_SIZE).color(ink));
                }
                Tail::Count(count) => {
                    atoms.push_right(
                        RichText::new(format!("({count})"))
                            .size(META_SIZE)
                            .color(muted),
                    );
                }
                Tail::Spinner => atoms.push_right(egui::Atom::paint(
                    Vec2::splat(12.0),
                    |ui: &egui::Ui, args: egui::AtomPaintArgs| {
                        egui::Spinner::new()
                            .color(args.fallback_text_color)
                            .paint_at(ui, args.rect);
                    },
                )),
                Tail::Lock => atoms.push_right(egui::Atom::paint(
                    Vec2::splat(16.0),
                    move |ui: &egui::Ui, args: egui::AtomPaintArgs| {
                        // The mock-up's lock takes the warning colour, and
                        // the selection's text colour on a selected node.
                        let ink = if current {
                            args.fallback_text_color
                        } else {
                            ui.visuals().warn_fg_color
                        };
                        ui_kit::shell_icons::paint(
                            ui.painter(),
                            args.rect,
                            Icon::Lock,
                            ink,
                            ui.visuals(),
                        );
                    },
                )),
            }
        }
        let response = ui.selectable_label(current, atoms);
        let spoken = line.spoken();
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), current, &spoken)
        });
        let picked = response.on_hover_text(&line.label).clicked();
        (toggled, picked)
    })
    .inner
}

/// How a part of a cell's text reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Plain,
    Muted,
    Good,
    Warn,
    /// Code style: a tag or a digest.
    Code,
}

type Part = (String, Tone);

/// A list row's cells and what its name cell draws before the name.
struct RowCells {
    reference: String,
    parts: [Vec<Part>; 8],
    /// The expand box of a repository of several tags, open or not.
    twisty: Option<bool>,
    icon: Option<Icon>,
    /// A tag of an expanded repository, set in under it.
    tag: bool,
    /// A tag this release cannot run: the whole row is muted.
    muted: bool,
}

/// What a click on a row asked for.
#[derive(Default)]
struct RowAnswer {
    clicked: bool,
    double: bool,
    toggled: bool,
    /// Enter on the focused row.
    enter: bool,
}

/// Paints `parts` into `rect`, as the library list's cells: left-aligned,
/// numbers right-aligned, a badge in its outline, truncated to the column.
fn paint_parts(
    ui: &egui::Ui,
    rect: Rect,
    parts: &[Part],
    kind: Cell,
    selected: Option<Color32>,
    muted: bool,
) {
    if parts.iter().all(|(text, _)| text.is_empty()) || !ui.is_rect_visible(rect) {
        return;
    }
    let visuals = ui.visuals();
    let ink = |tone: Tone| {
        selected.unwrap_or(match tone {
            Tone::Plain | Tone::Code if muted => visuals.weak_text_color(),
            Tone::Plain | Tone::Code => visuals.text_color(),
            Tone::Muted => visuals.weak_text_color(),
            Tone::Good => pane::good_colour(visuals),
            Tone::Warn => visuals.warn_fg_color,
        })
    };
    let mut job = egui::text::LayoutJob::default();
    for (text, tone) in parts {
        let style = if *tone == Tone::Code || kind == Cell::Mono {
            egui::TextStyle::Monospace
        } else {
            egui::TextStyle::Body
        };
        job.append(
            text,
            0.0,
            egui::TextFormat::simple(style.resolve(ui.style()), ink(*tone)),
        );
    }
    let inner = if kind == Cell::Badge {
        rect.width() - BADGE_PADDING
    } else {
        rect.width()
    };
    job.wrap = egui::text::TextWrapping::truncate_at_width(inner.max(0.0));
    let galley = ui.painter().layout_job(job);
    let size = galley.size();
    let left = match kind {
        Cell::Number => rect.right() - size.x,
        Cell::Badge => rect.left() + BADGE_PADDING / 2.0,
        _ => rect.left(),
    };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let at = egui::pos2(left, rect.center().y - size.y / 2.0);
    if kind == Cell::Badge {
        painter.rect_stroke(
            Rect::from_min_size(at, size).expand2(Vec2::new(BADGE_PADDING / 2.0 - 1.0, 0.0)),
            0.0,
            Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    painter.galley(at, galley, visuals.text_color());
}

/// One list row, as the library list draws its rows: the selection or the
/// stripe across it, each cell a clickable label of its text with the text
/// as its tooltip. The name cell draws the expand box, which answers a
/// click of its own, and the row's icon.
fn draw_row(
    ui: &mut egui::Ui,
    row: &RowCells,
    widths: &[f32; 8],
    selected: bool,
    striped: bool,
    id: egui::Id,
) -> RowAnswer {
    let fill = if selected {
        ui.visuals().selection.bg_fill
    } else if striped {
        ui.visuals().faint_bg_color
    } else {
        Color32::TRANSPARENT
    };
    let selected_ink = selected.then(|| ui.visuals().selection.stroke.color);
    let mut answer = RowAnswer::default();
    egui::Frame::new().fill(fill).show(ui, |ui| {
        ui.horizontal(|ui| {
            for (column, width) in widths.iter().copied().enumerate() {
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(width, ROW), Sense::click());
                let text: String = row.parts[column].iter().map(|(t, _)| t.as_str()).collect();
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), &text)
                });
                let mut room = rect.shrink2(Vec2::new(2.0, 0.0));
                if column == 0 {
                    if row.tag {
                        room.min.x += TAG_INDENT;
                    } else {
                        if let Some(open) = row.twisty {
                            let boxed = Rect::from_min_size(
                                egui::pos2(room.left() + 2.0, rect.center().y - TWISTY / 2.0),
                                Vec2::splat(TWISTY),
                            );
                            let toggle = ui.interact(boxed, id.with("twisty"), Sense::click());
                            let label =
                                format!("{} {text}", if open { "Collapse" } else { "Expand" });
                            toggle.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Button, true, &label)
                            });
                            let ink = selected_ink.unwrap_or_else(|| ui.visuals().text_color());
                            paint_twisty(ui, boxed, open, ink);
                            answer.toggled |= toggle.clicked();
                        }
                        room.min.x += TWISTY_ROOM;
                    }
                    if let Some(icon) = row.icon {
                        let at = Rect::from_min_size(
                            egui::pos2(room.left(), rect.center().y - 8.0),
                            Vec2::splat(16.0),
                        );
                        let ink = selected_ink.unwrap_or_else(|| {
                            if row.muted {
                                ui.visuals().weak_text_color()
                            } else {
                                ui.visuals().text_color()
                            }
                        });
                        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                        ui_kit::shell_icons::paint(&painter, at, icon, ink, ui.visuals());
                        room.min.x += ICON_ROOM;
                    }
                }
                paint_parts(
                    ui,
                    room,
                    &row.parts[column],
                    COLUMNS.cells[column],
                    selected_ink,
                    row.muted,
                );
                if response.clicked() {
                    // Arrow keys move on from the row that was clicked.
                    response.request_focus();
                }
                answer.enter |=
                    response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                answer.clicked |= response.clicked();
                answer.double |= response.double_clicked();
                if !text.is_empty() {
                    response.on_hover_text(format!("{}\n{text}", row.reference));
                }
            }
        });
    });
    answer
}

/// Whether a repository holds official templates: one name right under
/// `unemu/`, such as `unemu/open-at`, as Get templates always named them.
/// Deeper repositories under `unemu/` (`unemu/staging/…`) list under the
/// same node as machine images, not templates.
fn template(repository: &str) -> bool {
    repository
        .strip_prefix(OFFICIAL_NAMESPACE)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|name| !name.is_empty() && !name.contains('/'))
}

/// What the Type column says of an artifact type: a machine image of an
/// official template repository is a template, as the mock-up decides.
fn type_label(artifact_type: &str, repository: &str) -> &'static str {
    match Group::of(artifact_type) {
        Some(Group::Machines) if template(repository) => "Machine template",
        Some(Group::Machines) => "Machine image",
        Some(Group::Media) => "Installation media",
        Some(Group::Firmware) => "Firmware package",
        None => "Other artifact",
    }
}

/// The Local state cell of a read tag, and whether the row is muted.
fn state_parts(row: &TagRow, facts: &PlatformFacts) -> (Vec<Part>, bool) {
    let record = match &row.record {
        Ok(record) => record,
        Err(reason) => return (vec![(format!("⚠ {reason}"), Tone::Warn)], false),
    };
    if let Some(reason) = record.not_runnable(facts) {
        return (vec![(reason, Tone::Warn)], true);
    }
    let parts = match &row.local {
        Some(LocalState::InLibrary) => vec![("In library".into(), Tone::Good)],
        Some(LocalState::NewerAvailable { have }) => {
            let version = have.rsplit_once(':').map_or(have.as_str(), |(_, tag)| tag);
            vec![
                ("Newer available".into(), Tone::Warn),
                (format!(" · you have {version}"), Tone::Muted),
            ]
        }
        // Across releases, the local release's version as its record says it.
        Some(LocalState::NewerRelease { version, .. }) => vec![
            ("Newer available".into(), Tone::Warn),
            (format!(" · you have {version}"), Tone::Muted),
        ],
        Some(LocalState::LocalNewer { have }) => {
            let version = have.rsplit_once(':').map_or(have.as_str(), |(_, tag)| tag);
            vec![(format!("Local is newer: {version}"), Tone::Muted)]
        }
        Some(LocalState::OtherImage { .. }) => {
            vec![("⚠ Local tag names another image".into(), Tone::Warn)]
        }
        Some(LocalState::NotPulled { same_as: Some(tag) }) => vec![
            ("Not pulled".into(), Tone::Muted),
            (" · same image as ".into(), Tone::Muted),
            (tag.clone(), Tone::Code),
        ],
        Some(LocalState::NotPulled { same_as: None }) | None => {
            vec![("Not pulled".into(), Tone::Muted)]
        }
        // A tag pushed from a local tag of another name: pushing never
        // renames, so the image is here under that tag.
        Some(LocalState::PushedFromHere { tag }) => vec![
            ("Pushed from here".into(), Tone::Good),
            (" · as ".into(), Tone::Muted),
            (tag.clone(), Tone::Code),
        ],
    };
    (parts, false)
}

/// The Licence badge in the library's colours: open in the good colour,
/// user-supplied as a warning.
fn licence(record: &TagRecord) -> Vec<Part> {
    vec![if record.user_supplied {
        ("user-supplied".into(), Tone::Warn)
    } else {
        ("open".into(), Tone::Good)
    }]
}

/// A repository's row: its name with the tag count, and the cells of the tag
/// it stands for.
fn repository_row(
    host: &str,
    repository: &RepositoryRow,
    open: bool,
    facts: &PlatformFacts,
    namespace: Option<&str>,
) -> RowCells {
    let multi = repository.tags.len() > 1;
    let mut name = vec![(repository.label(namespace).into(), Tone::Plain)];
    if multi {
        name.push((format!(" ({} tags)", repository.tags.len()), Tone::Muted));
    }
    let mut parts: [Vec<Part>; 8] = Default::default();
    parts[0] = name;
    parts[1] = vec![(host.into(), Tone::Plain)];
    let mut row = RowCells {
        reference: repository.reference(host, repository.representative.as_deref()),
        parts,
        twisty: multi.then_some(open),
        icon: None,
        tag: false,
        muted: false,
    };
    if let Some(error) = &repository.error {
        row.parts[7] = vec![(format!("⚠ {error}"), Tone::Warn)];
        return row;
    }
    let Some(tag) = repository.representative_row() else {
        row.parts[7] = vec![(
            if repository.tags.is_empty() {
                "No tags"
            } else {
                "Reading…"
            }
            .into(),
            Tone::Muted,
        )];
        return row;
    };
    row.parts[3] = vec![(tag.tag.clone(), Tone::Code)];
    let (mut state, muted) = state_parts(tag, facts);
    row.muted = muted;
    let in_library = repository
        .read
        .iter()
        .filter(|t| t.local == Some(LocalState::InLibrary))
        .count();
    if multi && in_library > 0 {
        state.push((
            format!(" · {in_library} of {} tags", repository.tags.len()),
            Tone::Muted,
        ));
    }
    state.extend(newer_needs(repository));
    row.parts[7] = state;
    if let Ok(record) = &tag.record {
        row.icon = Group::of(&record.artifact_type).map(Group::icon);
        row.parts[2] = vec![(record.title.clone().unwrap_or_default(), Tone::Plain)];
        row.parts[4] = vec![(
            type_label(&record.artifact_type, &repository.name).into(),
            Tone::Plain,
        )];
        row.parts[5] = licence(record);
        row.parts[6] = vec![(human_size(record.size), Tone::Plain)];
    }
    row
}

/// The newer versions a repository's row cannot stand for, after its
/// Local state: " · 1.3.0 needs unemupc 0.2.0 or newer".
fn newer_needs(repository: &RepositoryRow) -> Vec<Part> {
    repository
        .newer_needs
        .iter()
        .map(|(tag, needs)| {
            let mut phrase = needs.clone();
            phrase[..1].make_ascii_lowercase();
            (format!(" · {tag} {phrase}"), Tone::Warn)
        })
        .collect()
}

/// One tag's row under its expanded repository.
fn tag_row(host: &str, repository: &RepositoryRow, tag: &str, facts: &PlatformFacts) -> RowCells {
    let mut parts: [Vec<Part>; 8] = Default::default();
    parts[0] = vec![(format!(":{tag}"), Tone::Code)];
    parts[1] = vec![(host.into(), Tone::Plain)];
    let mut row = RowCells {
        reference: repository.reference(host, Some(tag)),
        parts,
        twisty: None,
        icon: None,
        tag: true,
        muted: false,
    };
    let Some(read) = repository.row(tag) else {
        row.parts[7] = vec![("Reading…".into(), Tone::Muted)];
        return row;
    };
    let (state, muted) = state_parts(read, facts);
    row.parts[7] = state;
    row.muted = muted;
    if let Ok(record) = &read.record {
        let mut title = record.title.clone().unwrap_or_default();
        if !record.formats.is_empty() {
            title.push_str(" · ");
            title.push_str(&record.formats.join(", "));
        }
        row.parts[2] = vec![(title, Tone::Plain)];
        row.parts[4] = vec![(
            type_label(&record.artifact_type, &repository.name).into(),
            Tone::Plain,
        )];
        row.parts[5] = licence(record);
        row.parts[6] = vec![(human_size(record.size), Tone::Plain)];
    }
    row
}

/// A repository's tags newest first, by its artifact type's order: Semantic
/// Versioning 2.0.0 §11 precedence for machine images, then `latest`, then
/// the order of set and package tags ([`image_store::tag::newest_first`]),
/// which is a set's or package's own: by release, the alias of each release
/// first as its newest, then its revisions highest first.
fn newest_first(repository: &RepositoryRow) -> Vec<String> {
    let machine = group_of(repository) == Some(Group::Machines);
    let mut tags = repository.tags.clone();
    tags.sort_by(|a, b| {
        let by_set = || image_store::tag::newest_first(a, b);
        if !machine {
            return by_set();
        }
        match (
            image_store::tag::precedence(a),
            image_store::tag::precedence(b),
        ) {
            (Some(a), Some(b)) => b.cmp(&a),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            // A machine repository's `latest` leads its other tags.
            (None, None) => {
                let latest = |tag: &str| tag == image_store::tag::LATEST;
                latest(b).cmp(&latest(a)).then_with(by_set)
            }
        }
    });
    tags
}

/// The key a repository sorts by in `column`, then by name.
fn sort_key(repository: &RepositoryRow, column: usize) -> (String, String) {
    let tag = repository.representative_row();
    let record = tag.and_then(|t| t.record.as_ref().ok());
    let key = match column {
        2 => record
            .and_then(|r| r.title.as_deref())
            .unwrap_or_default()
            .to_lowercase(),
        3 => repository.representative.clone().unwrap_or_default(),
        TYPE_COLUMN => format!(
            "{}",
            group_of(repository).map_or(Group::ALL.len(), |g| g as usize)
        ),
        5 => record
            .map_or("2", |r| if r.user_supplied { "1" } else { "0" })
            .into(),
        6 => format!("{:020}", record.map_or(0, |r| r.size)),
        7 => {
            let rank = match tag.and_then(|t| t.local.as_ref()) {
                Some(LocalState::NewerAvailable { .. } | LocalState::NewerRelease { .. }) => 0,
                Some(LocalState::OtherImage { .. }) => 1,
                Some(LocalState::LocalNewer { .. }) => 2,
                Some(LocalState::NotPulled { .. }) => 3,
                Some(LocalState::PushedFromHere { .. }) => 4,
                Some(LocalState::InLibrary) => 5,
                None => 6,
            };
            rank.to_string()
        }
        _ => String::new(),
    };
    (key, repository.name.clone())
}

/// One line of the list: a repository by its place in the registry's rows,
/// or one tag of it when it is expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    repository: usize,
    tag: Option<String>,
}

/// The node's lines, sorted, with the tags of expanded repositories.
fn lines(
    view: &RegistryView,
    node: &Node,
    expanded: &BTreeSet<(String, String)>,
    sort: usize,
    descending: bool,
) -> Vec<Line> {
    let mut repositories: Vec<usize> = (0..view.repositories.len())
        .filter(|&i| node.holds(view, &view.repositories[i]))
        .collect();
    repositories.sort_by_cached_key(|&i| sort_key(&view.repositories[i], sort));
    if descending {
        repositories.reverse();
    }
    let mut lines = Vec::new();
    for index in repositories {
        let repository = &view.repositories[index];
        lines.push(Line {
            repository: index,
            tag: None,
        });
        if repository.tags.len() > 1
            && expanded.contains(&(view.host.clone(), repository.name.clone()))
        {
            lines.extend(newest_first(repository).into_iter().map(|tag| Line {
                repository: index,
                tag: Some(tag),
            }));
        }
    }
    lines
}

/// The mock-up's title blue, the strip's accent while a listing runs.
fn accent(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(0x3a, 0x5f, 0x99)
    } else {
        Color32::from_rgb(0x4b, 0x79, 0xc1)
    }
}

/// The button of an action. A login is asked on the registry's entry in
/// Preferences › Registries, which a registry preferences do not list must
/// get first: its button says Add registry….
fn action_label(action: Action, configured: bool) -> &'static str {
    match action {
        Action::Stop => "Stop",
        Action::Refresh => "Refresh",
        Action::TryAgain => "Try again",
        Action::LogIn if configured => "Log in…",
        Action::LogIn => "Add registry…",
    }
}

/// The mock-up's strip above the rows (`.banner`): on the field, inside a
/// thin outline, with a 4 px bar at its left in the title blue while a
/// listing runs and in the warning colour otherwise; a spinner or ⚠, the
/// heading in bold with the text after it, wrapped, and its button at the
/// right. Returns the action clicked.
fn strip(ui: &mut egui::Ui, message: &Message, listing: bool, configured: bool) -> Option<Action> {
    let visuals = ui.visuals().clone();
    let bar = if listing {
        accent(&visuals)
    } else {
        visuals.warn_fg_color
    };
    let mut clicked = None;
    let frame = egui::Frame::new()
        .fill(visuals.extreme_bg_color)
        .stroke(Stroke::new(1.0, pane::shadow_colour(&visuals)))
        .inner_margin(egui::Margin {
            left: 12,
            right: 8,
            top: 6,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if listing {
                    ui.add(egui::Spinner::new().size(12.0));
                } else {
                    ui.colored_label(visuals.warn_fg_color, "⚠");
                }
                let button = message
                    .action
                    .map(|action| action_label(action, configured));
                let button_width = button.map_or(0.0, |label| {
                    ui.painter()
                        .layout_no_wrap(
                            label.into(),
                            egui::TextStyle::Button.resolve(ui.style()),
                            Color32::WHITE,
                        )
                        .size()
                        .x
                        + 2.0 * ui.spacing().button_padding.x
                        + ui.spacing().item_spacing.x
                });
                let font = egui::TextStyle::Body.resolve(ui.style());
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    &message.heading,
                    0.0,
                    egui::TextFormat::simple(font.clone(), visuals.strong_text_color()),
                );
                job.append(
                    &format!(" {}", message.text),
                    0.0,
                    egui::TextFormat::simple(font, visuals.text_color()),
                );
                let width = (ui.available_width() - button_width).max(40.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(width);
                        ui.add(egui::Label::new(job).wrap());
                    },
                );
                if let (Some(action), Some(label)) = (message.action, button) {
                    if ui.button(label).clicked() {
                        clicked = Some(action);
                    }
                }
            });
        });
    let rect = frame.response.rect;
    ui.painter().rect_filled(
        Rect::from_min_max(rect.min, egui::pos2(rect.left() + 4.0, rect.bottom())),
        0.0,
        bar,
    );
    clicked
}

/// The picture and tone of the full-pane message of a registry's state.
fn message_look(state: &State, visuals: &egui::Visuals) -> (Icon, Color32) {
    match state {
        State::Listing => (Icon::Listing, visuals.text_color()),
        State::Listed | State::Idle | State::Stopped => (Icon::EmptyRegistry, visuals.text_color()),
        State::LoginRequired { .. } => (Icon::LoginRequired, visuals.warn_fg_color),
        State::Unreachable { .. } | State::ServerError { .. } | State::Failed(_) => {
            (Icon::Unreachable, visuals.error_fg_color)
        }
    }
}

/// The mock-up's message instead of the rows (`.listmsg`, `.msgcard`): the
/// 40 px picture beside the heading, the text and the button, at most 64
/// characters wide, in the middle of the list's area. Returns the action
/// clicked.
fn message_card(
    ui: &mut egui::Ui,
    message: &Message,
    state: &State,
    configured: bool,
) -> Option<Action> {
    let (icon, ink) = message_look(state, ui.visuals());
    let area = ui.available_rect_before_wrap();
    let char_width =
        ui.fonts_mut(|f| f.glyph_width(&egui::TextStyle::Body.resolve(ui.style()), '0'));
    let width = (64.0 * char_width).min(area.width() - 32.0).max(120.0);
    let gap = 14.0;
    let text_width = width - 40.0 - gap;
    let heading_font = egui::FontId::proportional(15.0);
    let heading = ui.painter().layout(
        message.heading.clone(),
        heading_font.clone(),
        ui.visuals().strong_text_color(),
        text_width,
    );
    let body = ui.painter().layout(
        message.text.clone(),
        egui::TextStyle::Body.resolve(ui.style()),
        ui.visuals().text_color(),
        text_width,
    );
    let button = if message.action.is_some() {
        ui.spacing().interact_size.y + 8.0
    } else {
        0.0
    };
    let height = (heading.size().y + 4.0 + body.size().y + button).max(40.0);
    let top = area.top() + ((area.height() - height) / 2.0).max(24.0);
    let left = area.left() + ((area.width() - width) / 2.0).max(0.0);
    let card = Rect::from_min_size(egui::pos2(left, top), Vec2::new(width, height));
    ui_kit::shell_icons::paint(
        ui.painter(),
        Rect::from_min_size(card.min, Vec2::splat(40.0)),
        icon,
        ink,
        ui.visuals(),
    );
    let mut clicked = None;
    let text = Rect::from_min_max(egui::pos2(card.left() + 40.0 + gap, card.top()), card.max);
    let mut column = ui.new_child(egui::UiBuilder::new().max_rect(text));
    column.spacing_mut().item_spacing.y = 4.0;
    column
        .add(egui::Label::new(RichText::new(&message.heading).font(heading_font).strong()).wrap());
    column.add(egui::Label::new(&message.text).wrap());
    if let Some(action) = message.action {
        column.add_space(4.0);
        let label = action_label(action, configured);
        let primary = matches!(action, Action::TryAgain | Action::LogIn);
        let response = if primary {
            let button = pane::primary(&column, label);
            column.add(button)
        } else {
            column.button(label)
        };
        if response.clicked() {
            clicked = Some(action);
        }
    }
    // The message takes the list's whole area, whatever its height.
    ui.advance_cursor_after_rect(area);
    clicked
}

/// What the list asked for this frame.
#[derive(Default)]
pub struct ListAnswer {
    pub action: Option<Action>,
    /// A row's tag was clicked, by repository and tag, with the modifiers
    /// held.
    pub select: Option<(String, String, egui::Modifiers)>,
    pub toggle: Option<String>,
    pub next_page: bool,
    /// Enter on a tag's row: Pull….
    pub pull: bool,
}

/// The event that ends a request the worker never got, with `message`.
pub fn unsent(request: &BrowseRequest) -> impl FnOnce(String) -> BrowseEvent {
    let (id, local) = match request {
        BrowseRequest::List { job, .. } | BrowseRequest::Read { job, .. } => (job.id, false),
        BrowseRequest::Local { id, .. } => (*id, true),
    };
    move |message| {
        if local {
            BrowseEvent::Local {
                id,
                states: Err(message),
            }
        } else {
            BrowseEvent::Ended {
                id,
                end: End::Failed(Failure::Local(message)),
            }
        }
    }
}

/// What Remotes reads of the library around it, lent for one call.
pub struct RemotesContext<'a> {
    /// The platforms this program runs, to tell a tag it cannot run.
    pub facts: &'a PlatformFacts,
    /// The classic skin draws the list's headers and the dialogs' titles.
    pub classic: bool,
    /// The library's local tags, which Pull… compares a pull with.
    pub tags: &'a [(String, Digest)],
    /// The media drives of the library's stopped machines, which Pull and
    /// insert… and Insert… offer.
    pub drives: &'a [Drive],
    /// How many library items could go to the shown node, once the dry
    /// runs of the library are read.
    pub here: Option<HereCounts>,
}

/// What a namespace's pane says of the library: how many items could go
/// there, how many are there already, of how many.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HereCounts {
    pub can: usize,
    pub same: usize,
    pub total: usize,
}

/// What Remotes asks of the library around it. The library applies them in
/// the order they come, as Remotes would have done them itself.
pub enum RemotesAction {
    /// Send a browsing request to the worker.
    Browse(BrowseRequest),
    /// Show a remote node instead of a library collection: the library drops
    /// its selection, so Start never targets something no longer shown, and
    /// then Remotes shows the node ([`RemotesView::select_remote`]).
    Select(Node),
    /// Select a tag in the library collection of its artifact type.
    ShowInLibrary {
        tag: String,
        artifact_type: String,
        image: Descriptor,
    },
    /// New machine from image… on a pulled image, selected in Images.
    NewMachine {
        tag: String,
        image: Descriptor,
        name: String,
    },
    /// Pull…, Pull and create machine… or Pull and insert… of the selected
    /// tags.
    BeginPull(Mode),
    /// Preferences › Registries opens on this registry's Log in….
    LogIn(String),
    /// Push here… of a registry or namespace.
    PushHere(library_model::push_destination::Place),
    /// Tools › Preferences › Registries.
    Registries,
    /// The media picker opens on a stopped machine's drive, the set chosen.
    Insert {
        machine: String,
        unit: u8,
        set: Digest,
    },
}

/// A browsing request, if there is one, as the action that sends it.
fn browse(request: Option<BrowseRequest>) -> Vec<RemotesAction> {
    request.into_iter().map(RemotesAction::Browse).collect()
}

impl RemotesView {
    /// Give Remotes the logins of Preferences › Registries; registries
    /// already open read their list from these preferences again.
    pub fn attach_remotes(&mut self, accounts: Accounts) {
        match &mut self.model {
            Some(model) => model.reattach(accounts),
            None => self.model = Some(Remotes::open(accounts)),
        }
    }

    /// A registry's node is shown instead of a library collection.
    pub fn remote_shown(&self) -> bool {
        self.node.is_some()
    }

    pub fn on_browse(&mut self, event: BrowseEvent) {
        if let Some(model) = &mut self.model {
            model.on_event(event);
        }
    }

    /// Stop every registry's listing and reads, as before every start of a
    /// machine: the worker answers one request at a time, so a slow registry
    /// must not hold the start behind it.
    pub fn stop_remotes(&mut self) {
        if let Some(model) = &mut self.model {
            model.stop_all();
        }
        self.collapse_unread(None);
    }

    /// The worker is gone: every running registry job ends with `message`.
    pub fn fail_remotes(&mut self, message: &str) {
        if let Some(model) = &mut self.model {
            model.fail_running(message);
        }
        self.collapse_unread(None);
    }

    /// The library's tags changed: compare every read tag with them again.
    pub fn remote_library_changed(&mut self) -> Vec<RemotesAction> {
        let request = self.model.as_mut().and_then(Remotes::library_changed);
        browse(request)
    }

    /// Preferences closed: the registries are read again, and the one whose
    /// Log in… opened them lists again.
    pub fn remote_preferences_closed(&mut self) -> Vec<RemotesAction> {
        let Some(model) = &mut self.model else {
            return Vec::new();
        };
        model.reload();
        let Some(host) = self.login.take() else {
            return Vec::new();
        };
        let request = model.refresh(&host);
        self.collapse_unread(Some(&host));
        browse(request)
    }

    /// F5 and the toolbar's Refresh: list the shown registry again.
    pub fn refresh_remote(&mut self) -> Vec<RemotesAction> {
        let Some(host) = self.node.as_ref().map(|n| n.host.clone()) else {
            return Vec::new();
        };
        let request = self.model.as_mut().and_then(|m| m.refresh(&host));
        self.collapse_unread(Some(&host));
        browse(request)
    }

    /// File › Get templates… and the empty library's button: Remotes › the
    /// default registry › `unemu/` › Machine images, listed first if this
    /// session has not.
    pub fn show_templates(&mut self, accounts: Accounts) -> Vec<RemotesAction> {
        self.attach_remotes(accounts);
        let Some(host) = self
            .model
            .as_ref()
            .and_then(|m| m.registries.first())
            .map(|v| v.host.clone())
        else {
            return Vec::new();
        };
        self.open.insert(host.clone());
        self.open
            .insert(namespace_key(&host, Some(OFFICIAL_NAMESPACE)));
        vec![RemotesAction::Select(Node {
            host,
            namespace: Some(Some(OFFICIAL_NAMESPACE.into())),
            group: Some(Group::Machines),
        })]
    }

    /// Show a remote node, and list its registry the first time in this
    /// session. The library drops its own selection first (see
    /// [`RemotesAction::Select`]).
    pub fn select_remote(&mut self, node: Node) -> Vec<RemotesAction> {
        // A tag of another node is no longer shown: Pull… never takes it.
        if self.node.as_ref() != Some(&node) {
            self.selected = None;
            self.anchor = None;
        }
        self.multi.clear();
        let request = self.model.as_mut().and_then(|m| m.open_node(&node.host));
        self.node = Some(node);
        browse(request)
    }

    /// The Remotes group under Library in the tree, after the mock-up's rule.
    pub fn remote_tree(&mut self, ui: &mut egui::Ui) -> Vec<RemotesAction> {
        ui.add_space(6.0);
        let (rule, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline(
            rule.x_range(),
            rule.center().y,
            Stroke::new(1.0, pane::shadow_colour(ui.visuals())),
        );
        ui.add_space(4.0);
        ui.strong("Remotes");
        let Some(model) = &self.model else {
            return Vec::new();
        };
        if let Some(error) = &model.error {
            ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().warn_fg_color)).wrap());
        }
        let revision = model.revision();
        let fresh = self
            .tree
            .as_ref()
            .is_some_and(|cache| cache.revision == revision && cache.open == self.open);
        if !fresh {
            self.tree = Some(TreeCache {
                revision,
                open: self.open.clone(),
                lines: tree_lines(model, &self.open),
            });
        }
        let mut toggled = None;
        let mut picked = None;
        for line in self.tree.iter().flat_map(|cache| &cache.lines) {
            let current = self.node.as_ref() == Some(&line.node);
            let (toggle, pick) = tree_line(ui, line, current);
            if toggle {
                toggled = line
                    .twisty
                    .clone()
                    .map(|(open, key)| (open, key, line.node.clone()));
            }
            if pick {
                picked = Some(line.node.clone());
            }
        }
        let mut actions = Vec::new();
        if let Some((open, key, node)) = toggled {
            if open {
                self.open.remove(&key);
            } else {
                self.open.insert(key);
                // Opening a registry's node lists it the first time.
                let request = self.model.as_mut().and_then(|m| m.open_node(&node.host));
                actions.extend(browse(request));
            }
        }
        if let Some(node) = picked {
            // A node's own pane shows until a tag is selected again.
            self.selected = None;
            self.anchor = None;
            self.multi.clear();
            actions.push(RemotesAction::Select(node));
        }
        actions
    }

    /// The list of the shown node: the strip, the rows or the message
    /// instead of them, and the paging line.
    pub fn remote_list(
        &mut self,
        ui: &mut egui::Ui,
        context: &RemotesContext<'_>,
    ) -> Vec<RemotesAction> {
        let classic = context.classic;
        let facts = context.facts;
        let mut answer = ListAnswer::default();
        let host;
        {
            let view = &mut *self;
            let (Some(model), Some(node)) = (view.model.as_ref(), view.node.as_ref()) else {
                return Vec::new();
            };
            let Some(registry) = model.registry(&node.host) else {
                // Preferences no longer list it.
                view.node = None;
                return Vec::new();
            };
            host = registry.host.clone();
            if let Some(banner) = registry.banner() {
                let listing = registry.state == State::Listing;
                answer.action = strip(ui, &banner, listing, registry.configured);
                ui.add_space(3.0);
            }
            if let Some(message) = registry.message() {
                answer.action = answer.action.or(message_card(
                    ui,
                    &message,
                    &registry.state,
                    registry.configured,
                ));
            } else {
                let revision = model.revision();
                let sorted = (view.sort, view.descending);
                let fresh = view.list.as_ref().is_some_and(|cache| {
                    cache.revision == revision
                        && cache.node == *node
                        && cache.sort == sorted
                        && cache.expanded == view.expanded
                });
                if !fresh {
                    view.list = Some(ListCache {
                        revision,
                        node: node.clone(),
                        sort: sorted,
                        expanded: view.expanded.clone(),
                        lines: lines(registry, node, &view.expanded, sorted.0, sorted.1),
                    });
                }
                let lines = view.list.as_ref().map_or(&[][..], |cache| &cache.lines[..]);
                if lines.is_empty() {
                    ui.label(if registry.state == State::Listing {
                        "Nothing here yet; the listing goes on."
                    } else {
                        "No repositories here. Pick another node."
                    });
                } else {
                    let widths = fit_columns(ui, &COLUMNS);
                    let sort = &mut view.sort;
                    let descending = &mut view.descending;
                    let expanded = &view.expanded;
                    let selected = &view.selected;
                    let multi = &view.multi;
                    egui::ScrollArea::horizontal()
                        .id_salt(("remote-columns", node))
                        .show(ui, |ui| {
                            ui.set_min_width(table_width(ui, &widths));
                            sort_header(
                                ui,
                                &COLUMNS.labels,
                                &widths,
                                &[],
                                classic,
                                sort,
                                descending,
                            );
                            // Each node keeps its own place in its list.
                            egui::ScrollArea::vertical()
                                .id_salt(("remote-list", node))
                                .max_height((ui.available_height() - 26.0).max(80.0))
                                .show_rows(ui, ROW, lines.len(), |ui, range| {
                                    for index in range {
                                        let line = &lines[index];
                                        let repository = &registry.repositories[line.repository];
                                        let picked = selected.as_ref().filter(|(h, r, _)| {
                                            *h == host && *r == repository.name
                                        });
                                        let several = multi.len() > 1;
                                        let chosen = |tag: &str| {
                                            multi
                                                .iter()
                                                .any(|(r, t)| *r == repository.name && t == tag)
                                        };
                                        let (cells, tag, is_selected) = match &line.tag {
                                            None => {
                                                let open = expanded.contains(&(
                                                    host.clone(),
                                                    repository.name.clone(),
                                                ));
                                                // A collapsed repository shows the
                                                // selection of any of its tags.
                                                let is_selected = !open
                                                    && if several {
                                                        repository.tags.iter().any(|t| chosen(t))
                                                    } else {
                                                        picked.is_some()
                                                    };
                                                let cells = repository_row(
                                                    &host,
                                                    repository,
                                                    open,
                                                    facts,
                                                    node.namespace
                                                        .as_ref()
                                                        .and_then(|n| n.as_deref()),
                                                );
                                                (
                                                    cells,
                                                    repository.representative.clone(),
                                                    is_selected,
                                                )
                                            }
                                            Some(tag) => {
                                                let is_selected = if several {
                                                    chosen(tag)
                                                } else {
                                                    picked.is_some_and(|(_, _, t)| t == tag)
                                                };
                                                (
                                                    tag_row(&host, repository, tag, facts),
                                                    Some(tag.clone()),
                                                    is_selected,
                                                )
                                            }
                                        };
                                        let id = ui.scope_id().with((
                                            "remote-row",
                                            &host,
                                            &repository.name,
                                            &line.tag,
                                        ));
                                        let row = draw_row(
                                            ui,
                                            &cells,
                                            &widths,
                                            is_selected,
                                            index % 2 == 0,
                                            id,
                                        );
                                        let expands = cells.twisty.is_some();
                                        if row.toggled || (expands && (row.double || row.enter)) {
                                            answer.toggle = Some(repository.name.clone());
                                        } else if let (true, Some(tag)) = (row.enter, &tag) {
                                            // egui also clicks a focused row on
                                            // Enter: the focused tag is the one
                                            // pulled, the selection of several
                                            // kept when it is part of it.
                                            if !(several && chosen(tag)) {
                                                answer.select = Some((
                                                    repository.name.clone(),
                                                    tag.clone(),
                                                    egui::Modifiers::NONE,
                                                ));
                                            }
                                            answer.pull = true;
                                        } else if row.clicked {
                                            if let Some(tag) = tag {
                                                let modifiers = ui.input(|i| i.modifiers);
                                                answer.select =
                                                    Some((repository.name.clone(), tag, modifiers));
                                            }
                                        }
                                    }
                                });
                        });
                    let count = registry
                        .repositories
                        .iter()
                        .filter(|r| node.holds(registry, r))
                        .count();
                    ui.horizontal(|ui| {
                        let more = registry.next.is_some();
                        ui.label(if more {
                            format!(
                                "Repositories 1–{count}. The catalog is read 50 repositories a page."
                            )
                        } else {
                            format!(
                                "Repositories 1–{count} of {count}. The catalog is read 50 repositories a page."
                            )
                        });
                        if more
                            && ui
                                .add_enabled(
                                    registry.state == State::Listed,
                                    egui::Button::new("Next 50 ›"),
                                )
                                .clicked()
                        {
                            answer.next_page = true;
                        }
                    });
                }
            }
        }
        self.apply_list(&host, answer, context)
    }

    pub fn apply_list(
        &mut self,
        host: &str,
        answer: ListAnswer,
        context: &RemotesContext<'_>,
    ) -> Vec<RemotesAction> {
        let mut actions = Vec::new();
        let mut requests = Vec::new();
        if let Some(model) = &mut self.model {
            match answer.action {
                Some(Action::Stop) => model.stop(host),
                Some(Action::Refresh | Action::TryAgain) => requests.push(model.refresh(host)),
                Some(Action::LogIn) => {
                    // Preferences › Registries opens on this registry's Log
                    // in; the registry lists again once they close.
                    actions.push(RemotesAction::LogIn(host.into()));
                    self.login = Some(host.into());
                }
                None => (),
            }
            if answer.next_page {
                requests.push(model.next_page(host));
            }
            if let Some(repository) = answer.toggle {
                let key = (host.to_owned(), repository.clone());
                if !self.expanded.remove(&key) {
                    self.expanded.insert(key);
                    requests.push(model.expand(host, &repository));
                }
            }
            if let Some((repository, tag, modifiers)) = answer.select {
                requests.push(model.select(host, &repository, &tag));
                self.pick(host, repository, tag, modifiers);
            }
        }
        if answer.pull && self.can_pull_remote(context) {
            actions.push(RemotesAction::BeginPull(Mode::Pull));
        }
        if matches!(
            answer.action,
            Some(Action::Stop | Action::Refresh | Action::TryAgain)
        ) {
            self.collapse_unread(Some(host));
        }
        actions.extend(requests.into_iter().flat_map(browse));
        actions
    }
}

mod details;

/// What the desktop's tests of Remotes, which drive the whole library
/// frame, compare with and build their states from.
#[cfg(feature = "test-support")]
pub mod test_support {
    use super::{Action, Columns, Remotes, RepositoryRow, TreeLine};
    use std::collections::BTreeSet;

    /// A list row's height.
    pub const ROW: f32 = super::ROW;
    /// The Remotes list's columns.
    pub const COLUMNS: Columns<8> = super::COLUMNS;

    pub fn tree_lines(model: &Remotes, open: &BTreeSet<String>) -> Vec<TreeLine> {
        super::tree_lines(model, open)
    }

    pub fn action_label(action: Action, configured: bool) -> &'static str {
        super::action_label(action, configured)
    }

    pub fn namespace_key(host: &str, namespace: Option<&str>) -> String {
        super::namespace_key(host, namespace)
    }

    pub fn type_label(artifact_type: &str, repository: &str) -> &'static str {
        super::type_label(artifact_type, repository)
    }

    pub fn newest_first(repository: &RepositoryRow) -> Vec<String> {
        super::newest_first(repository)
    }
}
