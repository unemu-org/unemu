// SPDX-License-Identifier: AGPL-3.0-or-later
//! The static checks of recipes §6.11, rules 2 to 10, over a parsed recipe
//! and its work file. Nothing but the recipe, its work file and whether its
//! lock exists is read: no input is opened.
//!
//! Every finding is reported, not only the first. A reference that fails is
//! reported once: the value it names still counts as used (rule 4), a step
//! whose verb is unknown makes no reference unknown, and checks that need a
//! value's size or kind skip a value that could not be resolved.

use crate::finding::{Rule, Sink};
use crate::model::*;
use crate::provider::{CatalogueEntry, Provider, Providers};
use std::collections::{BTreeMap, BTreeSet};

/// The largest bundled input (recipes §6.8, §6.11 rule 9).
pub const MAX_BUNDLE: u64 = 1 << 20;
/// The most inputs, steps, outputs and file-set entries of one recipe
/// (recipes §6.11, rule 9).
pub const MAX_INPUTS: usize = 4096;
pub const MAX_STEPS: usize = 64;
pub const MAX_OUTPUTS: usize = 256;
pub const MAX_FILE_SET_ENTRIES: usize = 100_000;
/// A file under this many bytes has a hash that gives the file away
/// (recipes §6.13).
pub const DISCLOSABLE_BELOW: u64 = 256;

/// The work file a recipe names, as far as it could be read.
pub(crate) enum Work<'a, 's> {
    /// The recipe names no work.
    None,
    /// No `<id>.work.toml` beside the recipe; the file name.
    Missing(String),
    /// The file was refused before its tables could be read; its findings
    /// are reported already.
    Unreadable,
    /// The file is a recipe, not a work file; reported already.
    Recipe,
    /// The work file travels with the recipe in its registry artifact
    /// (recipes §7.1), where the recipe is judged without fetching it; it
    /// is checked when the artifact is laid out beside the recipe.
    Held,
    /// The work file, its findings' sink and its file name.
    Read {
        work: &'a WorkFile,
        sink: &'a mut Sink<'s>,
        file: String,
    },
}

/// The split of rule 5: inputs an output of the artifact depends on, and
/// inputs only an attachment uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Split {
    pub required: Vec<String>,
    pub optional: Vec<String>,
}

/// A file value: what is known about it before any input is read.
#[derive(Clone, Debug, Default)]
struct File {
    size: Option<u64>,
    sha256: Option<String>,
    /// The format and step of a `fat` step that builds it.
    fat: Option<(String, String)>,
}

#[derive(Clone, Debug)]
struct Member {
    path: At<String>,
    file: File,
}

#[derive(Clone, Debug)]
enum Value {
    File(File),
    Tree(Vec<Member>),
    /// A value whose kind is not known: a refused input or step.
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    File,
    Tree,
}

struct Names {
    /// Values by name, in definition order: inputs, then steps.
    values: BTreeMap<String, Value>,
    /// Steps by name and position, to tell a later step from no value.
    steps: BTreeMap<String, usize>,
    /// Names that a reference reached.
    used: BTreeSet<String>,
}

impl Names {
    /// Resolve `reference` read at step `at` (`None` for an output), as a
    /// `want`. A failure is reported, and the value it names is marked as
    /// used so that rule 4 does not report it again; a reference that
    /// resolves marks nothing, as rule 4 follows the outputs.
    fn resolve(
        &mut self,
        sink: &mut Sink<'_>,
        reference: &At<String>,
        at: Option<usize>,
        want: Want,
    ) -> Option<Value> {
        let (name, member) = match reference.value.split_once('/') {
            Some((name, member)) => (name, Some(member)),
            None => (reference.value.as_str(), None),
        };
        let mut fail = |sink: &mut Sink<'_>, message: String| {
            sink.report(Rule::Reference, &reference.span, &reference.path, message);
            self.used.insert(name.to_owned());
        };
        if let (Some(step), Some(at)) = (self.steps.get(name), at) {
            if *step == at {
                fail(sink, format!("the step `{name}` cannot read its own value"));
                return None;
            }
            if *step > at {
                fail(
                    sink,
                    format!("`{name}` is a later step; a step reads only inputs and earlier steps"),
                );
                return None;
            }
        }
        let Some(value) = self.values.get(name).cloned() else {
            fail(sink, format!("no input or step is named `{name}`"));
            return None;
        };
        let value = match (value, member) {
            // A refused value: nothing can be said of a reference to it.
            (Value::Unknown, _) => {
                self.used.insert(name.to_owned());
                return None;
            }
            (value, None) => value,
            (Value::File(_), Some(member)) => {
                fail(
                    sink,
                    format!("`{name}` is a file; it has no member `{member}`"),
                );
                return None;
            }
            (Value::Tree(members), Some(member)) => {
                match members.into_iter().find(|m| m.path.value == member) {
                    Some(found) => Value::File(found.file),
                    None => {
                        fail(sink, format!("the tree `{name}` has no member `{member}`"));
                        return None;
                    }
                }
            }
        };
        match (&value, want) {
            (Value::File(_), Want::File) | (Value::Tree(_), Want::Tree) => Some(value),
            (Value::Tree(_), Want::File) => {
                fail(
                    sink,
                    format!("`{name}` is a tree; a file is wanted here, such as `{name}/PATH`"),
                );
                None
            }
            (Value::File(_), Want::Tree) => {
                fail(sink, format!("`{name}` is a file; a tree is wanted here"));
                None
            }
            (Value::Unknown, _) => None,
        }
    }
}

fn base(reference: &str) -> &str {
    reference
        .split_once('/')
        .map_or(reference, |(name, _)| name)
}

/// Run rules 2 to 10 on `recipe`. `lock` says whether the recipe's lock
/// directory exists beside it.
pub(crate) fn check(
    recipe: &Recipe,
    sink: &mut Sink<'_>,
    mut work: Work<'_, '_>,
    providers: Providers<'_>,
    lock: bool,
) -> Split {
    kind(recipe, sink, &work);
    if let (Some(work), Some(name)) = (&recipe.work, &recipe.identity.name) {
        let last = name.value.rsplit('/').next().unwrap_or_default();
        if last != work.value {
            sink.report(
                Rule::Reference,
                &work.span,
                &work.path,
                format!(
                    "the work is `{}`, so the repository's last component is `{}`, not `{last}`: the work is the repository",
                    work.value, work.value
                ),
            );
        }
    }
    if let Work::Missing(file) = &work {
        if let Some(name) = &recipe.work {
            sink.report(
                Rule::Reference,
                &name.span,
                &name.path,
                format!("no work file {file} beside the recipe"),
            );
        }
    }
    if let Work::Read {
        work: file,
        sink: work_sink,
        file: file_name,
    } = &mut work
    {
        if let (Some(id), Some(named)) = (&file.id, &recipe.work) {
            if id.value != named.value {
                work_sink.report(
                    Rule::Reference,
                    &id.span,
                    &id.path,
                    format!(
                        "the id is `{}`, but {file_name} is the work file of `{}`",
                        id.value, named.value
                    ),
                );
            }
        }
        if let (Some(mine), Some(its)) = (&recipe.version, &file.version) {
            // Version 1.1 adds no work-file keys, so a 1.0 work file means the
            // same to a 1.1 recipe: one work may hold 1.0 and 1.1 releases.
            let older_work = mine.value == "1.1" && its.value == "1.0";
            if mine.value != its.value && !older_work {
                let rule = if mine.value == "1.1" {
                    "a 1.1 recipe names a work file of version 1.0 or 1.1"
                } else {
                    "a recipe names a work file of its own recipe version only"
                };
                sink.report(
                    Rule::Reference,
                    &mine.span,
                    &mine.path,
                    format!(
                        "the recipe is {} and its work file {file_name} is {}; {rule}",
                        mine.value, its.value
                    ),
                );
            }
        }
    }
    let mut names = names(recipe, sink);
    let split = references(recipe, sink, &mut names);
    unused(recipe, sink, &names);
    let provider = provider(recipe, sink, providers);
    license(recipe, sink, &mut names, provider);
    if let Some(provider) = provider {
        vocabulary(recipe, sink, provider);
        if let Work::Read {
            work: file,
            sink: work_sink,
            ..
        } = &mut work
        {
            work_vocabulary(file, work_sink, provider);
        }
        platform_rules(recipe, sink, &mut names, provider);
    }
    lock_rule(recipe, sink, lock);
    limits(recipe, sink);
    bundled(recipe, sink);
    split
}

/// Rule 3, the bundle: two bundled inputs with the same bytes, or the same
/// name, cannot both travel with the recipe, as its artifact names each
/// blob once and lays each file out at its name beside the recipe
/// (recipes §7.1).
fn bundled(recipe: &Recipe, sink: &mut Sink<'_>) {
    let mut by_hash: BTreeMap<&str, &str> = BTreeMap::new();
    let mut by_name: BTreeMap<&str, &str> = BTreeMap::new();
    for input in &recipe.inputs {
        let (
            Some(bundle),
            InputForm::File {
                sha256: Some(sha256),
                ..
            },
        ) = (&input.bundle, &input.form)
        else {
            continue;
        };
        if !bundle.value {
            continue;
        }
        if let Some(first) = by_hash.insert(&sha256.value, &input.id.value) {
            sink.report(
                Rule::Reference,
                &bundle.span,
                &bundle.path,
                format!(
                    "input.{first} is bundled with the same bytes; a recipe bundles a file once, so name it from one input"
                ),
            );
        }
        if let Some(name) = &input.name {
            if let Some(first) = by_name.insert(&name.value, &input.id.value) {
                sink.report(
                    Rule::Reference,
                    &bundle.span,
                    &bundle.path,
                    format!(
                        "input.{first} is bundled as {} too; two bundled files beside the recipe cannot share a name",
                        name.value
                    ),
                );
            }
        }
    }
}

/// Rule 3, the bundle and the work file: a bundled input with the bytes of
/// the work file the recipe names, `work_sha256`, would be the same blob
/// twice in the recipe's artifact.
pub(crate) fn bundled_work(recipe: &Recipe, sink: &mut Sink<'_>, work_sha256: &str, file: &str) {
    for input in &recipe.inputs {
        let (
            Some(bundle),
            InputForm::File {
                sha256: Some(sha256),
                ..
            },
        ) = (&input.bundle, &input.form)
        else {
            continue;
        };
        if bundle.value && sha256.value == work_sha256 {
            sink.report(
                Rule::Reference,
                &bundle.span,
                &bundle.path,
                format!(
                    "the bundled input has the bytes of the work file {file}, which travels with the recipe already"
                ),
            );
        }
    }
}

/// Rule 2, the parts the parser cannot see: the tables a kind requires, the
/// title of the recipe and its work file together, and the components of
/// the repository that name the kind and the platform. Also the cross-key
/// values of rule 1.
fn kind(recipe: &Recipe, sink: &mut Sink<'_>, work: &Work<'_, '_>) {
    let at = &recipe.identity.at;
    match recipe.kind {
        Kind::Machine => machine(recipe, sink),
        Kind::Media => {
            if recipe.media.is_empty() {
                sink.report(
                    Rule::Kind,
                    &at.span,
                    &at.path,
                    "a Mediafile has at least one [[medium]]",
                );
            }
            let own = recipe.title.as_ref().is_some_and(|t| t.title.is_some());
            let shared = match work {
                Work::Read { work, .. } => work.title.as_ref().is_some_and(|t| t.title.is_some()),
                // The work file's refusal is reported; do not add to it.
                Work::Missing(_) | Work::Unreadable | Work::Held => true,
                Work::None | Work::Recipe => false,
            };
            if !own && !shared {
                let (span, path) = recipe
                    .title
                    .as_ref()
                    .map_or((&at.span, at.path.as_str()), |t| {
                        (&t.at.span, t.at.path.as_str())
                    });
                let message = if recipe.work.is_some() {
                    "no title: neither [title] nor the work file states title"
                } else {
                    "no title: a Mediafile states [title] with title, or names a work file that does"
                };
                sink.report(Rule::Kind, span, path, message);
            }
        }
        Kind::Firmware => {
            if recipe.package.is_none() {
                sink.report(
                    Rule::Kind,
                    &at.span,
                    &at.path,
                    "a Firmwarefile has [package]",
                );
            }
            if recipe.roms.is_empty() {
                sink.report(
                    Rule::Kind,
                    &at.span,
                    &at.path,
                    "a Firmwarefile has at least one [[rom]]",
                );
            }
        }
    }
    if let Some(name) = &recipe.identity.name {
        let parts: Vec<&str> = name.value.split('/').collect();
        if recipe.version.as_ref().is_some_and(|v| v.value == "1.1") && parts.len() >= 4 {
            let kind = parts[parts.len() - 3];
            let wanted = match recipe.kind {
                Kind::Machine => "machines",
                _ => recipe.kind.key(),
            };
            if kind != wanted {
                sink.report(Rule::Kind, &name.span, &name.path,
                    format!("the third component from the end is `{kind}`; the output of [{}] is named NAMESPACE/{wanted}/PLATFORM/ID", recipe.kind.key()));
            }
            if let Some(platform) = &recipe.identity.platform {
                let found = parts[parts.len() - 2];
                if found != platform.value {
                    sink.report(Rule::Kind, &name.span, &name.path,
                        format!("the second component from the end is `{found}`; it is the platform, `{}`", platform.value));
                }
            }
        } else if parts.len() == 4 {
            if parts[1] != recipe.kind.key() {
                sink.report(
                    Rule::Kind,
                    &name.span,
                    &name.path,
                    format!(
                        "the second component is `{}`; the output of [{}] is named NAMESPACE/{}/PLATFORM/ID",
                        parts[1],
                        recipe.kind.key(),
                        recipe.kind.key()
                    ),
                );
            }
            if let Some(platform) = &recipe.identity.platform {
                if parts[2] != platform.value {
                    sink.report(
                        Rule::Kind,
                        &name.span,
                        &name.path,
                        format!(
                            "the third component is `{}`; it is the platform, `{}`",
                            parts[2], platform.value
                        ),
                    );
                }
            }
        }
    }
    if let Some(title) = &recipe.title {
        players(title, sink);
    }
    for input in &recipe.inputs {
        match &input.form {
            InputForm::File { size, .. } => {
                disclosable(sink, input.disclosable.as_ref(), size.as_ref());
            }
            InputForm::FileSet { files } => {
                for entry in files {
                    disclosable(sink, entry.disclosable.as_ref(), entry.size.as_ref());
                }
            }
            InputForm::Refused => {}
        }
    }
    for step in &recipe.steps {
        if let Op::Unpack { members, .. } = &step.op {
            for member in members {
                disclosable(sink, member.disclosable.as_ref(), member.size.as_ref());
            }
        }
    }
}

fn machine(recipe: &Recipe, sink: &mut Sink<'_>) {
    let Some(template) = &recipe.template else {
        return;
    };
    if let Some(config) = &template.config {
        if !recipe.inputs.iter().any(|input| {
            input.id.value == config.value && matches!(input.form, InputForm::File { .. })
        }) {
            sink.report(
                Rule::Reference,
                &config.span,
                &config.path,
                "template config must name exactly one file input, not a step, file set or member",
            );
        }
    }
    let mut ids = BTreeSet::new();
    for id in template.disks.iter().filter_map(|disk| disk.id.as_ref()) {
        if !ids.insert(&id.value) {
            sink.report(
                Rule::Reference,
                &id.span,
                &id.path,
                format!("the disk id `{}` is declared twice", id.value),
            );
        }
    }
}

fn disclosable(sink: &mut Sink<'_>, flag: Option<&At<bool>>, size: Option<&At<u64>>) {
    if let (Some(flag), Some(size)) = (flag, size) {
        if flag.value && size.value >= DISCLOSABLE_BELOW {
            sink.report(
                Rule::Syntax,
                &flag.span,
                &flag.path,
                format!(
                    "disclosable is for a file under {DISCLOSABLE_BELOW} bytes; this one is {}",
                    size.value
                ),
            );
        }
    }
}

/// `players.min` at most `players.max` (rule 1).
pub(crate) fn players(title: &Title, sink: &mut Sink<'_>) {
    if let Some(players) = &title.players {
        if players.value.max < players.value.min {
            sink.report(
                Rule::Syntax,
                &players.span,
                &players.path,
                format!(
                    "max is {} and min is {}; max is at least min",
                    players.value.max, players.value.min
                ),
            );
        }
    }
}

/// The values of rule 3: inputs and steps in one namespace, each step's
/// operand resolved in order. Duplicate names and paths are reported here.
fn names(recipe: &Recipe, sink: &mut Sink<'_>) -> Names {
    let mut names = Names {
        values: BTreeMap::new(),
        steps: BTreeMap::new(),
        used: BTreeSet::new(),
    };
    for input in &recipe.inputs {
        let value = match &input.form {
            InputForm::File { size, sha256 } => Value::File(File {
                size: size.as_ref().map(|s| s.value),
                sha256: sha256.as_ref().map(|s| s.value.clone()),
                fat: None,
            }),
            InputForm::FileSet { files } => {
                let mut seen = BTreeSet::new();
                let mut members = Vec::new();
                for entry in files {
                    if !seen.insert(entry.path.value.clone()) {
                        sink.report(
                            Rule::Reference,
                            &entry.path.span,
                            &entry.path.path,
                            format!("the path `{}` is listed twice", entry.path.value),
                        );
                        continue;
                    }
                    members.push(Member {
                        path: entry.path.clone(),
                        file: File {
                            size: entry.size.as_ref().map(|s| s.value),
                            sha256: entry.sha256.as_ref().map(|s| s.value.clone()),
                            fat: None,
                        },
                    });
                }
                Value::Tree(members)
            }
            InputForm::Refused => Value::Unknown,
        };
        names.values.insert(input.id.value.clone(), value);
    }
    for (index, step) in recipe.steps.iter().enumerate() {
        let Some(id) = &step.id else { continue };
        if names.values.contains_key(&id.value) {
            sink.report(
                Rule::Reference,
                &id.span,
                &id.path,
                format!(
                    "the value name `{}` is used twice; inputs and steps share one namespace",
                    id.value
                ),
            );
            continue;
        }
        names.steps.insert(id.value.clone(), index);
        // Placeholder until the step is resolved in order.
        names.values.insert(id.value.clone(), Value::Unknown);
    }
    names
}

/// Rule 3: every reference, in order, and the output ids; returns the split
/// of rule 5.
fn references(recipe: &Recipe, sink: &mut Sink<'_>, names: &mut Names) -> Split {
    if let Some(config) = recipe.template.as_ref().and_then(|t| t.config.as_ref()) {
        names.used.insert(config.value.clone());
    }
    for (index, step) in recipe.steps.iter().enumerate() {
        let value = match &step.op {
            Op::Unpack {
                archive, members, ..
            } => {
                if let Some(archive) = archive {
                    names.resolve(sink, archive, Some(index), Want::File);
                }
                let mut seen = BTreeSet::new();
                let mut tree = Vec::new();
                for member in members {
                    if !seen.insert(member.path.value.clone()) {
                        sink.report(
                            Rule::Reference,
                            &member.path.span,
                            &member.path.path,
                            format!("the member `{}` is listed twice", member.path.value),
                        );
                        continue;
                    }
                    tree.push(Member {
                        path: member.path.clone(),
                        file: File {
                            size: member.size.as_ref().map(|s| s.value),
                            sha256: member.sha256.as_ref().map(|s| s.value.clone()),
                            fat: None,
                        },
                    });
                }
                Value::Tree(tree)
            }
            Op::Fat {
                tree,
                placements,
                format,
                volumes,
                ..
            } => {
                if let Some(tree) = tree {
                    names.resolve(sink, tree, Some(index), Want::Tree);
                }
                for placement in placements.iter().flatten() {
                    if let Some(from) = &placement.from {
                        names.used.insert(base(&from.value).to_owned());
                        if !recipe.inputs.iter().any(|input| {
                            input.id.value == from.value
                                && matches!(input.form, InputForm::File { .. })
                        }) {
                            sink.report(Rule::Reference, &from.span, &from.path,
                                "a FAT placement must name exactly one file input, not a step, file set or member");
                        }
                    }
                }
                let fat = match (format, &step.id) {
                    (Some(format), Some(id)) => Some((format.value.clone(), id.value.clone())),
                    _ => None,
                };
                let file = File {
                    size: None,
                    sha256: None,
                    fat,
                };
                match volumes {
                    None => Value::File(file),
                    Some(count) => Value::Tree(
                        (1..=count.value)
                            .map(|n| Member {
                                path: At {
                                    value: n.to_string(),
                                    span: count.span.clone(),
                                    path: count.path.clone(),
                                },
                                file: file.clone(),
                            })
                            .collect(),
                    ),
                }
            }
            Op::Text { text } => {
                if let Some(text) = text {
                    names.resolve(sink, text, Some(index), Want::File);
                }
                Value::File(File::default())
            }
            Op::Unknown { references } => {
                for reference in references {
                    names.used.insert(base(&reference.value).to_owned());
                }
                Value::Unknown
            }
        };
        if let Some(id) = &step.id {
            if names.steps.get(&id.value) == Some(&index) {
                names.values.insert(id.value.clone(), value);
            }
        }
    }
    // Outputs: the artifact's own, then the attachments'.
    let mut outputs: BTreeMap<String, ()> = BTreeMap::new();
    let mut output = |sink: &mut Sink<'_>, id: &Option<At<String>>| {
        if let Some(id) = id {
            if outputs.insert(id.value.clone(), ()).is_some() {
                sink.report(
                    Rule::Reference,
                    &id.span,
                    &id.path,
                    format!(
                        "the output id `{}` is used twice; media, ROMs, extras and attachments share one namespace",
                        id.value
                    ),
                );
            }
        }
    };
    for medium in &recipe.media {
        output(sink, &medium.id);
        if let Some(from) = &medium.from {
            names.resolve(sink, from, None, Want::File);
        }
    }
    for rom in &recipe.roms {
        output(sink, &rom.id);
        match &rom.source {
            RomSource::File(Some(from)) => {
                names.resolve(sink, from, None, Want::File);
            }
            RomSource::File(None) => {}
            RomSource::Parts { parts, .. } => {
                for part in parts {
                    if let Some(from) = &part.from {
                        names.resolve(sink, from, None, Want::File);
                    }
                }
            }
        }
    }
    for extra in &recipe.extras {
        output(sink, &extra.id);
        if let Some(from) = &extra.from {
            names.resolve(sink, from, None, Want::File);
        }
    }
    dos_names(sink, &recipe.extras);
    let required = closure(recipe, names, &own_roots(recipe));
    let mut attachment_roots = Vec::new();
    for attachment in &recipe.attachments {
        output(sink, &attachment.id);
        for extra in &attachment.extras {
            output(sink, &extra.id);
            if let Some(from) = &extra.from {
                names.resolve(sink, from, None, Want::File);
                attachment_roots.push(base(&from.value).to_owned());
            }
        }
        dos_names(sink, &attachment.extras);
    }
    let optional = closure(recipe, names, &attachment_roots);
    let inputs = |set: &BTreeSet<String>| -> Vec<String> {
        recipe
            .inputs
            .iter()
            .filter(|i| set.contains(&i.id.value))
            .map(|i| i.id.value.clone())
            .collect()
    };
    Split {
        required: inputs(&required),
        optional: inputs(
            &optional
                .difference(&required)
                .cloned()
                .collect::<BTreeSet<_>>(),
        ),
    }
}

fn dos_names(sink: &mut Sink<'_>, extras: &[Extra]) {
    let mut seen = BTreeSet::new();
    for name in extras.iter().filter_map(|e| e.dos_name.as_ref()) {
        if !seen.insert(name.value.clone()) {
            sink.report(
                Rule::Reference,
                &name.span,
                &name.path,
                format!("the DOS name {} is used twice", name.value),
            );
        }
    }
}

/// The value names the artifact's own outputs reference.
fn own_roots(recipe: &Recipe) -> Vec<String> {
    let mut roots = Vec::new();
    if let Some(config) = recipe.template.as_ref().and_then(|t| t.config.as_ref()) {
        roots.push(config.value.clone());
    }
    let mut add = |r: &Option<At<String>>| {
        if let Some(r) = r {
            roots.push(base(&r.value).to_owned());
        }
    };
    for medium in &recipe.media {
        add(&medium.from);
    }
    for rom in &recipe.roms {
        match &rom.source {
            RomSource::File(from) => add(from),
            RomSource::Parts { parts, .. } => parts.iter().for_each(|p| add(&p.from)),
        }
    }
    for extra in &recipe.extras {
        add(&extra.from);
    }
    roots
}

/// Every value name reachable from `roots` through the steps.
fn closure(recipe: &Recipe, names: &Names, roots: &[String]) -> BTreeSet<String> {
    let mut reached: BTreeSet<String> = roots.iter().cloned().collect();
    for step in recipe.steps.iter().rev() {
        let Some(id) = &step.id else { continue };
        if !reached.contains(&id.value) || !names.steps.contains_key(&id.value) {
            continue;
        }
        reached.extend(operands(step).into_iter().map(|r| base(r).to_owned()));
    }
    reached
}

fn operands(step: &Step) -> Vec<&str> {
    match &step.op {
        Op::Fat {
            tree, placements, ..
        } => tree
            .iter()
            .map(|r| r.value.as_str())
            .chain(
                placements
                    .iter()
                    .flatten()
                    .filter_map(|p| p.from.as_ref().map(|r| r.value.as_str())),
            )
            .collect(),
        Op::Unpack { archive: r, .. } | Op::Text { text: r } => {
            r.iter().map(|r| r.value.as_str()).collect()
        }
        Op::Unknown { references } => references.iter().map(|r| r.value.as_str()).collect(),
    }
}

/// Rule 4: an input or a step no output uses.
fn unused(recipe: &Recipe, sink: &mut Sink<'_>, names: &Names) {
    let mut roots = own_roots(recipe);
    for attachment in &recipe.attachments {
        for extra in &attachment.extras {
            if let Some(from) = &extra.from {
                roots.push(base(&from.value).to_owned());
            }
        }
    }
    let mut used = closure(recipe, names, &roots);
    // A value a failed reference named, or one behind a step whose verb is
    // unknown, is not reported again.
    used.extend(names.used.iter().cloned());
    for step in &recipe.steps {
        if matches!(step.op, Op::Unknown { .. }) {
            used.extend(operands(step).into_iter().map(|r| base(r).to_owned()));
        }
    }
    for input in &recipe.inputs {
        if !used.contains(&input.id.value) {
            sink.report(
                Rule::Unused,
                &input.id.span,
                &input.id.path,
                format!("the input `{}` is not used by any output", input.id.value),
            );
        }
    }
    for (index, step) in recipe.steps.iter().enumerate() {
        let Some(id) = &step.id else { continue };
        if matches!(step.op, Op::Unknown { .. })
            || names.steps.get(&id.value) != Some(&index)
            || used.contains(&id.value)
        {
            continue;
        }
        sink.report(
            Rule::Unused,
            &id.span,
            &id.path,
            format!("the step `{}` is not used by any output", id.value),
        );
    }
}

/// Rule 6: the open statement of recipes §6.6.
fn license(
    recipe: &Recipe,
    sink: &mut Sink<'_>,
    names: &mut Names,
    provider: Option<&dyn Provider>,
) {
    let Some(license) = &recipe.license else {
        if let Some(name) = &recipe.identity.name {
            if name.value.starts_with("unemu/") {
                sink.report(
                    Rule::License,
                    &name.span,
                    &name.path,
                    "a recipe without [license] is user-supplied and names its output under private/, not unemu/",
                );
            }
        }
        return;
    };
    let extras: BTreeSet<&str> = recipe
        .extras
        .iter()
        .filter_map(|e| e.id.as_ref().map(|i| i.value.as_str()))
        .collect();
    for (key, value) in [("terms", &license.terms), ("source", &license.source)] {
        if let Some(value) = value {
            if !extras.contains(value.value.as_str()) {
                sink.report(
                    Rule::License,
                    &value.span,
                    &value.path,
                    format!("{key} names `{}`, which is not an [[extra]]", value.value),
                );
            }
        }
    }
    let at = &license.at;
    let expression =
        license
            .expression
            .as_ref()
            .and_then(
                |text| match image_store::license::Expression::parse(&text.value) {
                    Ok(expression) => Some(expression),
                    Err(error) => {
                        sink.report(Rule::License, &text.span, &text.path, message(error));
                        None
                    }
                },
            );
    match recipe.kind {
        Kind::Machine => {}
        Kind::Media => {
            if license.terms.is_none() {
                sink.report(
                    Rule::License,
                    &at.span,
                    &at.path,
                    "an open set names its licence text: terms is required",
                );
            }
            if let Some(expression) = &expression {
                if !expression.is_reference() && license.source.is_none() {
                    sink.report(
                        Rule::License,
                        &at.span,
                        &at.path,
                        format!(
                            "an open set under {expression} carries its source: source is required unless the expression is one LicenseRef-"
                        ),
                    );
                }
            }
        }
        Kind::Firmware => {
            if let Some(expression) = &expression {
                if expression.needs_source(image_store::license::ACTIVE) && license.source.is_none()
                {
                    sink.report(
                        Rule::License,
                        &at.span,
                        &at.path,
                        format!("{expression} is copyleft: the package carries its source, so source is required"),
                    );
                }
            }
            let Some(provider) = provider else { return };
            for rom in &recipe.roms {
                let bytes = rom_bytes(rom, names, sink);
                for (id, entry) in rom_entries(rom, &bytes, provider, recipe.package.as_ref()) {
                    match (&entry.license, &expression) {
                        (None, _) => sink.report(
                            Rule::License,
                            &at.span,
                            &at.path,
                            format!(
                                "the firmware catalogue lists `{id}` as user-supplied; its package takes no [license]"
                            ),
                        ),
                        (Some(listed), Some(expression)) => {
                            let same = image_store::license::Expression::parse(listed)
                                .is_ok_and(|listed| listed.as_str() == expression.as_str());
                            if let (false, Some(text)) = (same, &license.expression) {
                                sink.report(
                                    Rule::License,
                                    &text.span,
                                    &text.path,
                                    format!(
                                        "the firmware catalogue records {listed} for `{id}`; the expression is that one"
                                    ),
                                );
                            }
                        }
                        (Some(_), None) => {}
                    }
                }
            }
        }
    }
}

/// What the recipe states of a ROM's bytes: the logical ROM's size and
/// SHA-256 with where each is written, and the size and SHA-256 of each
/// chip.
struct RomBytes {
    size: Option<At<u64>>,
    sha256: Option<At<String>>,
    chips: Vec<(u64, String)>,
}

fn rom_bytes(rom: &Rom, names: &mut Names, sink: &Sink<'_>) -> RomBytes {
    let mut quiet = Sink::new(sink.source);
    let mut file = |names: &mut Names, from: &At<String>| match names.resolve(
        &mut quiet,
        from,
        None,
        Want::File,
    ) {
        Some(Value::File(file)) => Some(file),
        _ => None,
    };
    match &rom.source {
        RomSource::File(from) => {
            let found = from
                .as_ref()
                .and_then(|from| Some((from, file(names, from)?)));
            RomBytes {
                size: found.as_ref().and_then(|(from, f)| {
                    Some(At {
                        value: f.size?,
                        span: from.span.clone(),
                        path: from.path.clone(),
                    })
                }),
                sha256: found.as_ref().and_then(|(from, f)| {
                    Some(At {
                        value: f.sha256.clone()?,
                        span: from.span.clone(),
                        path: from.path.clone(),
                    })
                }),
                chips: Vec::new(),
            }
        }
        RomSource::Parts {
            parts,
            size,
            sha256,
        } => RomBytes {
            size: size.as_ref().and_then(|s| {
                Some(At {
                    value: u64::try_from(s.value).ok()?,
                    span: s.span.clone(),
                    path: s.path.clone(),
                })
            }),
            sha256: sha256.clone(),
            chips: parts
                .iter()
                .filter_map(|p| {
                    let chip = file(names, p.from.as_ref()?)?;
                    Some((chip.size?, chip.sha256?))
                })
                .collect(),
        },
    }
}

/// The catalogue entries a ROM is: the one its `catalogue_id` names, or,
/// without one, those its stated bytes are, as a whole or by a chip.
fn rom_entries(
    rom: &Rom,
    bytes: &RomBytes,
    provider: &dyn Provider,
    package: Option<&crate::model::Package>,
) -> Vec<(String, CatalogueEntry)> {
    let firmware = provider.firmware();
    let scoped = firmware.for_package(package);
    let firmware = scoped.as_deref().unwrap_or(firmware);
    if let Some(id) = &rom.catalogue_id {
        return firmware
            .entry(&id.value)
            .map(|e| (id.value.clone(), e))
            .into_iter()
            .collect();
    }
    let mut found: Vec<(String, CatalogueEntry)> = Vec::new();
    let whole = bytes
        .size
        .as_ref()
        .zip(bytes.sha256.as_ref())
        .map(|(size, sha)| (size.value, sha.value.clone()));
    for (size, sha256) in whole.into_iter().chain(bytes.chips.iter().cloned()) {
        if let Some((id, entry)) = firmware.identify(&sha256, size) {
            if found.iter().all(|(known, _)| *known != id) {
                found.push((id, entry));
            }
        }
    }
    found
}

fn message(error: image_store::Error) -> String {
    match error {
        image_store::Error::Invalid(message) => message,
        other => other.to_string(),
    }
}

/// Rule 7, the platform itself: the provider of the recipe, or a finding.
fn provider<'p>(
    recipe: &Recipe,
    sink: &mut Sink<'_>,
    providers: Providers<'p>,
) -> Option<&'p dyn Provider> {
    let platform = recipe.identity.platform.as_ref()?;
    let canonical = image_store::platform::canonical(&platform.value);
    if canonical != platform.value {
        sink.report(
            Rule::Provider,
            &platform.span,
            &platform.path,
            format!(
                "`{}` is a retired platform id; a recipe names its platform {canonical}",
                platform.value
            ),
        );
        return None;
    }
    let found = providers.get(&platform.value);
    if found.is_none() {
        sink.report(
            Rule::Provider,
            &platform.span,
            &platform.path,
            match providers.platforms().as_slice() {
                [] => format!("no provider of platform `{}` in this build", platform.value),
                known => format!(
                    "no provider of platform `{}` in this build; it has {}",
                    platform.value,
                    known.join(", ")
                ),
            },
        );
    }
    found
}

fn one_of(
    sink: &mut Sink<'_>,
    value: &At<String>,
    known: &[&str],
    what: &str,
    provider: &dyn Provider,
) {
    if !known.contains(&value.value.as_str()) {
        sink.report(
            Rule::Provider,
            &value.span,
            &value.path,
            format!(
                "`{}` is not {what} of {}: one of {}",
                value.value,
                provider.platform(),
                known.join(", ")
            ),
        );
    }
}

/// Rule 7: formats, roles, lanes, boards, needs, software platforms, ids,
/// catalogue entries and DOS names the provider does not know.
fn vocabulary(recipe: &Recipe, sink: &mut Sink<'_>, provider: &dyn Provider) {
    let media = provider.media();
    for medium in &recipe.media {
        if let Some(format) = &medium.format {
            one_of(sink, format, media.formats(), "a media format", provider);
        }
    }
    let volume_formats: Vec<&str> = media
        .formats()
        .iter()
        .copied()
        .filter(|f| media.volume_profile(f).is_some())
        .collect();
    for step in &recipe.steps {
        if let Op::Fat {
            format: Some(format),
            ..
        } = &step.op
        {
            one_of(
                sink,
                format,
                &volume_formats,
                "a format with a FAT volume profile",
                provider,
            );
        }
    }
    let firmware = provider.firmware();
    let scoped = firmware.for_package(recipe.package.as_ref());
    let firmware = scoped.as_deref().unwrap_or(firmware);
    for rom in &recipe.roms {
        if let Some(role) = &rom.role {
            one_of(sink, role, firmware.roles(), "a ROM role", provider);
        }
        if let Some(id) = &rom.catalogue_id {
            if firmware.entry(&id.value).is_none() {
                sink.report(
                    Rule::Provider,
                    &id.span,
                    &id.path,
                    format!(
                        "the firmware catalogue of {} has no entry `{}`",
                        provider.platform(),
                        id.value
                    ),
                );
            }
        }
        if let RomSource::Parts { parts, .. } = &rom.source {
            for lane in parts.iter().filter_map(|p| p.lane.as_ref()) {
                one_of(sink, lane, firmware.lanes(), "a chip lane", provider);
            }
        }
    }
    if let Some(package) = &recipe.package {
        for board in &package.boards {
            one_of(sink, board, firmware.boards(), "a board", provider);
        }
        for class in &package.machine_classes {
            one_of(
                sink,
                class,
                firmware.machine_classes(),
                "a machine class",
                provider,
            );
        }
    }
    for extra in recipe
        .extras
        .iter()
        .chain(recipe.attachments.iter().flat_map(|a| &a.extras))
    {
        if let Some(name) = &extra.dos_name {
            if let Some(fault) = media.dos_name_fault(&name.value) {
                sink.report(
                    Rule::Provider,
                    &name.span,
                    &name.path,
                    format!("{}: {fault}", name.value),
                );
            }
        }
    }
    description(
        sink,
        provider,
        recipe.title.as_ref(),
        &recipe.ids,
        &recipe.needs,
    );
}

/// Rule 7 for the parts of a work file, against the provider of a recipe
/// that names it.
fn work_vocabulary(work: &WorkFile, sink: &mut Sink<'_>, provider: &dyn Provider) {
    description(sink, provider, work.title.as_ref(), &work.ids, &work.needs);
}

fn description(
    sink: &mut Sink<'_>,
    provider: &dyn Provider,
    title: Option<&Title>,
    ids: &[WorkId],
    needs: &[Need],
) {
    if let Some(platform) = title.and_then(|t| t.software_platform.as_ref()) {
        one_of(
            sink,
            platform,
            provider.software_platforms(),
            "a software platform",
            provider,
        );
    }
    for id in ids {
        one_of(
            sink,
            &id.catalogue,
            provider.work_id_catalogues(),
            "a catalogue of work ids",
            provider,
        );
    }
    if needs.is_empty() {
        return;
    }
    let Some(vocabulary) = provider.needs() else {
        if let Some(first) = needs.first() {
            sink.report(
                Rule::Provider,
                &first.kind.span,
                &first.kind.path,
                format!("platform {} has no needs vocabulary", provider.platform()),
            );
        }
        return;
    };
    for need in needs {
        if need.values.is_empty() {
            continue;
        }
        let one: image_store::needs::Needs = [(
            need.kind.value.clone(),
            need.values.iter().map(|v| v.value.clone()).collect(),
        )]
        .into();
        if let Err(error) = image_store::needs::check(&one, vocabulary) {
            // Report at the value the vocabulary refused, when one is named.
            let message = message(error);
            let at = need
                .values
                .iter()
                .find(|v| message.contains(&format!("{:?}", v.value)))
                .map_or((&need.kind.span, &need.kind.path), |v| (&v.span, &v.path));
            sink.report(Rule::Provider, at.0, at.1, message);
        }
    }
}

/// Rule 10: sizes the format refuses, a `fat` step of another format than
/// its medium, names a volume refuses, and a ROM that differs from its
/// catalogue entry.
fn platform_rules(
    recipe: &Recipe,
    sink: &mut Sink<'_>,
    names: &mut Names,
    provider: &dyn Provider,
) {
    let media = provider.media();
    let mut quiet = Sink::new(sink.source);
    for medium in &recipe.media {
        let (Some(format), Some(from)) = (&medium.format, &medium.from) else {
            continue;
        };
        if !media.formats().contains(&format.value.as_str()) {
            continue;
        }
        let Some(Value::File(file)) = names.resolve(&mut quiet, from, None, Want::File) else {
            continue;
        };
        if let Some(size) = file.size {
            if let Some(fault) = media.size_fault(&format.value, size) {
                sink.report(
                    Rule::Platform,
                    &format.span,
                    &format.path,
                    format!("`{}` is {size} bytes; {fault}", from.value),
                );
            }
        }
        if let Some((fat_format, step)) = &file.fat {
            if *fat_format != format.value {
                sink.report(
                    Rule::Platform,
                    &format.span,
                    &format.path,
                    format!(
                        "the medium is {}, but the fat step `{step}` builds a {fat_format} volume",
                        format.value
                    ),
                );
            }
        }
    }
    for (index, step) in recipe.steps.iter().enumerate() {
        let Op::Fat {
            tree,
            placements,
            format: Some(format),
            label,
            ..
        } = &step.op
        else {
            continue;
        };
        if tree.is_none() && placements.is_none() {
            continue;
        }
        let Some(profile) = media.volume_profile(&format.value) else {
            continue;
        };
        if let Some(label) = label {
            if let Some(fault) = profile.label_fault(&label.value) {
                sink.report(
                    Rule::Platform,
                    &label.span,
                    &label.path,
                    format!(
                        "the {} volume profile refuses the label: {fault}",
                        format.value
                    ),
                );
            }
        }
        let mut stored = BTreeSet::new();
        for name in placements
            .iter()
            .flatten()
            .filter_map(|p| p.dos_name.as_ref())
        {
            let fault = if name.value.contains(['/', '\\']) {
                Some("expected one root-level DOS 8.3 name".to_owned())
            } else {
                profile.path_fault(&name.value)
            };
            if let Some(fault) = fault {
                sink.report(
                    Rule::Platform,
                    &name.span,
                    &name.path,
                    format!(
                        "the {} volume profile refuses this name: {fault}",
                        format.value
                    ),
                );
            }
            if !stored.insert(&name.value) {
                sink.report(
                    Rule::Platform,
                    &name.span,
                    &name.path,
                    format!("the stored name `{}` is listed twice", name.value),
                );
            }
        }
        if let Some(Value::Tree(members)) = tree
            .as_ref()
            .and_then(|tree| names.resolve(&mut quiet, tree, Some(index), Want::Tree))
        {
            for member in members {
                if let Some(fault) = profile.path_fault(&member.path.value) {
                    sink.report(
                        Rule::Platform,
                        &member.path.span,
                        &member.path.path,
                        format!(
                            "the {} volume profile refuses this path: {fault}",
                            format.value
                        ),
                    );
                }
            }
        }
    }
    for rom in &recipe.roms {
        let bytes = rom_bytes(rom, names, sink);
        for (id, entry) in rom_entries(rom, &bytes, provider, recipe.package.as_ref()) {
            // An entry found by a chip is no claim about the whole ROM.
            if rom.catalogue_id.is_none()
                && bytes
                    .sha256
                    .as_ref()
                    .is_none_or(|s| s.value != entry.sha256)
            {
                continue;
            }
            if let Some(size) = bytes.size.as_ref().filter(|s| s.value != entry.size) {
                sink.report(
                    Rule::Platform,
                    &size.span,
                    &size.path,
                    format!(
                        "the catalogue entry `{id}` is {} bytes; this ROM is {}",
                        entry.size, size.value
                    ),
                );
            }
            if let Some(sha256) = bytes.sha256.as_ref().filter(|s| s.value != entry.sha256) {
                sink.report(
                    Rule::Platform,
                    &sha256.span,
                    &sha256.path,
                    format!(
                        "the catalogue entry `{id}` has SHA-256 {}; this ROM has {}",
                        entry.sha256, sha256.value
                    ),
                );
            }
        }
    }
}

/// Rule 8: a `digest` pins a lock. Comparing the two is left to the lock;
/// this checks only that the lock exists.
fn lock_rule(recipe: &Recipe, sink: &mut Sink<'_>, lock: bool) {
    if lock {
        return;
    }
    let digests = recipe
        .identity
        .digest
        .iter()
        .chain(recipe.attachments.iter().filter_map(|a| a.digest.as_ref()));
    for digest in digests {
        sink.report(
            Rule::Lock,
            &digest.span,
            &digest.path,
            format!(
                "a digest pins a lock, and there is no {}.lock beside the recipe",
                file_name(&sink.source.name)
            ),
        );
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Rule 9: a bundled input over 1 MiB, and the counts.
fn limits(recipe: &Recipe, sink: &mut Sink<'_>) {
    for input in &recipe.inputs {
        let (
            Some(bundle),
            InputForm::File {
                size: Some(size), ..
            },
        ) = (&input.bundle, &input.form)
        else {
            continue;
        };
        if bundle.value && size.value > MAX_BUNDLE {
            sink.report(
                Rule::Limit,
                &size.span,
                &size.path,
                format!(
                    "a bundled input is at most {MAX_BUNDLE} bytes (1 MiB); this one is {}",
                    size.value
                ),
            );
        }
    }
    let at = &recipe.identity.at;
    let outputs = recipe.media.len()
        + recipe.roms.len()
        + recipe.extras.len()
        + recipe
            .attachments
            .iter()
            .map(|a| 1 + a.extras.len())
            .sum::<usize>();
    let entries: usize = recipe
        .inputs
        .iter()
        .map(|i| match &i.form {
            InputForm::FileSet { files } => files.len(),
            InputForm::File { .. } | InputForm::Refused => 0,
        })
        .sum();
    for (count, limit, what) in [
        (recipe.inputs.len(), MAX_INPUTS, "inputs"),
        (recipe.steps.len(), MAX_STEPS, "steps"),
        (outputs, MAX_OUTPUTS, "outputs"),
        (entries, MAX_FILE_SET_ENTRIES, "file-set entries"),
    ] {
        if count > limit {
            sink.report(
                Rule::Limit,
                &at.span,
                &at.path,
                format!("the recipe has {count} {what}; at most {limit} are allowed"),
            );
        }
    }
}
