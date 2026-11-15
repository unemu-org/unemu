// SPDX-License-Identifier: AGPL-3.0-or-later
//! The one "Save as new image…" form: a repository name and a tag in the
//! registry tag grammar, used by Disk layers (Save as layer…, New image from
//! here…, Keep as new image…) and by Settings on an image; machine-images §§3
//! and 7, `backlog/docs/design/mockups/save-as-image.html`.
//!
//! The tag rules are the store's: a tag shaped like a version never moves once
//! it names an image; any other existing tag moves only after a confirmation
//! that shows the old digest, and the move is a compare-and-swap from that
//! digest. New machine… applies the same rules.

use image_store::{Digest, Store};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TagLevel {
    Ok,
    Warn,
    Bad,
}
/// The result of checking the repository name and tag against the store rules.
#[derive(Clone, Debug)]
pub(crate) struct TagCheck {
    pub level: TagLevel,
    pub text: String,
    /// A suggested free version tag after a refusal.
    pub fix: Option<String>,
    /// The digest an existing movable tag names; saving asks before moving it.
    pub moving: Option<Digest>,
    /// The digest an existing version tag names; it never moves.
    pub taken: Option<Digest>,
}

pub(crate) fn short(digest: &Digest) -> String {
    format!("sha256:{}…{}", &digest.hex()[..8], &digest.hex()[60..])
}

type Version = (u64, u64, u64);

fn version(tag: &str) -> Option<Version> {
    if !Store::is_version_tag(tag) {
        return None;
    }
    let core = tag.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|n| n.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}
fn versions<'a>(
    repo: &'a str,
    tags: &'a [(String, Digest)],
) -> impl Iterator<Item = (Version, &'a Digest)> + 'a {
    let prefix = format!("{repo}:");
    tags.iter()
        .filter_map(move |(name, digest)| Some((version(name.strip_prefix(&prefix)?)?, digest)))
}

/// The next free minor version of a repository, or 1.0.0.
pub(crate) fn next_version(repo: &str, tags: &[(String, Digest)]) -> String {
    match versions(repo, tags).map(|(v, _)| v).max() {
        Some((major, minor, _)) => format!("{major}.{}.0", minor + 1),
        None => "1.0.0".into(),
    }
}

/// Store and registry tag rules: the OCI grammar, version tags never move,
/// other tags move only after a confirmation. `asks` names who asks before a
/// move, such as "Create will ask".
pub(crate) fn check_tag(
    repo: &str,
    tag: &str,
    tags: &[(String, Digest)],
    asks: &str,
    fix: impl FnOnce() -> String,
) -> TagCheck {
    let bad = |text: String| TagCheck {
        level: TagLevel::Bad,
        text,
        fix: None,
        moving: None,
        taken: None,
    };
    let (repo, tag) = (repo.trim(), tag.trim());
    if Store::validate_tag_name(&format!("{repo}:latest")).is_err() {
        return bad(
            "Repository names use lowercase letters and digits, separated by . _ or - characters."
                .into(),
        );
    }
    if tag.contains('+') {
        return bad(
            "Tags cannot contain \"+\": a registry tag allows only letters, digits and . _ -."
                .into(),
        );
    }
    let reference = format!("{repo}:{tag}");
    if Store::validate_tag_name(&reference).is_err() {
        return bad("Tags start with a letter, digit or _ and use only letters, digits and . _ - (at most 128 characters).".into());
    }
    let is_version = Store::is_version_tag(tag);
    match tags.iter().find(|(name, _)| name == &reference).map(|(_, d)| d) {
        Some(digest) if is_version => TagCheck {
            level: TagLevel::Bad,
            text: format!(
                "{reference} already exists ({}). Version tags never move, so this name cannot be reused.",
                short(digest)
            ),
            fix: Some(fix()),
            moving: None,
            taken: Some(digest.clone()),
        },
        Some(digest) => {
            let others: Vec<_> = tags
                .iter()
                .filter(|(name, d)| d == digest && name != &reference)
                .map(|(name, _)| name.as_str())
                .collect();
            TagCheck {
                level: TagLevel::Warn,
                text: format!(
                    "{reference} already names {}. {asks} before moving it; the old image stays reachable by its digest{}.",
                    short(digest),
                    if others.is_empty() {
                        String::new()
                    } else {
                        format!(" and by {}", others.join(", "))
                    }
                ),
                fix: None,
                moving: Some(digest.clone()),
                taken: None,
            }
        }
        None => TagCheck {
            level: TagLevel::Ok,
            text: format!(
                "{reference} is new. {}",
                if is_version {
                    "As a version tag it will never move."
                } else {
                    "This tag may move to a newer image later, after a confirmation."
                }
            ),
            fix: None,
            moving: None,
            taken: None,
        },
    }
}

/// Every local tag with the digest it names, for the tag rules.
pub(crate) fn local_tags(store: &Store) -> Result<Vec<(String, Digest)>, String> {
    let mut tags = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = store
            .list_tags(after.as_deref(), 1000)
            .map_err(|e| e.to_string())?;
        tags.extend(page.tags.into_iter().map(|t| (t.name, t.manifest.digest)));
        match page.next_after {
            Some(next) => after = Some(next),
            None => return Ok(tags),
        }
    }
}

/// The default repository of an image known only by its digest.
const RECOVERED: &str = "recovered";

/// The repository of `repo:tag` or `repo@digest`; a recovered session that
/// knows its image only by digest gets a valid default.
fn repository_of(source: &str) -> &str {
    let named = source.split('@').next().unwrap_or_default();
    if Store::validate_tag_name(named).is_ok() {
        named.rsplit_once(':').map_or(named, |(repo, _)| repo)
    } else if Store::validate_tag_name(&format!("{named}:latest")).is_ok() {
        named
    } else {
        RECOVERED
    }
}

/// Where and how to save, once the tag rules allow it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub reference: String,
    /// The digest a confirmed move replaces; `None` for a new tag.
    pub expected: Option<Digest>,
    /// A new title; `None` keeps the source image's title.
    pub title: Option<String>,
    pub flatten: bool,
}

/// A question the form asks before saving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Prompt {
    /// Move this non-version tag from the digest it names now?
    Move(Digest),
    /// This version tag already names that digest; it never moves.
    Refuse(Digest),
}

/// The headless state of the Save as new image… form.
pub(crate) struct SaveAsImage {
    pub source: String,
    pub source_digest: Digest,
    pub repo: String,
    pub tag: String,
    tag_edited: bool,
    /// Shown and saved when `Some`; Settings revisions keep the source title.
    pub title: Option<String>,
    source_title: Option<String>,
    /// "Flatten disk chains into new bases", offered by New image from here….
    pub flatten: Option<bool>,
    pub tags: Vec<(String, Digest)>,
    /// The source image's own version in its repository, if it has one.
    source_version: Option<Version>,
    pub prompt: Option<Prompt>,
    confirmed: Option<Digest>,
    /// Who asks before a move, in the check text.
    asks: &'static str,
}

impl SaveAsImage {
    /// Start from the source's repository with the next free version: the
    /// next minor version after the source's own version tag, else after the
    /// repository's newest version, else 1.0.0.
    pub fn new(source: &str, source_digest: Digest, tags: Vec<(String, Digest)>) -> Self {
        let repo = repository_of(source).to_owned();
        let source_version = versions(&repo, &tags)
            .filter(|(_, digest)| **digest == source_digest)
            .map(|(v, _)| v)
            .max();
        let mut form = Self {
            source: source.into(),
            source_digest,
            repo,
            tag: String::new(),
            tag_edited: false,
            title: None,
            source_title: None,
            flatten: None,
            tags,
            source_version,
            prompt: None,
            confirmed: None,
            asks: "Saving will ask",
        };
        form.tag = form.suggested();
        form
    }
    /// Offer the title field, starting from the source's title.
    pub fn with_title(mut self, title: Option<&str>) -> Self {
        self.source_title = title.map(str::to_owned);
        self.title = Some(title.unwrap_or_default().to_owned());
        self
    }
    /// Offer the flatten option of New image from here….
    pub fn with_flatten(mut self) -> Self {
        self.flatten = Some(false);
        self
    }
    /// Publishing a Settings revision asks before a move, too.
    pub fn publishing(mut self) -> Self {
        self.asks = "Publishing will ask";
        self
    }
    fn source_repo(&self) -> &str {
        repository_of(&self.source)
    }
    fn taken(&self, version: Version) -> bool {
        let name = format!(
            "{}:{}.{}.{}",
            self.repo.trim(),
            version.0,
            version.1,
            version.2
        );
        self.tags.iter().any(|(tag, _)| tag == &name)
    }
    /// The default version tag of the current repository name.
    pub fn suggested(&self) -> String {
        let repo = self.repo.trim();
        let start = self
            .source_version
            .filter(|_| repo == self.source_repo())
            .or_else(|| versions(repo, &self.tags).map(|(v, _)| v).max())
            .map(|(major, minor, _)| (major, minor + 1, 0));
        let Some(mut version) = start else {
            return "1.0.0".into();
        };
        while self.taken(version) {
            version.1 += 1;
        }
        format!("{}.{}.{}", version.0, version.1, version.2)
    }
    /// Version bumps and `latest`, offered beside the tag field.
    pub fn chips(&self) -> Vec<String> {
        let repo = self.repo.trim();
        let mut chips = match versions(repo, &self.tags).map(|(v, _)| v).max() {
            Some((major, minor, patch)) => vec![
                format!("{major}.{minor}.{}", patch + 1),
                format!("{major}.{}.0", minor + 1),
                format!("{}.0.0", major + 1),
            ],
            None => vec!["1.0.0".to_owned()],
        };
        let suggested = self.suggested();
        if !chips.contains(&suggested) {
            chips.insert(0, suggested);
        }
        chips.push("latest".into());
        chips
    }
    pub fn set_repo(&mut self, repo: String) {
        if repo != self.repo {
            self.repo = repo;
            if !self.tag_edited {
                self.tag = self.suggested();
            }
            self.confirmed = None;
        }
    }
    pub fn set_tag(&mut self, tag: String) {
        if tag != self.tag {
            self.tag = tag;
            self.tag_edited = true;
            self.confirmed = None;
        }
    }
    pub fn reference(&self) -> String {
        format!("{}:{}", self.repo.trim(), self.tag.trim())
    }
    pub fn check(&self) -> TagCheck {
        check_tag(&self.repo, &self.tag, &self.tags, self.asks, || {
            self.suggested()
        })
    }
    fn target(&self, expected: Option<Digest>) -> Target {
        Target {
            reference: self.reference(),
            expected,
            title: self
                .title
                .as_ref()
                .filter(|title| title.trim() != self.source_title.as_deref().unwrap_or_default())
                .map(|title| title.trim().to_owned()),
            flatten: self.flatten.unwrap_or(false),
        }
    }
    /// Save: a free tag saves at once; an existing version tag is refused with
    /// the next free version; any other existing tag asks before moving.
    pub fn submit(&mut self) -> Option<Target> {
        let check = self.check();
        match (check.level, check.taken, check.moving) {
            (TagLevel::Bad, Some(digest), _) => {
                self.prompt = Some(Prompt::Refuse(digest));
                None
            }
            (TagLevel::Bad, None, _) => None,
            (_, _, Some(current)) if self.confirmed.as_ref() == Some(&current) => {
                Some(self.target(Some(current)))
            }
            (_, _, Some(current)) => {
                self.prompt = Some(Prompt::Move(current));
                None
            }
            _ => Some(self.target(None)),
        }
    }
    /// Move tag: the compare-and-swap expects the digest the user saw.
    pub fn confirm_move(&mut self) -> Option<Target> {
        let Some(Prompt::Move(digest)) = self.prompt.take() else {
            return None;
        };
        self.confirmed = Some(digest);
        self.submit()
    }
    /// Use the next free version after a refusal.
    pub fn use_next(&mut self) {
        if matches!(self.prompt.take(), Some(Prompt::Refuse(_))) {
            let next = self.suggested();
            self.set_tag(next);
        }
    }
    pub fn dismiss(&mut self) {
        self.prompt = None;
    }
    /// After a refused save: take the store's current tags, forget the
    /// confirmed digest and ask again. A tag another program moved asks to
    /// move it from its new digest; a version tag that appeared meanwhile is
    /// refused. Nothing is saved from here.
    pub fn refresh(&mut self, tags: Vec<(String, Digest)>) {
        self.tags = tags;
        self.confirmed = None;
        self.prompt = None;
        let _ = self.submit();
    }
}

// ---- rendering -------------------------------------------------------------

/// A dialog's message beside its mark, as the save-as-image mock-up lays out
/// its question and refusal dialogs.
fn alert(ui: &mut egui::Ui, icon: crate::shell_icons::Icon, body: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        crate::shell_icons::show(ui, icon);
        ui.vertical(body);
    });
}

fn small(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(egui::RichText::new(text.into()).small()).wrap());
}

/// The Name fieldset: repository, tag with version chips, title, flatten.
pub(crate) fn form(ui: &mut egui::Ui, model: &mut SaveAsImage) {
    let mut repo = model.repo.clone();
    ui.horizontal(|ui| {
        ui.add_sized([130.0, 20.0], egui::Label::new("Repository name"));
        ui.text_edit_singleline(&mut repo);
    });
    model.set_repo(repo);
    small(
        ui,
        format!(
            "Keep {} for a new version, or name what the image now holds (a new name starts at 1.0.0).",
            model.source_repo()
        ),
    );
    let mut tag = model.tag.clone();
    ui.horizontal(|ui| {
        ui.add_sized([130.0, 20.0], egui::Label::new("Tag"));
        ui.text_edit_singleline(&mut tag);
    });
    ui.horizontal_wrapped(|ui| {
        for chip in model.chips() {
            if ui.button(&chip).clicked() {
                tag = chip;
            }
        }
    });
    model.set_tag(tag);
    let check = model.check();
    ui.add(
        egui::Label::new(if check.level == TagLevel::Ok {
            // The save-as-image mock-up checks a free tag off with ✓.
            egui::RichText::new(format!("✓ {}", check.text))
        } else {
            egui::RichText::new(&check.text).color(ui.visuals().warn_fg_color)
        })
        .wrap(),
    );
    if let Some(title) = &mut model.title {
        ui.horizontal(|ui| {
            ui.add_sized([130.0, 20.0], egui::Label::new("Title"));
            ui.text_edit_singleline(title);
        });
    }
    if let Some(flatten) = &mut model.flatten {
        ui.checkbox(flatten, "Flatten disk chains into new bases");
    }
}

/// The Move tag? and Version tag already exists questions. Returns a target
/// once a move is confirmed.
pub(crate) fn prompts(
    ctx: &egui::Context,
    model: &mut SaveAsImage,
    classic: bool,
) -> Option<Target> {
    let prompt = model.prompt.clone()?;
    let reference = model.reference();
    let mut answer = None;
    let mut close = false;
    let title = match &prompt {
        Prompt::Move(_) => format!("Move tag {reference}?"),
        Prompt::Refuse(_) => "Version tag already exists".to_owned(),
    };
    egui::Modal::new(egui::Id::unique("save-as-image-prompt")).show(ctx, |ui| {
        ui.set_width(500.0);
        if classic {
            crate::shell::gradient_title(ui, &title);
        } else {
            ui.heading(&title);
        }
        match &prompt {
            Prompt::Move(current) => {
                // The save-as-image mock-up's question mark beside the text.
                alert(ui, crate::shell_icons::Icon::Question, |ui| {
                    ui.label(format!(
                        "{reference} already names another image. Move it to the image you are saving?"
                    ));
                    ui.monospace(format!("Now:   {}", short(current)));
                    ui.label("After: the image saved now.");
                    let others: Vec<_> = model
                        .tags
                        .iter()
                        .filter(|(name, d)| d == current && name != &reference)
                        .map(|(name, _)| name.as_str())
                        .collect();
                    small(
                        ui,
                        format!(
                            "The old image stays in the store. It remains reachable by its digest{}. Workspaces that pinned it keep running it.",
                            if others.is_empty() {
                                String::new()
                            } else {
                                format!(" and by {}", others.join(", "))
                            }
                        ),
                    );
                    small(ui, "Only tags that are not versions can move. The move is a compare-and-swap: if another program changed the tag meanwhile, nothing moves and this question returns with the current digest.");
                });
                ui.horizontal(|ui| {
                    if ui.button("Move tag").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            }
            Prompt::Refuse(current) => {
                // The save-as-image mock-up's stop mark beside the refusal.
                alert(ui, crate::shell_icons::Icon::Refusal, |ui| {
                    ui.label(format!("{reference} already names {}.", short(current)));
                    ui.label(format!(
                        "Tags shaped like a version (x.y.z) never move, so this image cannot use {}. There is no override.",
                        model.tag.trim()
                    ));
                    small(ui, "Use the next version, or a tag that is not a version — such as latest, which can move after a confirmation.");
                });
                ui.horizontal(|ui| {
                    if ui.button(format!("Use {}", model.suggested())).clicked() {
                        answer = Some(false);
                    }
                    if ui.button("Back").clicked() {
                        close = true;
                    }
                });
            }
        }
    });
    if close || ctx.input_mut(crate::shell::escape_pressed) {
        model.dismiss();
        return None;
    }
    match answer {
        Some(true) => model.confirm_move(),
        Some(false) => {
            model.use_next();
            None
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(label: &str) -> Digest {
        Digest::of(label.as_bytes())
    }
    fn tags() -> Vec<(String, Digest)> {
        vec![
            ("open-at:1.0.0".into(), digest("a")),
            ("open-at:latest".into(), digest("a")),
            ("open-at:2.0.0".into(), digest("b")),
            ("ibm-at:1.0.0".into(), digest("c")),
        ]
    }

    #[test]
    fn version_defaults_to_the_next_bump_of_the_source_version_else_one() {
        let form = SaveAsImage::new("open-at:latest", digest("a"), tags());
        assert_eq!(form.repo, "open-at");
        assert_eq!(form.tag, "1.1.0", "the source image is open-at:1.0.0");
        let mut tags = tags();
        tags.push(("open-at:1.1.0".into(), digest("d")));
        let form = SaveAsImage::new("open-at:1.0.0", digest("a"), tags);
        assert_eq!(form.tag, "1.2.0", "1.1.0 is taken");
        let mut form = SaveAsImage::new("open-at:2.0.0", digest("b"), self::tags());
        assert_eq!(form.tag, "2.1.0");
        form.set_repo("open-at-dos".into());
        assert_eq!(form.tag, "1.0.0", "a new name starts at 1.0.0");
        form.set_repo("ibm-at".into());
        assert_eq!(form.tag, "1.1.0", "after that repository's newest version");
        form.set_tag("latest".into());
        form.set_repo("open-at".into());
        assert_eq!(form.tag, "latest", "an edited tag stays");
        let form = SaveAsImage::new("other:custom", digest("z"), self::tags());
        assert_eq!(form.tag, "1.0.0");
    }

    #[test]
    fn a_version_tag_is_refused_with_the_next_free_version() {
        let mut form = SaveAsImage::new("open-at:1.0.0", digest("a"), tags());
        form.set_tag("1.0.0".into());
        let check = form.check();
        assert_eq!(check.level, TagLevel::Bad);
        assert!(check.text.contains("Version tags never move"));
        assert_eq!(form.submit(), None);
        assert_eq!(form.prompt, Some(Prompt::Refuse(digest("a"))));
        form.use_next();
        assert_eq!(form.tag, "1.1.0");
        assert_eq!(form.prompt, None);
        assert_eq!(
            form.submit(),
            Some(Target {
                reference: "open-at:1.1.0".into(),
                expected: None,
                title: None,
                flatten: false,
            })
        );
    }

    #[test]
    fn moving_another_tag_asks_and_swaps_from_the_digest_shown() {
        let mut form = SaveAsImage::new("open-at:1.0.0", digest("a"), tags()).with_flatten();
        form.set_tag("latest".into());
        let check = form.check();
        assert_eq!(check.level, TagLevel::Warn);
        assert!(check.text.contains("Saving will ask before moving it"));
        assert!(check.text.contains("and by open-at:1.0.0"));
        assert_eq!(form.submit(), None);
        assert_eq!(form.prompt, Some(Prompt::Move(digest("a"))));
        form.flatten = Some(true);
        let target = form.confirm_move().unwrap();
        assert_eq!(target.reference, "open-at:latest");
        assert_eq!(target.expected, Some(digest("a")));
        assert!(target.flatten);
        // Editing the tag withdraws the confirmation.
        form.set_tag("lates".into());
        form.set_tag("latest".into());
        assert_eq!(form.submit(), None);
        assert!(matches!(form.prompt, Some(Prompt::Move(_))));
    }

    #[test]
    fn a_refused_save_asks_again_with_the_current_digests() {
        // Another program moved latest after the user confirmed the move.
        let mut form = SaveAsImage::new("open-at:1.0.0", digest("a"), tags());
        form.set_tag("latest".into());
        assert_eq!(form.submit(), None);
        assert!(form.confirm_move().is_some());
        let mut moved = tags();
        moved[1].1 = digest("elsewhere");
        form.refresh(moved);
        assert_eq!(form.prompt, Some(Prompt::Move(digest("elsewhere"))));
        let target = form.confirm_move().unwrap();
        assert_eq!(target.expected, Some(digest("elsewhere")));
        // A version tag that appeared meanwhile is refused, not moved.
        let mut form = SaveAsImage::new("open-at:1.0.0", digest("a"), tags());
        assert_eq!(form.reference(), "open-at:1.1.0");
        assert!(form.submit().is_some());
        let mut taken = tags();
        taken.push(("open-at:1.1.0".into(), digest("raced")));
        form.refresh(taken);
        assert_eq!(form.prompt, Some(Prompt::Refuse(digest("raced"))));
        form.use_next();
        assert_eq!(form.tag, "1.2.0");
    }

    #[test]
    fn a_recovered_session_without_a_tag_starts_from_a_valid_repository() {
        let form = SaveAsImage::new("Recovered 0123456789ab", digest("a"), tags());
        assert_eq!(form.repo, "recovered");
        assert_eq!(form.reference(), "recovered:1.0.0");
        assert_eq!(form.check().level, TagLevel::Ok);
        // A session pinned to an untagged image keeps its repository.
        let detached = format!("open-at@{}", digest("z"));
        let form = SaveAsImage::new(&detached, digest("z"), tags());
        assert_eq!(form.repo, "open-at");
        assert_eq!(form.reference(), "open-at:2.1.0");
    }

    #[test]
    fn invalid_names_and_build_metadata_cannot_save() {
        for (repo, tag) in [
            ("Open-AT", "1.0.0"),
            ("open-at", "1.0.0+build.5"),
            ("open-at", "-x"),
            ("open-at", ""),
        ] {
            let mut form = SaveAsImage::new("open-at:1.0.0", digest("a"), tags());
            form.set_repo(repo.into());
            form.set_tag(tag.into());
            assert_eq!(form.check().level, TagLevel::Bad, "{repo}:{tag}");
            assert_eq!(form.submit(), None);
            assert_eq!(form.prompt, None);
        }
    }

    #[test]
    fn title_is_saved_only_when_changed() {
        let mut form =
            SaveAsImage::new("open-at:1.0.0", digest("a"), tags()).with_title(Some("Open AT"));
        assert_eq!(form.title.as_deref(), Some("Open AT"));
        assert_eq!(form.submit().unwrap().title, None);
        form.title = Some("Open AT with FreeDOS ".into());
        assert_eq!(
            form.submit().unwrap().title.as_deref(),
            Some("Open AT with FreeDOS")
        );
        form.title = Some(String::new());
        assert_eq!(form.submit().unwrap().title.as_deref(), Some(""));
    }

    #[test]
    fn the_form_and_both_questions_render_without_a_window() {
        let ctx = egui::Context::default();
        let mut model = SaveAsImage::new("open-at:1.0.0", digest("a"), tags())
            .with_title(Some("Open AT"))
            .with_flatten();
        for prompt in [
            None,
            Some(Prompt::Move(digest("a"))),
            Some(Prompt::Refuse(digest("a"))),
        ] {
            model.prompt = prompt;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                form(ui, &mut model);
                assert!(prompts(ui.ctx(), &mut model, true).is_none());
            });
            output.textures_delta.clear();
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("Repository name"))));
        }
    }
}
