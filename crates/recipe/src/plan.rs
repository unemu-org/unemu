// SPDX-License-Identifier: AGPL-3.0-or-later
//! The plan of a recipe (recipes §3.4, §3.5): where each input is found, or
//! why not, which steps would run and which outputs can be built. It reads
//! and hashes, and writes nothing but the rebuildable index caches.
//!
//! **Resolution order.** A value with a stated size and SHA-256 is looked
//! for in: (1) the store, by the decoded digest of its layers
//! ([`StoreIndex`]); (2) the download cache, a folder of files named by
//! their SHA-256 that a build with `--fetch` fills; (3) the recipe's bundle, a file of the input's name beside
//! the recipe, for an input with `bundle = true`; (4) the libraries in the
//! order given, each searched as [`Library::find`] says. The first match
//! wins. URLs (5) are listed, never fetched.
//!
//! **Found instead of made.** A member of an `unpack` step that states its
//! size and SHA-256 is looked for first. An archive is needed only for the
//! members still missing; if every member an output reads is found, the
//! archive is not needed and is not looked for. The same holds one level
//! further for a pinned recipe ([`plan_pinned`]): an output a step makes
//! is looked for by the payload its lock names, and when it is found, the
//! steps and inputs only it needed are not needed. Otherwise a step makes
//! new bytes and runs when an output needs them.
//!
//! **Near misses** are reported for a value not found and never used: a
//! file or member with one of its names and another size or hash, and one
//! of its size whose hash differs.

use crate::library::{Hints, Library, Place};
use crate::model::{InputForm, Kind, Op, Recipe, RomSource};
use crate::store_index::StoreIndex;
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// How many near misses a value reports, at most.
const NEAR_MISSES: usize = 8;

/// Where the plan looks.
pub struct Sources<'a> {
    pub store: Option<&'a StoreIndex>,
    /// The download cache: files named by their lower-case SHA-256.
    pub downloads: Option<&'a Path>,
    pub libraries: &'a mut [Library],
}

/// Where a value was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// Content of images in the store, under these tags.
    Store { tags: Vec<String> },
    /// A file of the download cache, named by its SHA-256.
    Download { path: PathBuf },
    /// A file of the input's name beside the recipe.
    Bundle { path: PathBuf },
    /// A file or zip member of a library.
    Library { library: PathBuf, place: Place },
}

impl std::fmt::Display for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store { tags } => write!(f, "in the store ({})", tags.join(", ")),
            Self::Download { path } => write!(f, "in the download cache: {}", path.display()),
            Self::Bundle { path } => write!(f, "beside the recipe: {}", path.display()),
            Self::Library { library, place } => {
                write!(f, "in library {}: {place}", library.display())
            }
        }
    }
}

/// Why a near miss is not the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissKind {
    /// It has one of the value's names and another size.
    NameOtherSize,
    /// It has one of the value's names and size, and another hash.
    NameOtherHash,
    /// It has the value's size and another hash.
    SizeOtherHash,
}

impl MissKind {
    fn key(self) -> &'static str {
        match self {
            Self::NameOtherSize => "name-other-size",
            Self::NameOtherHash => "name-other-hash",
            Self::SizeOtherHash => "size-other-hash",
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::NameOtherSize => "same name, another size",
            Self::NameOtherHash => "same name and size, another hash",
            Self::SizeOtherHash => "same size, another hash",
        }
    }
}

/// A file or member that looks like a missing value but is not it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Miss {
    pub library: PathBuf,
    pub place: Place,
    pub size: u64,
    pub sha256: Option<String>,
    pub kind: MissKind,
}

/// One value with a stated size and hash, and what the search gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    pub size: u64,
    pub sha256: String,
    pub found: Option<Found>,
    pub near_misses: Vec<Miss>,
}

/// What an input is, as the plan reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Form {
    File(Value),
    /// The files of a file set, by path.
    Files(Vec<(String, Value)>),
}

/// The state of an input, a step or an output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Input: found. Step: runs. Output: can be built.
    Ready,
    /// Not needed, and why.
    NotNeeded(String),
    /// Missing, or blocked by the inputs named.
    Missing(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputPlan {
    pub id: String,
    pub name: String,
    /// Only attachments use it.
    pub optional: bool,
    pub form: Form,
    pub status: Status,
    /// The labels of the outputs that need it.
    pub used_by: Vec<String>,
    pub urls: Vec<String>,
    /// `(db, set, name)` of each catalogue entry of the dump.
    pub ids: Vec<(String, String, String)>,
}

/// A member of an `unpack` step looked for before its archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberPlan {
    pub step: String,
    pub path: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepPlan {
    pub id: String,
    pub verb: &'static str,
    pub operand: String,
    pub status: Status,
    /// What the step does when it runs, beyond its verb.
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputPlan {
    /// `medium`, `rom`, `extra` or `attachment`.
    pub kind: &'static str,
    pub id: String,
    pub label: String,
    pub from: Vec<String>,
    pub optional: bool,
    pub status: Status,
}

/// What one library did for the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryPlan {
    pub path: PathBuf,
    pub stats: crate::library::Stats,
}

/// The plan of one recipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub file: String,
    pub kind: Kind,
    pub name: Option<String>,
    pub store: Option<PathBuf>,
    /// A pinned recipe whose lock is lost (recipes §7.2): the lock's path.
    /// A build makes the lock's files with its own writers and accepts only
    /// the pinned digest.
    pub lock_lost: Option<PathBuf>,
    /// What the store's index could not read or keep.
    pub store_warnings: Vec<String>,
    pub libraries: Vec<LibraryPlan>,
    pub inputs: Vec<InputPlan>,
    pub members: Vec<MemberPlan>,
    /// A pinned recipe's outputs looked for by their locked payloads; empty
    /// for a draft.
    pub payloads: Vec<PayloadPlan>,
    pub steps: Vec<StepPlan>,
    pub outputs: Vec<OutputPlan>,
}

/// The payload a pinned recipe's lock names for one output file, which the
/// plan looks for before it plans the steps that make it (recipes §3.4,
/// "found instead of made").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedOutput {
    /// `medium`, `rom` or `extra`.
    pub kind: &'static str,
    /// The output's place among the recipe's outputs of its kind.
    pub index: usize,
    /// The file of the output: 0, or a ROM's chip in order.
    pub part: usize,
    /// The output's file name, a hint for the library search.
    pub name: String,
    /// The decoded size and SHA-256 of the locked payload.
    pub size: u64,
    pub sha256: String,
}

/// A locked output looked for before it is made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadPlan {
    pub kind: &'static str,
    pub id: String,
    pub index: usize,
    pub part: usize,
    pub name: String,
    /// The reference the output is made from when it is not found.
    pub from: String,
    pub value: Value,
}

/// What [`plan`] needs to know of a step.
struct StepInfo<'r> {
    op: &'r Op,
}

struct Planner<'r, 's> {
    bundle: Option<&'r Path>,
    /// Bundled inputs a recipe artifact lists, `(name, size, sha256)`, which
    /// its manifest names without their bytes being fetched.
    listed: &'r [(String, u64, String)],
    sources: Sources<'s>,
    steps: BTreeMap<&'r str, StepInfo<'r>>,
    inputs: BTreeSet<&'r str>,
    members: BTreeMap<(String, String), Option<Value>>,
    member_order: Vec<(String, String)>,
    step_needs: BTreeMap<String, BTreeSet<String>>,
}

/// One output's references, its kind, id and label, and whether only an
/// attachment has it.
struct OutputRef {
    kind: &'static str,
    /// The place among the recipe's outputs of its kind.
    index: usize,
    id: String,
    label: String,
    from: Vec<String>,
    optional: bool,
}

fn outputs(recipe: &Recipe) -> Vec<OutputRef> {
    let mut out = Vec::new();
    let value = |v: &Option<crate::model::At<String>>| v.as_ref().map(|v| v.value.clone());
    for (index, medium) in recipe.media.iter().enumerate() {
        out.push(OutputRef {
            kind: "medium",
            index,
            id: value(&medium.id).unwrap_or_default(),
            label: value(&medium.label)
                .or_else(|| value(&medium.id))
                .unwrap_or_default(),
            from: value(&medium.from).into_iter().collect(),
            optional: false,
        });
    }
    for rom in &recipe.roms {
        let from = match &rom.source {
            RomSource::File(from) => value(from).into_iter().collect(),
            RomSource::Parts { parts, .. } => parts.iter().filter_map(|p| value(&p.from)).collect(),
        };
        out.push(OutputRef {
            kind: "rom",
            index: out.iter().filter(|o| o.kind == "rom").count(),
            id: value(&rom.id).unwrap_or_default(),
            label: value(&rom.id).unwrap_or_default(),
            from,
            optional: false,
        });
    }
    for (index, extra) in recipe.extras.iter().enumerate() {
        out.push(OutputRef {
            kind: "extra",
            index,
            id: value(&extra.id).unwrap_or_default(),
            label: value(&extra.file_name)
                .or_else(|| value(&extra.id))
                .unwrap_or_default(),
            from: value(&extra.from).into_iter().collect(),
            optional: false,
        });
    }
    for attachment in &recipe.attachments {
        let id = value(&attachment.id).unwrap_or_default();
        for extra in &attachment.extras {
            out.push(OutputRef {
                kind: "attachment",
                index: 0,
                id: format!("{id}/{}", value(&extra.id).unwrap_or_default()),
                label: format!(
                    "{}: {}",
                    value(&attachment.title).unwrap_or_else(|| id.clone()),
                    value(&extra.file_name)
                        .or_else(|| value(&extra.id))
                        .unwrap_or_default()
                ),
                from: value(&extra.from).into_iter().collect(),
                optional: true,
            });
        }
    }
    out
}

fn split(reference: &str) -> (&str, Option<&str>) {
    match reference.split_once('/') {
        Some((name, member)) => (name, Some(member)),
        None => (reference, None),
    }
}

fn base_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

impl<'r, 's> Planner<'r, 's> {
    /// Look a value up in the store and the libraries (and, for `bundle`, the
    /// file of that name beside the recipe), in the order of recipes §3.4.
    fn search(&mut self, size: u64, sha256: &str, hints: &Hints, bundle: Option<&str>) -> Value {
        let mut value = Value {
            size,
            sha256: sha256.to_owned(),
            found: None,
            near_misses: Vec::new(),
        };
        if let Some(tags) = self.sources.store.and_then(|s| s.find(size, sha256)) {
            value.found = Some(Found::Store {
                tags: tags.to_vec(),
            });
            return value;
        }
        if let Some(folder) = self.sources.downloads {
            let path = folder.join(sha256);
            let matches = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == size)
                && file_sha256(&path).is_ok_and(|hash| hash == sha256);
            if matches {
                value.found = Some(Found::Download { path });
                return value;
            }
        }
        if let (Some(name), Some(folder)) = (bundle, self.bundle) {
            let path = folder.join(name);
            let matches = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == size)
                && std::fs::read(&path).is_ok_and(|bytes| {
                    use sha2::Digest as _;
                    format!("{:x}", sha2::Sha256::digest(&bytes)) == sha256
                });
            if matches {
                value.found = Some(Found::Bundle { path });
                return value;
            }
        }
        if let Some(name) = bundle {
            if self
                .listed
                .iter()
                .any(|(n, s, h)| n == name && *s == size && h == sha256)
            {
                value.found = Some(Found::Bundle {
                    path: PathBuf::from(name),
                });
                return value;
            }
        }
        for library in self.sources.libraries.iter_mut() {
            if let Some(place) = library.find(size, sha256, hints) {
                value.found = Some(Found::Library {
                    library: library.named.clone(),
                    place,
                });
                return value;
            }
        }
        for library in self.sources.libraries.iter() {
            for miss in library.near_misses(size, sha256, hints) {
                if value.near_misses.len() == NEAR_MISSES {
                    break;
                }
                let kind = match (miss.named, miss.size == size) {
                    (true, false) => MissKind::NameOtherSize,
                    (true, true) => MissKind::NameOtherHash,
                    (false, _) => MissKind::SizeOtherHash,
                };
                value.near_misses.push(Miss {
                    library: library.named.clone(),
                    place: miss.place,
                    size: miss.size,
                    sha256: miss.sha256,
                    kind,
                });
            }
        }
        value
    }

    /// The member `path` of the unpack step `step`, looked for once; `None`
    /// when the recipe does not state its size and hash, so only its
    /// archive gives it.
    fn member(&mut self, step: &str, path: &str) -> Option<bool> {
        let key = (step.to_owned(), path.to_owned());
        if let Some(known) = self.members.get(&key) {
            return known.as_ref().map(|v| v.found.is_some());
        }
        let stated = self.steps.get(step).and_then(|info| match info.op {
            Op::Unpack { members, .. } => members
                .iter()
                .find(|m| m.path.value == path)
                .and_then(|m| Some((m.size.as_ref()?.value, m.sha256.as_ref()?.value.clone()))),
            _ => None,
        });
        let value = stated.map(|(size, sha256)| {
            let hints = Hints {
                names: vec![base_name(path)],
                path: Some(path.to_owned()),
                ..Hints::default()
            };
            self.search(size, &sha256, &hints, None)
        });
        let found = value.as_ref().map(|v| v.found.is_some());
        self.members.insert(key.clone(), value);
        self.member_order.push(key);
        found
    }

    /// Whether the value `reference` names is made by a step and is not a
    /// member the recipe states the size and hash of, which the member
    /// search looks for already: the outputs worth looking for by their
    /// locked payload.
    fn made_by_step(&self, reference: &str) -> bool {
        let (name, member) = split(reference);
        match (self.steps.get(name).map(|info| info.op), member) {
            (None, _) => false,
            (Some(Op::Unpack { members, .. }), Some(path)) => !members
                .iter()
                .any(|m| m.path.value == path && m.size.is_some() && m.sha256.is_some()),
            (Some(_), _) => true,
        }
    }

    /// Every step and input `reference` is made from, members and found
    /// values aside: what a found output makes unnecessary.
    fn reach(&self, reference: &str, into: &mut BTreeSet<String>, depth: usize) {
        let (name, _) = split(reference);
        into.insert(name.to_owned());
        if depth > 64 {
            return;
        }
        let operand = match self.steps.get(name).map(|info| info.op) {
            Some(Op::Unpack { archive, .. }) => archive.as_ref(),
            Some(Op::Fat { tree, .. }) => tree.as_ref(),
            Some(Op::Text { text }) => text.as_ref(),
            _ => None,
        };
        if let Some(operand) = operand {
            self.reach(&operand.value, into, depth + 1);
        }
    }

    /// The inputs a reference needs, after found members are taken out. The
    /// steps it reaches are added to `steps`, and each one's own needs kept
    /// in `step_needs`.
    fn needs(
        &mut self,
        reference: &str,
        steps: &mut BTreeSet<String>,
        depth: usize,
    ) -> BTreeSet<String> {
        let (name, member) = split(reference);
        let mut needs = BTreeSet::new();
        if self.inputs.contains(name) {
            needs.insert(name.to_owned());
            return needs;
        }
        // References only reach earlier steps (rule 4), so the depth is
        // bounded by the number of steps; the guard keeps a refused recipe
        // from looping.
        if depth > 64 {
            return needs;
        }
        let Some(info) = self.steps.get(name) else {
            return needs;
        };
        steps.insert(name.to_owned());
        let operand = match info.op {
            Op::Unpack {
                archive, members, ..
            } => {
                let archive = archive.as_ref().map(|a| a.value.clone());
                let wanted: Vec<String> = match member {
                    Some(member) => vec![member.to_owned()],
                    None => members.iter().map(|m| m.path.value.clone()).collect(),
                };
                let mut open = false;
                for path in wanted {
                    if self.member(name, &path) != Some(true) {
                        open = true;
                    }
                }
                archive.filter(|_| open)
            }
            Op::Fat { tree, .. } => tree.as_ref().map(|t| t.value.clone()),
            Op::Text { text } => text.as_ref().map(|t| t.value.clone()),
            Op::Unknown { .. } => None,
        };
        if let Some(operand) = operand {
            needs = self.needs(&operand, steps, depth + 1);
        }
        self.step_needs
            .entry(name.to_owned())
            .or_default()
            .extend(needs.iter().cloned());
        needs
    }
}

/// Plan `recipe`, read from `file` (as named, for the report), whose bundled
/// inputs lie in `bundle`.
pub fn plan(recipe: &Recipe, file: &str, bundle: Option<&Path>, sources: Sources<'_>) -> Plan {
    plan_pinned(recipe, file, bundle, sources, &[])
}

/// [`plan`] for a pinned recipe whose lock names the payloads `locked`:
/// each output a step makes is looked for by its locked payload first, in
/// the same sources and order as an input; a found output needs no step and
/// no input (recipes §3.4, "found instead of made"). An output that refers
/// to an input, or to a member whose size and hash the recipe states, is
/// not looked for again: its own search is the same search.
pub fn plan_pinned(
    recipe: &Recipe,
    file: &str,
    bundle: Option<&Path>,
    sources: Sources<'_>,
    locked: &[LockedOutput],
) -> Plan {
    plan_with(recipe, file, bundle, &[], sources, locked)
}

/// [`plan`] for a recipe held in a registry artifact whose manifest lists
/// its bundled inputs as `(name, size, sha256)`: such an input counts as
/// found in the artifact, which a pull lays out beside the recipe, without
/// its bytes being fetched (recipes §7.1).
pub fn plan_listed(
    recipe: &Recipe,
    file: &str,
    listed: &[(String, u64, String)],
    sources: Sources<'_>,
) -> Plan {
    plan_with(recipe, file, None, listed, sources, &[])
}

fn plan_with(
    recipe: &Recipe,
    file: &str,
    bundle: Option<&Path>,
    listed: &[(String, u64, String)],
    sources: Sources<'_>,
    locked: &[LockedOutput],
) -> Plan {
    let store = sources.store.map(|s| s.root.clone());
    let store_warnings = sources
        .store
        .map(|s| s.warnings.clone())
        .unwrap_or_default();
    let mut planner = Planner {
        bundle,
        listed,
        sources,
        steps: recipe
            .steps
            .iter()
            .filter_map(|s| Some((s.id.as_ref()?.value.as_str(), StepInfo { op: &s.op })))
            .collect(),
        inputs: recipe.inputs.iter().map(|i| i.id.value.as_str()).collect(),
        members: BTreeMap::new(),
        member_order: Vec::new(),
        step_needs: BTreeMap::new(),
    };

    // What each output needs: its inputs after found outputs and found
    // members are taken out.
    let outputs = outputs(recipe);
    let mut needed: Vec<BTreeSet<String>> = Vec::new();
    let mut reached = BTreeSet::new();
    let mut payloads = Vec::new();
    // The steps and inputs found outputs are made from.
    let mut spared = BTreeSet::new();
    for output in &outputs {
        let mut inputs = BTreeSet::new();
        for (part, from) in output.from.iter().enumerate() {
            let wanted = locked
                .iter()
                .find(|l| l.kind == output.kind && l.index == output.index && l.part == part);
            if let Some(wanted) = wanted.filter(|_| planner.made_by_step(from)) {
                let hints = Hints {
                    names: vec![wanted.name.clone()],
                    ..Hints::default()
                };
                let value = planner.search(wanted.size, &wanted.sha256, &hints, None);
                let found = value.found.is_some();
                payloads.push(PayloadPlan {
                    kind: output.kind,
                    id: output.id.clone(),
                    index: output.index,
                    part,
                    name: wanted.name.clone(),
                    from: from.clone(),
                    value,
                });
                if found {
                    planner.reach(from, &mut spared, 0);
                    continue;
                }
            }
            inputs.extend(planner.needs(from, &mut reached, 0));
        }
        needed.push(inputs);
    }
    let required: BTreeSet<&String> = outputs
        .iter()
        .zip(&needed)
        .filter(|(o, _)| !o.optional)
        .flat_map(|(_, n)| n)
        .collect();
    let any: BTreeSet<&String> = needed.iter().flatten().collect();

    // The inputs, looked for in recipe order.
    let mut inputs = Vec::new();
    for input in &recipe.inputs {
        let id = input.id.value.clone();
        let name = input
            .name
            .as_ref()
            .map(|n| n.value.clone())
            .unwrap_or_default();
        let wanted = any.contains(&id);
        let used_by: Vec<String> = outputs
            .iter()
            .zip(&needed)
            .filter(|(_, n)| n.contains(&id))
            .map(|(o, _)| o.label.clone())
            .collect();
        let mut names = vec![name.clone()];
        names.extend(input.names.iter().map(|n| n.value.clone()));
        let hints = Hints {
            names,
            path: None,
            crc32: input.crc32.as_ref().map(|v| v.value.clone()),
            sha1: input.sha1.as_ref().map(|v| v.value.clone()),
            md5: input.md5.as_ref().map(|v| v.value.clone()),
        };
        let unsearched = |size: u64, sha256: String| Value {
            size,
            sha256,
            found: None,
            near_misses: Vec::new(),
        };
        let bundled = input.bundle.as_ref().is_some_and(|b| b.value);
        let form = match &input.form {
            InputForm::File { size, sha256 } => {
                let size = size.as_ref().map_or(0, |s| s.value);
                let sha256 = sha256.as_ref().map(|s| s.value.clone()).unwrap_or_default();
                Form::File(if wanted {
                    planner.search(size, &sha256, &hints, bundled.then_some(name.as_str()))
                } else {
                    unsearched(size, sha256)
                })
            }
            InputForm::FileSet { files } => Form::Files(
                files
                    .iter()
                    .map(|f| {
                        let size = f.size.as_ref().map_or(0, |s| s.value);
                        let sha256 = f
                            .sha256
                            .as_ref()
                            .map(|s| s.value.clone())
                            .unwrap_or_default();
                        let value = if wanted {
                            let hints = Hints {
                                names: vec![base_name(&f.path.value)],
                                path: Some(f.path.value.clone()),
                                ..Hints::default()
                            };
                            planner.search(size, &sha256, &hints, None)
                        } else {
                            unsearched(size, sha256)
                        };
                        (f.path.value.clone(), value)
                    })
                    .collect(),
            ),
            InputForm::Refused => Form::Files(Vec::new()),
        };
        let complete = match &form {
            Form::File(v) => v.found.is_some(),
            Form::Files(files) => files.iter().all(|(_, v)| v.found.is_some()),
        };
        let status = if !wanted {
            not_needed(recipe, &id, &planner.members, &spared)
        } else if complete {
            Status::Ready
        } else {
            Status::Missing(Vec::new())
        };
        inputs.push(InputPlan {
            id: id.clone(),
            name,
            optional: wanted && !required.contains(&id),
            form,
            status,
            used_by,
            urls: input.urls.iter().map(|u| u.value.clone()).collect(),
            ids: input
                .ids
                .iter()
                .map(|i| {
                    (
                        i.db.value.clone(),
                        i.set.value.clone(),
                        i.name.value.clone(),
                    )
                })
                .collect(),
        });
    }
    let missing: BTreeSet<String> = inputs
        .iter()
        .filter(|i| matches!(i.status, Status::Missing(_)))
        .map(|i| i.id.clone())
        .collect();

    // Steps, in recipe order.
    let mut steps = Vec::new();
    for step in &recipe.steps {
        let Some(id) = step.id.as_ref().map(|i| i.value.clone()) else {
            continue;
        };
        let (verb, operand) = match &step.op {
            Op::Unpack { archive, .. } => ("unpack", archive),
            Op::Fat { tree, .. } => ("fat", tree),
            Op::Text { text } => ("text", text),
            Op::Unknown { .. } => continue,
        };
        let operand = operand
            .as_ref()
            .map(|o| o.value.clone())
            .unwrap_or_default();
        let mut detail = None;
        let status = if !reached.contains(&id) {
            Status::NotNeeded(if spared.contains(&id) {
                "the outputs it makes were found".into()
            } else {
                "no output uses it".into()
            })
        } else {
            let blocked: Vec<String> = planner
                .step_needs
                .get(&id)
                .map(|needs| needs.intersection(&missing).cloned().collect())
                .unwrap_or_default();
            let used: Vec<&(String, String)> = planner
                .member_order
                .iter()
                .filter(|(s, _)| *s == id)
                .collect();
            let open = used
                .iter()
                .filter(
                    |key| !matches!(planner.members.get(*key), Some(Some(v)) if v.found.is_some()),
                )
                .count();
            if verb == "unpack" && open == 0 {
                Status::NotNeeded(format!(
                    "its {} member{} were found",
                    used.len(),
                    if used.len() == 1 { "" } else { "s" }
                ))
            } else {
                if verb == "unpack" && open < used.len() {
                    detail = Some(format!(
                        "opens {operand} for {open} of {} members",
                        used.len()
                    ));
                }
                if blocked.is_empty() {
                    Status::Ready
                } else {
                    Status::Missing(blocked)
                }
            }
        };
        steps.push(StepPlan {
            id,
            verb,
            operand,
            status,
            detail,
        });
    }

    let outputs = outputs
        .into_iter()
        .zip(&needed)
        .map(|(o, needs)| {
            let blocked: Vec<String> = needs.intersection(&missing).cloned().collect();
            OutputPlan {
                kind: o.kind,
                id: o.id,
                label: o.label,
                from: o.from,
                optional: o.optional,
                status: if blocked.is_empty() {
                    Status::Ready
                } else {
                    Status::Missing(blocked)
                },
            }
        })
        .collect();

    let members = planner
        .member_order
        .iter()
        .filter_map(|key| {
            let value = planner.members.get(key)?.clone()?;
            Some(MemberPlan {
                step: key.0.clone(),
                path: key.1.clone(),
                value,
            })
        })
        .collect();
    Plan {
        file: file.to_owned(),
        kind: recipe.kind,
        name: recipe.identity.name.as_ref().map(|n| n.value.clone()),
        store,
        lock_lost: None,
        store_warnings,
        libraries: planner
            .sources
            .libraries
            .iter()
            .map(|l| LibraryPlan {
                path: l.named.clone(),
                stats: l.stats.clone(),
            })
            .collect(),
        inputs,
        members,
        payloads,
        steps,
        outputs,
    }
}

/// The lower-case SHA-256 of a file, read in pieces.
pub(crate) fn file_sha256(path: &Path) -> std::io::Result<String> {
    use sha2::Digest as _;
    use std::io::Read as _;
    let mut input = std::fs::File::open(path)?;
    let mut sha = sha2::Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            return Ok(format!("{:x}", sha.finalize()));
        }
        sha.update(&buffer[..n]);
    }
}

/// Why an input no output needs is not needed: it is the archive of unpack
/// steps whose members were all found, the outputs made from it were
/// found, or nothing uses it.
fn not_needed(
    recipe: &Recipe,
    id: &str,
    members: &BTreeMap<(String, String), Option<Value>>,
    spared: &BTreeSet<String>,
) -> Status {
    let mut found = 0;
    for step in &recipe.steps {
        if let (Some(step_id), Op::Unpack { archive, .. }) = (&step.id, &step.op) {
            if archive.as_ref().is_some_and(|a| split(&a.value).0 == id) {
                found += members
                    .iter()
                    .filter(|((s, _), v)| {
                        *s == step_id.value && v.as_ref().is_some_and(|v| v.found.is_some())
                    })
                    .count();
            }
        }
    }
    Status::NotNeeded(if found > 0 {
        format!(
            "its {found} member{} were found",
            if found == 1 { "" } else { "s" }
        )
    } else if spared.contains(id) {
        "the outputs made from it were found".into()
    } else {
        "no output uses it".into()
    })
}

impl Plan {
    /// Every input the artifact needs is found or not needed.
    pub fn complete(&self) -> bool {
        self.inputs
            .iter()
            .all(|i| i.optional || !matches!(i.status, Status::Missing(_)))
    }

    /// The summary line of recipes §3.5.
    pub fn summary(&self) -> String {
        let own: Vec<&InputPlan> = self.inputs.iter().filter(|i| !i.optional).collect();
        let total = own.len();
        let missing = own
            .iter()
            .filter(|i| matches!(i.status, Status::Missing(_)))
            .count();
        let not_needed = own.iter().any(|i| matches!(i.status, Status::NotNeeded(_)));
        let optional_missing = self
            .inputs
            .iter()
            .filter(|i| i.optional && matches!(i.status, Status::Missing(_)))
            .count();
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let mut text = if missing == 0 {
            format!(
                "can build: all {total} input{} found{}",
                plural(total),
                if not_needed { " or not needed" } else { "" }
            )
        } else {
            format!("missing {missing} of {total} input{}", plural(total))
        };
        if optional_missing > 0 {
            text.push_str(&format!(
                "; {optional_missing} optional input{} not found",
                plural(optional_missing)
            ));
        }
        text
    }

    /// The text `upcli recipe plan` prints.
    pub fn render(&self) -> String {
        let mut out = format!(
            "{}: {} recipe {}\n",
            self.file,
            self.kind.key(),
            self.name.as_deref().unwrap_or("(no name)")
        );
        match &self.store {
            Some(store) => out.push_str(&format!("store: {}\n", store.display())),
            None => out.push_str("store: none\n"),
        }
        for warning in &self.store_warnings {
            out.push_str(&format!("  warning: {warning}\n"));
        }
        for library in &self.libraries {
            let s = &library.stats;
            out.push_str(&format!(
                "library {}: {} files, {} zip archives ({} members), {} catalogue rows; hashed {}, kept {}, dropped {}\n",
                library.path.display(),
                s.files,
                s.archives,
                s.members,
                s.catalogue_rows,
                s.hashed.len(),
                s.kept,
                s.dropped
            ));
            for (catalogue, place) in &s.stale {
                out.push_str(&format!(
                    "  stale catalogue row: {catalogue} names {place}, whose bytes differ\n"
                ));
            }
            for (place, error) in &s.unreadable {
                out.push_str(&format!("  unreadable: {place}: {error}\n"));
            }
            for (place, error) in &s.failed {
                out.push_str(&format!("  hashing failed: {place}: {error}\n"));
            }
        }
        if self.libraries.is_empty() {
            out.push_str("libraries: none\n");
        }
        if let Some(lock) = &self.lock_lost {
            out.push_str(&format!(
                "lock: {} is lost; a build makes its files with this build's writers and accepts only the pinned digest\n",
                lock.display()
            ));
        }
        out.push_str("inputs:\n");
        for input in &self.inputs {
            let optional = if input.optional { " (optional)" } else { "" };
            out.push_str(&format!("  {} ({}){optional}: ", input.id, input.name));
            match (&input.status, &input.form) {
                (Status::NotNeeded(why), _) => out.push_str(&format!("not needed: {why}\n")),
                (_, Form::File(value)) => match &value.found {
                    Some(found) => out.push_str(&format!("found {found}\n")),
                    None => {
                        out.push_str(&format!(
                            "missing: {} bytes, sha256 {}\n",
                            value.size, value.sha256
                        ));
                        misses(&mut out, value, "    ");
                    }
                },
                (_, Form::Files(files)) => {
                    let found = files.iter().filter(|(_, v)| v.found.is_some()).count();
                    out.push_str(&format!(
                        "{} {found} of {} files{}\n",
                        if found == files.len() {
                            "found"
                        } else {
                            "missing some:"
                        },
                        files.len(),
                        places(files)
                    ));
                    for (path, value) in files.iter().filter(|(_, v)| v.found.is_none()) {
                        out.push_str(&format!(
                            "    missing {path}: {} bytes, sha256 {}\n",
                            value.size, value.sha256
                        ));
                        misses(&mut out, value, "      ");
                    }
                }
            }
            if matches!(input.status, Status::Missing(_)) {
                if !input.used_by.is_empty() {
                    out.push_str(&format!("    used for: {}\n", input.used_by.join("; ")));
                }
                for (db, set, name) in &input.ids {
                    out.push_str(&format!("    id: {db} {set:?} {name:?}\n"));
                }
                for url in &input.urls {
                    out.push_str(&format!("    url: {url}\n"));
                }
            }
        }
        if !self.members.is_empty() {
            out.push_str("members found instead of made:\n");
            for member in &self.members {
                match &member.value.found {
                    Some(found) => out.push_str(&format!(
                        "  {}/{}: found {found}\n",
                        member.step, member.path
                    )),
                    None => {
                        out.push_str(&format!(
                            "  {}/{}: not found, taken from the archive\n",
                            member.step, member.path
                        ));
                        misses(&mut out, &member.value, "    ");
                    }
                }
            }
        }
        if !self.payloads.is_empty() {
            out.push_str("outputs found instead of made:\n");
            for payload in &self.payloads {
                out.push_str(&format!("  {}: ", payload.what()));
                match &payload.value.found {
                    Some(found) => out.push_str(&format!("found {found}\n")),
                    None => out.push_str(&format!("not found, made from {}\n", payload.from)),
                }
            }
        }
        out.push_str("steps:\n");
        for step in &self.steps {
            out.push_str(&format!(
                "  {}: {} {}: {}",
                step.id,
                step.verb,
                step.operand,
                status(&step.status, "runs", "blocked")
            ));
            if let Some(detail) = &step.detail {
                out.push_str(&format!(" ({detail})"));
            }
            out.push('\n');
        }
        out.push_str("outputs:\n");
        for output in &self.outputs {
            out.push_str(&format!(
                "  {} {} {:?}: {}\n",
                output.kind,
                output.id,
                output.label,
                status(&output.status, "can build", "blocked")
            ));
        }
        out.push_str(&format!("{}\n", self.summary()));
        out
    }

    /// The text `upcli recipe plan --json` prints: [`Self::json`], indented,
    /// and a line break.
    pub fn render_json(&self) -> String {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&self.json()).unwrap_or_default()
        )
    }

    /// The JSON of the plan: one object, keys sorted. A pinned recipe's
    /// plan adds `payloads`, its outputs looked for by their locked
    /// payloads.
    pub fn json(&self) -> Json {
        let mut object = self.json_common();
        if let (Json::Object(map), Some(lock)) = (&mut object, &self.lock_lost) {
            map.insert("lock_lost".into(), json!(lock.display().to_string()));
        }
        if let (Json::Object(map), false) = (&mut object, self.payloads.is_empty()) {
            map.insert(
                "payloads".into(),
                Json::Array(
                    self.payloads
                        .iter()
                        .map(|p| {
                            json!({
                                "kind": p.kind,
                                "id": p.id,
                                "part": p.part,
                                "name": p.name,
                                "from": p.from,
                                "size": p.value.size,
                                "sha256": p.value.sha256,
                                "found": p.value.found.as_ref().map(found_json),
                            })
                        })
                        .collect(),
                ),
            );
        }
        object
    }

    fn json_common(&self) -> Json {
        json!({
            "recipe": self.file,
            "kind": self.kind.key(),
            "name": self.name,
            "complete": self.complete(),
            "summary": self.summary(),
            "store": self.store.as_ref().map(|s| s.display().to_string()),
            "store_warnings": self.store_warnings,
            "libraries": self.libraries.iter().map(|l| {
                let s = &l.stats;
                json!({
                    "path": l.path.display().to_string(),
                    "files": s.files,
                    "archives": s.archives,
                    "members": s.members,
                    "catalogue_rows": s.catalogue_rows,
                    "hashed": s.hashed.len(),
                    "failed": s.failed.iter().map(|(p, e)| json!({"path": p.to_string(), "error": e})).collect::<Vec<_>>(),
                    "kept": s.kept,
                    "dropped": s.dropped,
                    "stale": s.stale.iter().map(|(c, p)| json!({"catalogue": c, "path": p.to_string()})).collect::<Vec<_>>(),
                    "unreadable": s.unreadable.iter().map(|(p, e)| json!({"path": p.to_string(), "error": e})).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
            "inputs": self.inputs.iter().map(input_json).collect::<Vec<_>>(),
            "members": self.members.iter().map(|m| {
                let mut value = value_json(&m.value);
                if let Json::Object(map) = &mut value {
                    map.insert("step".into(), json!(m.step));
                    map.insert("path".into(), json!(m.path));
                }
                value
            }).collect::<Vec<_>>(),
            "steps": self.steps.iter().map(|s| {
                let (status, reason, missing) = status_json(&s.status, "runs", "blocked");
                json!({
                    "id": s.id,
                    "verb": s.verb,
                    "operand": s.operand,
                    "status": status,
                    "reason": reason,
                    "missing": missing,
                    "detail": s.detail,
                })
            }).collect::<Vec<_>>(),
            "outputs": self.outputs.iter().map(|o| {
                let (status, _, missing) = status_json(&o.status, "ready", "blocked");
                json!({
                    "kind": o.kind,
                    "id": o.id,
                    "label": o.label,
                    "from": o.from,
                    "optional": o.optional,
                    "status": status,
                    "missing": missing,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

impl PayloadPlan {
    /// The output, as the plan and the build name it: `medium disk-1
    /// (DISK1.IMG)`; a ROM's chips by their file names.
    pub fn what(&self) -> String {
        format!("{} {} ({})", self.kind, self.id, self.name)
    }
}

/// What `upcli recipe plan --json` prints for a recipe that does not pass
/// the checks: `complete` false, a summary and the findings as `recipe
/// check` prints them.
pub fn refused_json(file: &str, report: &crate::Report) -> String {
    let findings: Vec<String> = report.findings.iter().map(ToString::to_string).collect();
    let summary = format!(
        "refused: {} finding{}",
        findings.len(),
        if findings.len() == 1 { "" } else { "s" }
    );
    format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({
            "recipe": file,
            "complete": false,
            "summary": summary,
            "findings": findings,
        }))
        .unwrap_or_default()
    )
}

fn status(status: &Status, ready: &str, blocked: &str) -> String {
    match status {
        Status::Ready => ready.to_owned(),
        Status::NotNeeded(why) => format!("not needed: {why}"),
        Status::Missing(ids) => format!("{blocked}: missing {}", ids.join(", ")),
    }
}

fn status_json(
    status: &Status,
    ready: &str,
    blocked: &str,
) -> (String, Option<String>, Vec<String>) {
    match status {
        Status::Ready => (ready.to_owned(), None, Vec::new()),
        Status::NotNeeded(why) => ("not-needed".into(), Some(why.clone()), Vec::new()),
        Status::Missing(ids) => (blocked.to_owned(), None, ids.clone()),
    }
}

/// Where a file set's found files were read from: the number of distinct
/// places, per source.
fn places(files: &[(String, Value)]) -> String {
    let mut by_source: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for found in files.iter().filter_map(|(_, v)| v.found.as_ref()) {
        let (source, place) = match found {
            Found::Store { tags } => ("the store".to_owned(), tags.join(",")),
            Found::Download { path } => {
                ("the download cache".to_owned(), path.display().to_string())
            }
            Found::Bundle { path } => ("the bundle".to_owned(), path.display().to_string()),
            Found::Library { library, place } => {
                (format!("library {}", library.display()), place.to_string())
            }
        };
        by_source.entry(source).or_default().insert(place);
    }
    if by_source.is_empty() {
        return String::new();
    }
    let total: usize = by_source.values().map(BTreeSet::len).sum();
    let parts: Vec<String> = by_source
        .iter()
        .map(|(source, places)| format!("{source}: {}", places.len()))
        .collect();
    format!(", read from {total} places ({})", parts.join("; "))
}

fn misses(out: &mut String, value: &Value, indent: &str) {
    for miss in &value.near_misses {
        out.push_str(&format!(
            "{indent}near miss ({}): library {}: {}, {} bytes{}\n",
            miss.kind.text(),
            miss.library.display(),
            miss.place,
            miss.size,
            miss.sha256
                .as_ref()
                .map(|s| format!(", sha256 {s}"))
                .unwrap_or_default()
        ));
    }
}

fn found_json(found: &Found) -> Json {
    match found {
        Found::Store { tags } => json!({"source": "store", "tags": tags}),
        Found::Download { path } => {
            json!({"source": "download-cache", "path": path.display().to_string()})
        }
        Found::Bundle { path } => json!({"source": "bundle", "path": path.display().to_string()}),
        Found::Library { library, place } => json!({
            "source": "library",
            "library": library.display().to_string(),
            "path": place.path,
            "member": place.member,
        }),
    }
}

fn value_json(value: &Value) -> Json {
    json!({
        "size": value.size,
        "sha256": value.sha256,
        "found": value.found.as_ref().map(found_json),
        "near_misses": value.near_misses.iter().map(|m| json!({
            "kind": m.kind.key(),
            "library": m.library.display().to_string(),
            "path": m.place.path,
            "member": m.place.member,
            "size": m.size,
            "sha256": m.sha256,
        })).collect::<Vec<_>>(),
    })
}

fn input_json(input: &InputPlan) -> Json {
    let (status, reason, _) = status_json(&input.status, "found", "missing");
    let mut object = json!({
        "id": input.id,
        "name": input.name,
        "optional": input.optional,
        "status": status,
        "reason": reason,
        "used_by": input.used_by,
        "urls": input.urls,
        "ids": input.ids.iter().map(|(db, set, name)| json!({"db": db, "set": set, "name": name})).collect::<Vec<_>>(),
    });
    let Json::Object(map) = &mut object else {
        return object;
    };
    match &input.form {
        Form::File(value) => {
            map.insert("form".into(), json!("file"));
            if let Json::Object(value) = value_json(value) {
                map.extend(value);
            }
        }
        Form::Files(files) => {
            map.insert("form".into(), json!("files"));
            map.insert(
                "found_files".into(),
                json!(files.iter().filter(|(_, v)| v.found.is_some()).count()),
            );
            let places: BTreeSet<String> = files
                .iter()
                .filter_map(|(_, v)| v.found.as_ref().map(|f| f.to_string()))
                .collect();
            map.insert("places".into(), json!(places.len()));
            map.insert(
                "files".into(),
                Json::Array(
                    files
                        .iter()
                        .map(|(path, value)| {
                            let mut v = value_json(value);
                            if let Json::Object(m) = &mut v {
                                m.insert("path".into(), json!(path));
                            }
                            v
                        })
                        .collect(),
                ),
            );
        }
    }
    object
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Document;
    use sha2::{Digest as _, Sha256};
    use std::fs;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("recipe-plan-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn recipe(text: &str) -> Recipe {
        match crate::parse_text(text) {
            Some(Document::Recipe(recipe)) => *recipe,
            _ => panic!("not a recipe:\n{text}"),
        }
    }

    const HEAD: &str = "recipe = \"0.1\"\n\n[media]\nname = \"private/media/ibm-pc/x\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n[title]\ntitle = \"X\"\n\n";

    /// A recipe that unpacks two members of GAME.ZIP and builds a disk from
    /// them, and the bytes of the members.
    fn unpacking() -> (Recipe, Vec<u8>, Vec<u8>) {
        let (a, b) = (b"first member".to_vec(), b"second member".to_vec());
        let text = format!(
            "{HEAD}[input.game]\nname = \"GAME.ZIP\"\nsize = 999\nsha256 = \"{zip}\"\nurls = [\"https://example.org/GAME.ZIP\"]\n\n\
             [[step]]\nid = \"files\"\nunpack = \"game\"\nformat = \"zip\"\nmembers = [\n\
             {{ path = \"A.EXE\", size = {la}, sha256 = \"{ha}\" }},\n\
             {{ path = \"DATA/B.DAT\", size = {lb}, sha256 = \"{hb}\" }},\n]\n\n\
             [[step]]\nid = \"disk\"\nfat = \"files\"\nformat = \"1.44M\"\n\n\
             [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"1.44M\"\nfrom = \"disk\"\n\n\
             [[extra]]\nid = \"readme\"\nfile_name = \"A.EXE\"\nfrom = \"files/A.EXE\"\n",
            zip = sha(b"zip"),
            la = a.len(),
            ha = sha(&a),
            lb = b.len(),
            hb = sha(&b),
        );
        (recipe(&text), a, b)
    }

    fn library(dir: &Path) -> Library {
        Library::scan(dir, None).unwrap()
    }

    #[test]
    fn members_found_in_a_library_make_the_archive_not_needed() {
        let temp = Temp::new("members");
        let lib = temp.0.join("lib");
        fs::create_dir_all(lib.join("loose")).unwrap();
        let (recipe, a, b) = unpacking();
        fs::write(lib.join("loose/A.EXE"), &a).unwrap();
        fs::write(lib.join("loose/B.DAT"), &b).unwrap();
        let mut libraries = [library(&lib)];
        let plan = plan(
            &recipe,
            "x.media.toml",
            None,
            Sources {
                store: None,
                downloads: None,
                libraries: &mut libraries,
            },
        );
        assert_eq!(
            plan.inputs[0].status,
            Status::NotNeeded("its 2 members were found".into())
        );
        assert_eq!(plan.members.len(), 2);
        assert!(plan.members.iter().all(|m| m.value.found.is_some()));
        assert_eq!(
            plan.steps
                .iter()
                .map(|s| s.status.clone())
                .collect::<Vec<_>>(),
            [
                Status::NotNeeded("its 2 members were found".into()),
                Status::Ready
            ]
        );
        assert!(plan.outputs.iter().all(|o| o.status == Status::Ready));
        assert!(plan.complete());
        assert_eq!(plan.summary(), "can build: all 1 input found or not needed");
        let text = plan.render();
        assert!(
            text.contains("  game (GAME.ZIP): not needed: its 2 members were found\n"),
            "{text}"
        );
        assert!(text.contains("  files/A.EXE: found in library "), "{text}");
    }

    #[test]
    fn a_member_still_missing_opens_the_archive_and_a_missing_archive_blocks() {
        let temp = Temp::new("partial");
        let lib = temp.0.join("lib");
        fs::create_dir_all(&lib).unwrap();
        let (recipe, a, _) = unpacking();
        fs::write(lib.join("A.EXE"), &a).unwrap();
        let mut libraries = [library(&lib)];
        let plan = plan(
            &recipe,
            "x.media.toml",
            None,
            Sources {
                store: None,
                downloads: None,
                libraries: &mut libraries,
            },
        );
        assert_eq!(plan.inputs[0].status, Status::Missing(Vec::new()));
        assert_eq!(
            plan.inputs[0].used_by,
            ["Disk 1"],
            "the extra's member was found"
        );
        assert_eq!(plan.steps[0].status, Status::Missing(vec!["game".into()]));
        assert_eq!(
            plan.steps[0].detail.as_deref(),
            Some("opens game for 1 of 2 members")
        );
        assert_eq!(plan.outputs[0].status, Status::Missing(vec!["game".into()]));
        assert_eq!(plan.outputs[1].status, Status::Ready);
        assert!(!plan.complete());
        assert_eq!(plan.summary(), "missing 1 of 1 input");
        let text = plan.render();
        assert!(
            text.contains("    url: https://example.org/GAME.ZIP\n"),
            "{text}"
        );
    }

    #[test]
    fn a_near_miss_is_reported_and_never_used() {
        let temp = Temp::new("near");
        let lib = temp.0.join("lib");
        fs::create_dir_all(&lib).unwrap();
        let wanted = vec![7u8; 20];
        let text = format!(
            "{HEAD}[input.disk]\nname = \"DISK1.IMG\"\nsize = 20\nsha256 = \"{}\"\n\n\
             [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1 of 1\"\nformat = \"1.44M\"\nfrom = \"disk\"\n",
            sha(&wanted)
        );
        fs::write(lib.join("disk1.img"), [1u8; 10]).unwrap();
        fs::write(lib.join("copy.img"), [2u8; 20]).unwrap();
        let mut libraries = [library(&lib)];
        let plan = plan(
            &recipe(&text),
            "x.media.toml",
            None,
            Sources {
                store: None,
                downloads: None,
                libraries: &mut libraries,
            },
        );
        let Form::File(value) = &plan.inputs[0].form else {
            panic!()
        };
        assert_eq!(value.found, None);
        let kinds: Vec<(String, MissKind)> = value
            .near_misses
            .iter()
            .map(|m| (m.place.to_string(), m.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("disk1.img".to_owned(), MissKind::NameOtherSize),
                ("copy.img".to_owned(), MissKind::SizeOtherHash)
            ]
        );
        let text = plan.render();
        assert!(
            text.contains("    near miss (same name, another size): library "),
            "{text}"
        );
        assert!(text.contains("    used for: Disk 1 of 1\n"), "{text}");
        let json = plan.json();
        assert_eq!(
            json["inputs"][0]["near_misses"][0]["kind"],
            "name-other-size"
        );
        assert_eq!(json["inputs"][0]["status"], "missing");
        assert_eq!(json["complete"], false);
    }

    #[test]
    fn the_store_is_searched_first_and_a_bundle_before_libraries() {
        let temp = Temp::new("order");
        let lib = temp.0.join("lib");
        fs::create_dir_all(&lib).unwrap();
        let (disk, text) = (vec![1u8; 30], b"our own text".to_vec());
        fs::write(lib.join("DISK.IMG"), &disk).unwrap();
        fs::write(lib.join("notes.txt"), &text).unwrap();
        fs::write(temp.0.join("notes.txt"), &text).unwrap();
        let source = format!(
            "{HEAD}[input.disk]\nname = \"DISK.IMG\"\nsize = 30\nsha256 = \"{}\"\n\n\
             [input.notes]\nname = \"notes.txt\"\nsize = {}\nsha256 = \"{}\"\nbundle = true\n\n\
             [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"1.44M\"\nfrom = \"disk\"\n\n\
             [[extra]]\nid = \"notes\"\nfile_name = \"NOTES.TXT\"\nfrom = \"notes\"\n",
            sha(&disk),
            text.len(),
            sha(&text)
        );
        let store = StoreIndex::of(&[(&sha(&disk), 30, "private/media/ibm-pc/x:1")]);
        let mut libraries = [library(&lib)];
        let plan = plan(
            &recipe(&source),
            "x.media.toml",
            Some(&temp.0),
            Sources {
                store: Some(&store),
                downloads: None,
                libraries: &mut libraries,
            },
        );
        let found: Vec<Option<Found>> = plan
            .inputs
            .iter()
            .map(|i| match &i.form {
                Form::File(v) => v.found.clone(),
                Form::Files(_) => None,
            })
            .collect();
        assert_eq!(
            found,
            [
                Some(Found::Store {
                    tags: vec!["private/media/ibm-pc/x:1".into()]
                }),
                Some(Found::Bundle {
                    path: temp.0.join("notes.txt")
                })
            ]
        );
        assert!(
            plan.libraries[0].stats.hashed.is_empty(),
            "no library file was hashed"
        );
        assert_eq!(plan.summary(), "can build: all 2 inputs found");
    }

    #[test]
    fn a_file_set_is_found_file_by_file_and_the_json_keys_are_stable() {
        let temp = Temp::new("set");
        let lib = temp.0.join("lib");
        fs::create_dir_all(lib.join("GAME/L1")).unwrap();
        fs::create_dir_all(lib.join("GAME/L2")).unwrap();
        // Two entries share their bytes: one file serves both.
        fs::write(lib.join("GAME/L1/A.BIN"), [1u8; 8]).unwrap();
        fs::write(lib.join("GAME/L2/A.BIN"), [1u8; 8]).unwrap();
        fs::write(lib.join("GAME/RUN.COM"), [2u8; 4]).unwrap();
        let text = format!(
            "{HEAD}[input.game]\nname = \"GAME\"\nfiles = [\n\
             {{ path = \"RUN.COM\", size = 4, sha256 = \"{run}\" }},\n\
             {{ path = \"L1/A.BIN\", size = 8, sha256 = \"{a}\" }},\n\
             {{ path = \"L2/A.BIN\", size = 8, sha256 = \"{a}\" }},\n]\n\n\
             [[step]]\nid = \"disk\"\nfat = \"game\"\nformat = \"1.44M\"\n\n\
             [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"1.44M\"\nfrom = \"disk\"\n",
            run = sha(&[2u8; 4]),
            a = sha(&[1u8; 8]),
        );
        let mut libraries = [library(&lib)];
        let plan = plan(
            &recipe(&text),
            "x.media.toml",
            None,
            Sources {
                store: None,
                downloads: None,
                libraries: &mut libraries,
            },
        );
        assert_eq!(plan.inputs[0].status, Status::Ready);
        assert_eq!(
            plan.libraries[0].stats.hashed.len(),
            2,
            "each content hashed once"
        );
        let json = plan.json();
        let keys = |v: &Json| -> Vec<String> { v.as_object().unwrap().keys().cloned().collect() };
        assert_eq!(
            keys(&json),
            [
                "complete",
                "inputs",
                "kind",
                "libraries",
                "members",
                "name",
                "outputs",
                "recipe",
                "steps",
                "store",
                "store_warnings",
                "summary"
            ]
        );
        assert_eq!(
            keys(&json["inputs"][0]),
            [
                "files",
                "form",
                "found_files",
                "id",
                "ids",
                "name",
                "optional",
                "places",
                "reason",
                "status",
                "urls",
                "used_by"
            ]
        );
        assert_eq!(json["inputs"][0]["found_files"], 3);
        assert_eq!(json["inputs"][0]["places"], 2);
        assert_eq!(
            keys(&json["inputs"][0]["files"][0]),
            ["found", "near_misses", "path", "sha256", "size"]
        );
        assert_eq!(
            keys(&json["inputs"][0]["files"][0]["found"]),
            ["library", "member", "path", "source"]
        );
        assert_eq!(
            keys(&json["steps"][0]),
            ["detail", "id", "missing", "operand", "reason", "status", "verb"]
        );
        assert_eq!(
            keys(&json["outputs"][0]),
            ["from", "id", "kind", "label", "missing", "optional", "status"]
        );
        let text = plan.render();
        assert!(
            text.contains("  game (GAME): found 3 of 3 files, read from 2 places (library "),
            "{text}"
        );
    }
}
