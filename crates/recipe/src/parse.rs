// SPDX-License-Identifier: AGPL-3.0-or-later
//! From parsed TOML to the typed model (recipes §6.1 to §6.10, §6.16).
//!
//! The tables of [`crate::spec`] are checked first, all at once: every
//! unknown key, missing key, wrong type and value outside its form is a
//! finding of rule 1 with its position and key path, and the value is left
//! out. The model is then built from what passed. Which identity table a
//! file has, and the tables of the other kind, are rule 2.

use crate::finding::{Rule, Sink};
use crate::model::*;
use crate::spec::{self, MapKey, Table, Ty};
use crate::syntax::push_key;
use std::ops::Range;
use toml::de::{DeTable, DeValue};
use toml::Spanned;

/// A value that passed its check.
pub(crate) struct Node {
    span: Range<usize>,
    path: String,
    /// For a table of a [`Ty::OneOf`], the variant it was read as.
    variant: Option<&'static str>,
    value: V,
}

enum V {
    Str(String),
    Int(i64),
    Bool(bool),
    Time(LocalTime),
    Arr(Vec<Node>),
    Tab(Vec<(At<String>, Node)>),
}

fn key_path(path: &str, key: &str) -> String {
    let mut out = path.to_owned();
    if !out.is_empty() {
        out.push('.');
    }
    push_key(&mut out, key);
    out
}

fn item_path(path: &str, index: usize) -> String {
    format!("{path}[{}]", index + 1)
}

/// What a TOML value is, for a message.
fn found(value: &DeValue<'_>) -> &'static str {
    match value {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) => "an integer",
        DeValue::Float(_) => "a float",
        DeValue::Boolean(_) => "a boolean",
        DeValue::Datetime(d) => match (d.date, d.time, d.offset) {
            (_, _, Some(_)) => "an offset date-time",
            (Some(_), None, None) => "a date",
            (None, Some(_), None) => "a time",
            _ => "a local date-time",
        },
        DeValue::Array(_) => "an array",
        DeValue::Table(_) => "a table",
    }
}

fn expected(ty: &Ty) -> &'static str {
    match ty {
        Ty::Str(_) | Ty::Enum(_) | Ty::Vocab(_) => "a string",
        Ty::Int { .. } => "an integer",
        Ty::Bool => "a boolean",
        Ty::Time => "a local date-time",
        Ty::Array { .. } => "an array",
        Ty::Table(_) | Ty::OneOf(_) | Ty::Map { .. } => "a table",
        Ty::Needs => "a string or an array of strings",
        Ty::FatOperand => "a tree reference or an array of file-input placements",
    }
}

struct Checker<'a, 's> {
    sink: &'a mut Sink<'s>,
    version: String,
}

impl Checker<'_, '_> {
    fn finding(&mut self, span: &Range<usize>, path: &str, message: impl Into<String>) {
        self.sink.report(Rule::Syntax, span, path, message);
    }

    fn wrong_type(&mut self, value: &Spanned<DeValue<'_>>, path: &str, ty: &Ty) {
        let message = format!(
            "expected {}, found {}",
            expected(ty),
            found(value.get_ref())
        );
        self.finding(&value.span(), path, message);
    }

    fn value(&mut self, value: &Spanned<DeValue<'_>>, path: &str, ty: &'static Ty) -> Option<Node> {
        let span = value.span();
        let node = |v| Node {
            span: span.clone(),
            path: path.to_owned(),
            variant: None,
            value: v,
        };
        match (ty, value.get_ref()) {
            (Ty::FatOperand, _) => self.value(
                value,
                path,
                if matches!(value.get_ref(), DeValue::Array(_)) {
                    &spec::PLACEMENTS
                } else {
                    &REF
                },
            ),
            (Ty::Str(form), DeValue::String(text)) => {
                let text = self.line_ends(&span, text);
                match form.fault(&text) {
                    None => Some(node(V::Str(text))),
                    Some(fault) => {
                        self.finding(&span, path, format!("{text:?}: {fault}"));
                        None
                    }
                }
            }
            (Ty::Enum(names), DeValue::String(text)) => {
                if names.contains(&text.as_ref()) {
                    Some(node(V::Str(text.to_string())))
                } else {
                    let message = if path == "recipe" && text == "0.1" {
                        "recipe \"0.1\" is the experimental format; this unemu reads recipe \"1.0\": change recipe = \"0.1\" to \"1.0\" (and the work file's version) and check the recipe with unemu recipe check before pinning".to_owned()
                    } else {
                        format!(
                            "{text:?} is not one of {}",
                            // Preserve experimental-version findings in the frozen corpus.
                            if path == "recipe" && !text.starts_with("0.") {
                                spec::VERSIONS
                            } else {
                                names
                            }
                            .join(", ")
                        )
                    };
                    self.finding(&span, path, message);
                    None
                }
            }
            (Ty::Vocab(_), DeValue::String(text)) => {
                if text.is_empty() {
                    self.finding(&span, path, "expected a name, found an empty string");
                    None
                } else {
                    Some(node(V::Str(text.to_string())))
                }
            }
            (Ty::Int { min, max }, DeValue::Integer(integer)) => {
                let parsed = i64::from_str_radix(integer.as_str(), integer.radix()).ok();
                match parsed.filter(|n| (*min..=*max).contains(n)) {
                    Some(n) => Some(node(V::Int(n))),
                    None => {
                        let range = if *max == i64::MAX {
                            format!("of at least {min}")
                        } else {
                            format!("from {min} to {max}")
                        };
                        self.finding(
                            &span,
                            path,
                            format!("expected an integer {range}, found {integer}"),
                        );
                        None
                    }
                }
            }
            (Ty::Bool, DeValue::Boolean(b)) => Some(node(V::Bool(*b))),
            (Ty::Time, DeValue::Datetime(datetime)) => match local_time(datetime) {
                Ok(time) => Some(node(V::Time(time))),
                Err(fault) => {
                    self.finding(&span, path, fault);
                    None
                }
            },
            (Ty::Array { item, min }, DeValue::Array(items)) => {
                // In a recipe's [title] only the credits and genres take at
                // least one item: an empty list there would state nothing
                // (recipes §6.16), so the message says what to write instead.
                if items.is_empty() && *min > 0 && path.starts_with("title.") {
                    self.finding(
                        &span,
                        path,
                        "an empty list states nothing: leave the key out, which keeps the work file's list",
                    );
                } else if items.len() < *min {
                    self.finding(&span, path, format!("expected at least {min} item"));
                }
                let nodes = items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, value)| self.value(value, &item_path(path, index), item))
                    .collect();
                Some(node(V::Arr(nodes)))
            }
            (Ty::Table(spec), DeValue::Table(table)) => {
                Some(self.table(table, span, path, spec, &[]))
            }
            (Ty::OneOf(variants), DeValue::Table(table)) => {
                Some(self.one_of(table, span, path, variants))
            }
            (Ty::Map { key, value: item }, DeValue::Table(table)) => {
                let mut entries = Vec::new();
                for (name, value) in table {
                    let child = key_path(path, name.get_ref());
                    let good = match key {
                        MapKey::Id => image_store::media_set::valid_id(name.get_ref()),
                        MapKey::Kind => image_store::needs::valid_kind(name.get_ref()),
                    };
                    if !good {
                        let form = match key {
                            MapKey::Id => spec::Text::Id.expected(),
                            MapKey::Kind => "expected a name of lower-case letters, digits and '_', starting with a letter, at most 32 characters",
                        };
                        self.finding(
                            &name.span(),
                            &child,
                            format!("key {:?}: {form}", name.get_ref()),
                        );
                        continue;
                    }
                    let at = At {
                        value: name.get_ref().to_string(),
                        span: name.span(),
                        path: child.clone(),
                    };
                    // A value that is refused still names its key, so that
                    // references to it are not reported again.
                    let checked = self.value(value, &child, item).unwrap_or(Node {
                        span: value.span(),
                        path: child,
                        variant: None,
                        value: V::Tab(Vec::new()),
                    });
                    entries.push((at, checked));
                }
                Some(node(V::Tab(entries)))
            }
            (Ty::Needs, DeValue::String(_)) => self
                .need_value(value, path)
                .map(|one| node(V::Arr(vec![one]))),
            (Ty::Needs, DeValue::Array(items)) => {
                if items.is_empty() {
                    self.finding(&span, path, "a need states at least one value");
                    return None;
                }
                let mut nodes: Vec<Node> = Vec::new();
                for (index, value) in items.iter().enumerate() {
                    let child = item_path(path, index);
                    if let Some(one) = self.need_value(value, &child) {
                        if nodes.iter().any(|n| same_str(n, &one)) {
                            self.finding(&one.span, &child, "the value is repeated");
                        } else {
                            nodes.push(one);
                        }
                    }
                }
                Some(node(V::Arr(nodes)))
            }
            _ => {
                self.wrong_type(value, path, ty);
                None
            }
        }
    }

    /// A multi-line string of a file saved with CR LF line ends holds CR LF
    /// where it breaks a line; TOML lets a parser read that as one line
    /// break, and so does a recipe. Any other CR stays and is refused.
    fn line_ends(&self, span: &Range<usize>, text: &str) -> String {
        let raw = self.sink.source.text.get(span.clone()).unwrap_or("");
        if raw.starts_with("\"\"\"") || raw.starts_with("'''") {
            text.replace("\r\n", "\n")
        } else {
            text.to_owned()
        }
    }

    fn need_value(&mut self, value: &Spanned<DeValue<'_>>, path: &str) -> Option<Node> {
        let DeValue::String(text) = value.get_ref() else {
            self.wrong_type(value, path, &Ty::Str(spec::Text::Line));
            return None;
        };
        if !image_store::needs::valid_value(text) {
            self.finding(
                &value.span(),
                path,
                format!("{text:?}: expected ASCII letters, digits, '+', '.', '_' and '-', starting with a letter or digit, at most 32 characters"),
            );
            return None;
        }
        Some(Node {
            span: value.span(),
            path: path.to_owned(),
            variant: None,
            value: V::Str(text.to_string()),
        })
    }

    /// Check a table against `spec`. Keys in `skip` were reported already.
    fn table(
        &mut self,
        table: &DeTable<'_>,
        span: Range<usize>,
        path: &str,
        spec: &'static Table,
        skip: &[&str],
    ) -> Node {
        let spec = spec::version_table(spec, &self.version);
        let mut entries = Vec::new();
        for (key, value) in table {
            let name = key.get_ref().as_ref();
            if skip.contains(&name) {
                continue;
            }
            let child = key_path(path, name);
            let Some(field) = spec.field(name) else {
                self.finding(
                    &key.span(),
                    &child,
                    format!(
                        "unknown key `{name}`; expected one of {}",
                        spec.keys().join(", ")
                    ),
                );
                continue;
            };
            if spec.name == "fat-step"
                && name == "fat"
                && self.version == "1.0"
                && matches!(value.get_ref(), DeValue::Array(_))
            {
                self.finding(
                    &value.span(),
                    &child,
                    "the FAT placement array requires recipe 1.1",
                );
                continue;
            }
            if let Some(node) = self.value(value, &child, &field.ty) {
                entries.push((
                    At {
                        value: name.to_owned(),
                        span: key.span(),
                        path: child,
                    },
                    node,
                ));
            }
        }
        for field in spec.fields.iter().filter(|f| f.required) {
            if !table.contains_key(field.key) && !skip.contains(&field.key) {
                self.finding(&span, path, format!("missing key `{}`", field.key));
            }
        }
        Node {
            span,
            path: path.to_owned(),
            variant: Some(spec.name),
            value: V::Tab(entries),
        }
    }

    /// Check a table of one of `variants`, chosen by its marker key. With
    /// several markers, or none and no default, the table keeps only its
    /// `id` and the strings its markers hold, as a variant of none.
    fn one_of(
        &mut self,
        table: &DeTable<'_>,
        span: Range<usize>,
        path: &str,
        variants: &'static [&'static Table],
    ) -> Node {
        let markers: Vec<&'static str> = variants.iter().filter_map(|v| v.marker).collect();
        let present: Vec<&'static str> = markers
            .iter()
            .copied()
            .filter(|m| table.contains_key(*m))
            .collect();
        let default = variants.iter().find(|v| v.marker.is_none());
        let chosen = match (present.as_slice(), default) {
            ([one], _) => variants.iter().find(|v| v.marker == Some(one)),
            ([], Some(default)) => Some(default),
            _ => None,
        };
        if let Some(variant) = chosen {
            return self.table(table, span, path, variant, &[]);
        }
        let has = if present.is_empty() {
            "none".to_owned()
        } else {
            present.join(" and ")
        };
        self.finding(
            &span,
            path,
            format!(
                "expected exactly one of {}; this table has {has}",
                markers.join(", ")
            ),
        );
        let mut entries = Vec::new();
        for (key, value) in table {
            let name = key.get_ref().as_ref();
            if name != "id" && !present.contains(&name) {
                continue;
            }
            let child = key_path(path, name);
            let ty = if name == "id" { &ID } else { &REF };
            if let Some(node) = self.value(value, &child, ty) {
                entries.push((
                    At {
                        value: name.to_owned(),
                        span: key.span(),
                        path: child,
                    },
                    node,
                ));
            }
        }
        Node {
            span,
            path: path.to_owned(),
            variant: None,
            value: V::Tab(entries),
        }
    }
}

static ID: Ty = Ty::Str(spec::Text::Id);
static REF: Ty = Ty::Str(spec::Text::Ref);

fn same_str(a: &Node, b: &Node) -> bool {
    matches!((&a.value, &b.value), (V::Str(x), V::Str(y)) if x == y)
}

fn local_time(datetime: &toml::value::Datetime) -> Result<LocalTime, String> {
    let (Some(date), Some(time), None) = (datetime.date, datetime.time, datetime.offset) else {
        return Err(format!(
            "expected a local date-time such as 1993-03-27T13:58:22, found {datetime}"
        ));
    };
    let second = time.second.unwrap_or(0);
    if time.nanosecond.is_some() || !second.is_multiple_of(2) {
        return Err(format!(
            "{datetime}: a time has whole, even seconds, as FAT and zip keep it"
        ));
    }
    if second > 58 {
        return Err(format!(
            "{datetime}: the second is at most 58, as FAT and zip keep it"
        ));
    }
    if !(1980..=2107).contains(&date.year) {
        return Err(format!(
            "{datetime}: the year is from 1980 to 2107, which FAT and zip hold"
        ));
    }
    Ok(LocalTime {
        year: date.year,
        month: date.month,
        day: date.day,
        hour: time.hour,
        minute: time.minute,
        second,
    })
}

impl Node {
    fn entries(&self) -> &[(At<String>, Node)] {
        match &self.value {
            V::Tab(entries) => entries,
            _ => &[],
        }
    }

    fn items(&self) -> &[Node] {
        match &self.value {
            V::Arr(items) => items,
            _ => &[],
        }
    }

    fn get(&self, key: &str) -> Option<&Node> {
        self.entries()
            .iter()
            .find(|(k, _)| k.value == key)
            .map(|(_, node)| node)
    }

    fn at(&self) -> At<()> {
        self.with(())
    }

    fn with<T>(&self, value: T) -> At<T> {
        At {
            value,
            span: self.span.clone(),
            path: self.path.clone(),
        }
    }

    fn string(&self) -> Option<At<String>> {
        match &self.value {
            V::Str(s) => Some(self.with(s.clone())),
            _ => None,
        }
    }

    fn str(&self, key: &str) -> Option<At<String>> {
        self.get(key).and_then(Node::string)
    }

    fn strings(&self, key: &str) -> Vec<At<String>> {
        self.get(key)
            .map(|n| n.items().iter().filter_map(Node::string).collect())
            .unwrap_or_default()
    }

    fn int(&self, key: &str) -> Option<At<i64>> {
        match self.get(key)? {
            node @ Node {
                value: V::Int(n), ..
            } => Some(node.with(*n)),
            _ => None,
        }
    }

    fn size(&self, key: &str) -> Option<At<u64>> {
        self.int(key).and_then(|n| {
            let value = u64::try_from(n.value).ok()?;
            Some(At {
                value,
                span: n.span,
                path: n.path,
            })
        })
    }

    fn boolean(&self, key: &str) -> Option<At<bool>> {
        match self.get(key)? {
            node @ Node {
                value: V::Bool(b), ..
            } => Some(node.with(*b)),
            _ => None,
        }
    }

    fn time(&self, key: &str) -> Option<At<LocalTime>> {
        match self.get(key)? {
            node @ Node {
                value: V::Time(t), ..
            } => Some(node.with(*t)),
            _ => None,
        }
    }
}

/// The `recipe` key as written, even when rule 1 refuses its value: a work
/// file of another version is reported as such (recipes §6.16).
fn version(table: &DeTable<'_>) -> Option<At<String>> {
    let (key, value) = table.get_key_value("recipe")?;
    match value.get_ref() {
        DeValue::String(text) => Some(At {
            value: text.to_string(),
            span: value.span(),
            path: key.get_ref().to_string(),
        }),
        _ => None,
    }
}

/// Read one file: a recipe or a work file. `None` when it has no identity
/// table to tell its kind by.
pub(crate) fn document(sink: &mut Sink<'_>, root: &Spanned<DeTable<'_>>) -> Option<Document> {
    let table = root.get_ref();
    let mut checker = Checker {
        sink,
        version: version(table).map(|v| v.value).unwrap_or_default(),
    };
    if !(checker.version == "1.1" && table.contains_key("machine"))
        && table
            .get("work")
            .is_some_and(|w| matches!(w.get_ref(), DeValue::Table(_)))
    {
        let node = checker.table(table, 0..0, "", &spec::WORK_FILE, &[]);
        let mut work = work_file(&node);
        work.version = version(table);
        return Some(Document::Work(Box::new(work)));
    }
    let mut identities: Vec<(&'static str, Range<usize>)> = table
        .iter()
        .filter_map(|(k, _)| {
            let name = spec::IDENTITIES
                .iter()
                .find(|i| **i == k.get_ref().as_ref())?;
            Some((*name, k.span()))
        })
        .collect();
    identities.sort_by_key(|(_, span)| span.start);
    let mut skip: Vec<&'static str> = Vec::new();
    if let Some((_, span)) = identities
        .iter()
        .find(|(name, _)| *name == "machine" && checker.version != "1.1")
    {
        checker.sink.report(
            Rule::Kind,
            span,
            "machine",
            "[machine] is reserved for the Machinefile, which recipe 1.0 does not specify",
        );
        skip.push("machine");
    }
    let kinds: Vec<&(&'static str, Range<usize>)> = identities
        .iter()
        .filter(|(name, _)| *name != "machine" || checker.version == "1.1")
        .collect();
    let Some((first, _)) = kinds.first() else {
        if skip.is_empty() {
            checker.sink.report(
                Rule::Kind,
                &(0..0),
                "",
                "no identity table: a recipe has [media] or [firmware], a work file [work]",
            );
        }
        return None;
    };
    let kind = match *first {
        "media" => Kind::Media,
        "machine" => Kind::Machine,
        _ => Kind::Firmware,
    };
    for (name, span) in kinds.iter().skip(1) {
        checker.sink.report(
            Rule::Kind,
            span,
            name,
            format!("a recipe has one identity table; this one has [{first}] and [{name}]"),
        );
        skip.push(name);
    }
    let (spec, foreign, other): (&'static Table, &'static [&'static str], &str) = match kind {
        Kind::Media => (&spec::MEDIAFILE, spec::FIRMWARE_ONLY, "a Firmwarefile"),
        Kind::Firmware => (&spec::FIRMWAREFILE, spec::MEDIA_ONLY, "a Mediafile"),
        Kind::Machine => (&spec::MACHINEFILE, &[], "another recipe kind"),
    };
    for (key, _) in table {
        let name = key.get_ref().as_ref();
        let Some(foreign) = foreign.iter().find(|f| **f == name) else {
            continue;
        };
        if !skip.contains(foreign) {
            let what = if name == "work" {
                "`work`".to_owned()
            } else {
                format!("[{name}]")
            };
            checker.sink.report(
                Rule::Kind,
                &key.span(),
                &key_path("", name),
                format!("{what} belongs to {other}; a recipe of [{first}] has none"),
            );
            skip.push(*foreign);
        }
    }
    let node = checker.table(table, 0..0, "", spec, &skip);
    let mut recipe = recipe(kind, &node);
    recipe.version = version(table);
    Some(Document::Recipe(Box::new(recipe)))
}

fn title(node: &Node) -> Title {
    Title {
        at: node.at(),
        title: node.str("title"),
        sort_title: node.str("sort_title"),
        series: node.str("series"),
        version: node.str("version"),
        edition: node.str("edition"),
        source_media: node.str("source_media"),
        description: node.str("description"),
        requirements: node.str("requirements"),
        synopsis: node.str("synopsis"),
        year: node.int("year"),
        publishers: node.strings("publishers"),
        developers: node.strings("developers"),
        genres: node.strings("genres"),
        languages: node.strings("languages"),
        regions: node.strings("regions"),
        software_platform: node.str("software_platform"),
        players: node.get("players").and_then(|p| {
            Some(p.with(Players {
                min: p.int("min")?.value,
                max: p.int("max")?.value,
            }))
        }),
        labels: node.strings("labels"),
    }
}

fn needs(node: Option<&Node>) -> Vec<Need> {
    node.map(|n| {
        n.entries()
            .iter()
            .map(|(kind, values)| Need {
                kind: kind.clone(),
                values: values.items().iter().filter_map(Node::string).collect(),
            })
            .collect()
    })
    .unwrap_or_default()
}

fn ids(node: Option<&Node>) -> Vec<WorkId> {
    node.map(|n| {
        n.entries()
            .iter()
            .filter_map(|(catalogue, id)| {
                Some(WorkId {
                    catalogue: catalogue.clone(),
                    id: id.string()?,
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

fn work_file(root: &Node) -> WorkFile {
    WorkFile {
        version: root.str("recipe"),
        id: root.get("work").and_then(|w| w.str("id")),
        title: root.get("title").map(title),
        ids: ids(root.get("ids")),
        needs: needs(root.get("needs")),
    }
}

fn extra(node: &Node) -> Extra {
    Extra {
        at: node.at(),
        id: node.str("id"),
        file_name: node.str("file_name"),
        kind: node.str("kind"),
        dos_name: node.str("dos_name"),
        from: node.str("from"),
    }
}

fn recipe(kind: Kind, root: &Node) -> Recipe {
    let identity = root.get(kind.key());
    let identity = Identity {
        at: identity.map_or_else(|| root.at(), Node::at),
        name: identity.and_then(|n| n.str("name")),
        platform: identity.and_then(|n| n.str("platform")),
        release: identity.and_then(|n| n.str("release")),
        version: identity.and_then(|n| n.str("version")),
        revision: identity.and_then(|n| n.int("revision")),
        digest: identity.and_then(|n| n.str("digest")),
        min_app_version: identity.and_then(|n| n.str("min_app_version")),
    };
    let inputs = root
        .get("input")
        .map(|n| {
            n.entries()
                .iter()
                .map(|(id, node)| input(id, node))
                .collect()
        })
        .unwrap_or_default();
    let steps = root
        .get("step")
        .map(|n| n.items().iter().map(step).collect())
        .unwrap_or_default();
    let media = root
        .get("medium")
        .map(|n| {
            n.items()
                .iter()
                .map(|m| Medium {
                    at: m.at(),
                    id: m.str("id"),
                    label: m.str("label"),
                    format: m.str("format"),
                    file_name: m.str("file_name"),
                    from: m.str("from"),
                })
                .collect()
        })
        .unwrap_or_default();
    let roms = root
        .get("rom")
        .map(|n| n.items().iter().map(rom).collect())
        .unwrap_or_default();
    let extras = root
        .get("extra")
        .map(|n| n.items().iter().map(extra).collect())
        .unwrap_or_default();
    let attachments = root
        .get("attachment")
        .map(|n| {
            n.items()
                .iter()
                .map(|a| Attachment {
                    at: a.at(),
                    id: a.str("id"),
                    title: a.str("title"),
                    digest: a.str("digest"),
                    extras: a
                        .get("extra")
                        .map(|e| e.items().iter().map(extra).collect())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    Recipe {
        kind,
        version: root.str("recipe"),
        work: root.str("work"),
        identity,
        template: root.get("template").map(|t| Template {
            config: t.str("config"),
            disks: t
                .get("disks")
                .map(|d| {
                    d.items()
                        .iter()
                        .map(|d| TemplateDisk {
                            id: d.str("id"),
                            size: d.size("size"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            title: t.str("title"),
            description: t.str("description"),
            labels: t.strings("labels"),
        }),
        uses: root
            .get("use")
            .map(|u| {
                u.entries()
                    .iter()
                    .map(|(id, u)| Use {
                        id: id.clone(),
                        image: u.str("image"),
                        digest: u.str("digest"),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        title: root.get("title").map(title),
        package: root.get("package").map(|p| {
            let fits = p.get("fits");
            Package {
                at: p.at(),
                title: p.str("title"),
                vendor: p.str("vendor"),
                version: p.str("version"),
                date: p.str("date"),
                description: p.str("description"),
                notes: p.str("notes"),
                labels: p.strings("labels"),
                boards: fits.map(|f| f.strings("boards")).unwrap_or_default(),
                machine_classes: fits
                    .map(|f| f.strings("machine_classes"))
                    .unwrap_or_default(),
            }
        }),
        license: root.get("license").map(|l| License {
            at: l.at(),
            expression: l.str("expression"),
            terms: l.str("terms"),
            source: l.str("source"),
        }),
        needs: needs(root.get("needs")),
        ids: ids(root.get("ids")),
        inputs,
        steps,
        media,
        roms,
        extras,
        attachments,
    }
}

fn input(id: &At<String>, node: &Node) -> Input {
    let form = if node.variant == Some(spec::FILE_SET_INPUT.name) {
        InputForm::FileSet {
            files: node
                .get("files")
                .map(|f| {
                    f.items()
                        .iter()
                        .filter_map(|e| {
                            Some(FileEntry {
                                path: e.str("path")?,
                                size: e.size("size"),
                                sha256: e.str("sha256"),
                                time: e.time("time"),
                                disclosable: e.boolean("disclosable"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    } else if node.variant.is_none() {
        InputForm::Refused
    } else {
        InputForm::File {
            size: node.size("size"),
            sha256: node.str("sha256"),
        }
    };
    Input {
        id: id.clone(),
        name: node.str("name"),
        form,
        names: node.strings("names"),
        crc32: node.str("crc32"),
        md5: node.str("md5"),
        sha1: node.str("sha1"),
        ids: node
            .get("ids")
            .map(|ids| {
                ids.items()
                    .iter()
                    .filter_map(|id| {
                        Some(DbId {
                            db: id.str("db")?,
                            set: id.str("set")?,
                            name: id.str("name")?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        urls: node.strings("urls"),
        bundle: node.boolean("bundle"),
        disclosable: node.boolean("disclosable"),
    }
}

fn step(node: &Node) -> Step {
    let op = match node.variant {
        Some(name) if name == spec::UNPACK.name => Op::Unpack {
            archive: node.str("unpack"),
            members: node
                .get("members")
                .map(|m| {
                    m.items()
                        .iter()
                        .filter_map(|m| {
                            Some(Member {
                                path: m.str("path")?,
                                size: m.size("size"),
                                sha256: m.str("sha256"),
                                time: m.time("time"),
                                disclosable: m.boolean("disclosable"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            complete: node.boolean("complete").is_some_and(|c| c.value),
        },
        Some(name) if name == spec::FAT.name => Op::Fat {
            tree: node.str("fat"),
            placements: node
                .get("fat")
                .filter(|n| matches!(n.value, V::Arr(_)))
                .map(|n| {
                    n.items()
                        .iter()
                        .map(|p| Placement {
                            from: p.str("from"),
                            dos_name: p.str("dos_name"),
                            time: p.time("time"),
                        })
                        .collect()
                }),
            format: node.str("format"),
            label: node.str("label"),
            serial: node.int("serial"),
            order: node.str("order"),
            time: node.time("time"),
            volumes: node.int("volumes"),
        },
        Some(name) if name == spec::TEXT.name => Op::Text {
            text: node.str("text"),
        },
        _ => Op::Unknown {
            references: spec::VERBS.iter().filter_map(|v| node.str(v)).collect(),
        },
    };
    Step {
        at: node.at(),
        id: node.str("id"),
        op,
    }
}

fn rom(node: &Node) -> Rom {
    let source = if node.variant == Some(spec::ROM_PARTS.name) {
        RomSource::Parts {
            parts: node
                .get("parts")
                .map(|p| {
                    p.items()
                        .iter()
                        .map(|p| Part {
                            at: p.at(),
                            position: p.str("position"),
                            part_number: p.str("part_number"),
                            lane: p.str("lane"),
                            offset: p.size("offset"),
                            from: p.str("from"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            size: node.int("size"),
            sha256: node.str("sha256"),
        }
    } else {
        RomSource::File(node.str("from"))
    };
    Rom {
        at: node.at(),
        id: node.str("id"),
        role: node.str("role"),
        catalogue_id: node.str("catalogue_id"),
        source,
    }
}
