// SPDX-License-Identifier: AGPL-3.0-or-later
//! The JSON Schema (draft 2020-12) of recipe version 0.1, written from the
//! key tables of [`crate::spec`] and the vocabularies of the providers, so
//! that it cannot drift from the checker (recipes §4.6, §19.2).
//!
//! It states every key, type and required key, the closed vocabularies,
//! the forms of strings as patterns, one variant of an input, a step and a
//! ROM, and per platform its media formats, ROM roles, lanes, boards,
//! machine classes, software platforms, `[ids]` catalogues and needs. A
//! TOML file is validated through the usual TOML-to-JSON mapping, in which
//! a local date-time is a string. What a schema cannot say, from the TOML
//! 1.1 forms to references, uniqueness, sums and the licence rules, only
//! `upcli recipe check` decides (recipes §4.6).

use crate::provider::Provider;
use crate::spec::{self, MapKey, Table, Text, Ty, Vocab};
use image_store::needs::Form;
use serde_json::{json, Map, Value};

/// The schema for `providers`, as pretty JSON with a final line break.
pub fn render(providers: &[&dyn Provider]) -> String {
    let mut text = serde_json::to_string_pretty(&schema(providers)).unwrap_or_default();
    text.push('\n');
    text
}

/// The schema for `providers`.
pub fn schema(providers: &[&dyn Provider]) -> Value {
    let mut defs = Map::new();
    for form in TEXTS {
        defs.insert(text_name(*form).to_owned(), text_schema(*form));
    }
    defs.insert("local-time".into(), local_time());
    let mut writer = Writer { defs };
    let roots: [(&str, &str, &'static Table); 2] = [
        ("mediafile", "media", &spec::MEDIAFILE),
        ("firmwarefile", "firmware", &spec::FIRMWAREFILE),
    ];
    for (name, identity, table) in roots {
        let mut dispatch = json!({ "$ref": format!("#/$defs/{}", writer.table(table, None)) });
        for provider in providers.iter().rev() {
            let then = writer.table(table, Some(*provider));
            dispatch = json!({
                "if": {
                    "properties": { identity: { "properties": { "platform": { "const": provider.platform() } }, "required": ["platform"] } },
                    "required": [identity]
                },
                "then": { "$ref": format!("#/$defs/{then}") },
                "else": dispatch
            });
        }
        writer.defs.insert(name.into(), dispatch);
    }
    let work = writer.table(&spec::WORK_FILE, None);
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "unemupc recipe 0.1: Mediafile, Firmwarefile or work file",
        "description": "Generated from the key tables of the recipe crate and the vocabularies of its platform providers (recipes.md section 6, experimental). A TOML file is validated through the usual TOML-to-JSON mapping, in which a local date-time is a string. What a schema cannot say (TOML 1.1 syntax, references, uniqueness, sums, the licence rules, sizes against formats and the firmware catalogue) is decided by upcli recipe check.",
        "type": "object",
        "if": { "properties": { "work": { "type": "object" } }, "required": ["work"] },
        "then": { "$ref": format!("#/$defs/{work}") },
        "else": {
            "if": { "required": ["firmware"] },
            "then": { "$ref": "#/$defs/firmwarefile" },
            "else": { "$ref": "#/$defs/mediafile" }
        },
        "$defs": Value::Object(writer.defs),
    })
}

struct Writer {
    defs: Map<String, Value>,
}

/// Whether a table holds a provider's vocabulary anywhere below it.
fn has_vocab(table: &Table) -> bool {
    fn ty(t: &Ty) -> bool {
        match t {
            Ty::Vocab(_) | Ty::Needs => true,
            Ty::Map {
                key: MapKey::Kind, ..
            } => true,
            Ty::Array { item, .. } => ty(item),
            Ty::Table(table) => has_vocab(table),
            Ty::OneOf(tables) => tables.iter().any(|t| has_vocab(t)),
            Ty::Map { value, .. } => ty(value),
            _ => false,
        }
    }
    table.fields.iter().any(|f| ty(&f.ty))
}

impl Writer {
    /// The definition of `table` for `provider` (`None`: any platform),
    /// written once; returns its name.
    fn table(&mut self, table: &'static Table, provider: Option<&dyn Provider>) -> String {
        let name = match (has_vocab(table), provider) {
            (false, _) => table.name.to_owned(),
            (true, Some(provider)) => format!("{}.{}", table.name, provider.platform()),
            (true, None) => format!("{}.any", table.name),
        };
        if self.defs.contains_key(&name) {
            return name;
        }
        self.defs.insert(name.clone(), Value::Null);
        let mut properties = Map::new();
        for field in table.fields {
            properties.insert(field.key.into(), self.ty(&field.ty, provider));
        }
        let required: Vec<&str> = table
            .fields
            .iter()
            .filter(|f| f.required)
            .map(|f| f.key)
            .collect();
        let mut schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": properties,
        });
        if !required.is_empty() {
            schema["required"] = json!(required);
        }
        self.defs.insert(name.clone(), schema);
        name
    }

    fn ty(&mut self, ty: &'static Ty, provider: Option<&dyn Provider>) -> Value {
        match ty {
            Ty::Str(form) => json!({ "$ref": format!("#/$defs/{}", text_name(*form)) }),
            Ty::Enum(names) => json!({ "enum": names }),
            Ty::Vocab(vocab) => match provider.and_then(|p| vocabulary(p, *vocab)) {
                Some(names) => json!({ "enum": names }),
                None => json!({ "type": "string", "minLength": 1 }),
            },
            Ty::Int { min, max } => {
                let mut schema = json!({ "type": "integer", "minimum": min });
                if *max != i64::MAX {
                    schema["maximum"] = json!(max);
                }
                schema
            }
            Ty::Bool => json!({ "type": "boolean" }),
            Ty::Time => json!({ "$ref": "#/$defs/local-time" }),
            Ty::Array { item, min } => {
                let mut schema = json!({ "type": "array", "items": self.ty(item, provider) });
                if *min > 0 {
                    schema["minItems"] = json!(min);
                }
                schema
            }
            Ty::Table(table) => {
                json!({ "$ref": format!("#/$defs/{}", self.table(table, provider)) })
            }
            Ty::OneOf(tables) => {
                let refs: Vec<Value> = tables
                    .iter()
                    .map(|t| json!({ "$ref": format!("#/$defs/{}", self.table(t, provider)) }))
                    .collect();
                json!({ "oneOf": refs })
            }
            Ty::Map { key, value } => self.map(*key, value, provider),
            Ty::Needs => need_values(&json!({ "type": "string", "pattern": NEED_VALUE }), false),
        }
    }

    fn map(&mut self, key: MapKey, value: &'static Ty, provider: Option<&dyn Provider>) -> Value {
        let names = match key {
            MapKey::Id => json!({ "$ref": "#/$defs/id" }),
            MapKey::Kind => json!({ "type": "string", "pattern": "^[a-z][a-z0-9_]{0,31}$" }),
        };
        if let (Ty::Needs, Some(provider)) = (value, provider) {
            let Some(vocabulary) = provider.needs() else {
                return json!({ "type": "object", "maxProperties": 0 });
            };
            let mut properties = Map::new();
            for kind in vocabulary.kinds() {
                let (one, single) = match kind.form {
                    Form::Ordered(scale) => {
                        let alternatives: Vec<String> = scale.iter().map(|s| escape(s)).collect();
                        (
                            json!({ "type": "string", "pattern": format!("^({})\\+?$", alternatives.join("|")) }),
                            false,
                        )
                    }
                    Form::Quantity => (
                        json!({ "type": "string", "pattern": "^[1-9][0-9]{0,6}[KM]$" }),
                        true,
                    ),
                    Form::Choice(names) => {
                        let mut names: Vec<&str> = names.to_vec();
                        if !names.contains(&image_store::needs::NONE) {
                            names.push(image_store::needs::NONE);
                        }
                        (json!({ "enum": names }), false)
                    }
                };
                properties.insert(kind.name.into(), need_values(&one, single));
            }
            return json!({ "type": "object", "additionalProperties": false, "properties": properties });
        }
        if let (Ty::Str(Text::Line), MapKey::Kind, Some(provider)) = (value, key, provider) {
            return json!({
                "type": "object",
                "propertyNames": { "enum": provider.work_id_catalogues() },
                "additionalProperties": { "$ref": format!("#/$defs/{}", text_name(Text::Line)) }
            });
        }
        let mut schema = json!({
            "type": "object",
            "propertyNames": names,
            "additionalProperties": self.ty(value, provider),
        });
        if key == MapKey::Id {
            schema["maxProperties"] = json!(crate::MAX_INPUTS);
        }
        schema
    }
}

const NEED_VALUE: &str = "^[A-Za-z0-9][A-Za-z0-9+._-]{0,31}$";

/// One value, or a non-empty array of distinct values (`single`: of one).
fn need_values(one: &Value, single: bool) -> Value {
    let mut array = json!({ "type": "array", "minItems": 1, "uniqueItems": true, "items": one });
    if single {
        array["maxItems"] = json!(1);
    }
    json!({ "oneOf": [one, array] })
}

fn escape(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let special = "\\^$.|?*+()[]{}".contains(c);
            special
                .then_some('\\')
                .into_iter()
                .chain(std::iter::once(c))
        })
        .collect()
}

fn vocabulary(provider: &dyn Provider, vocab: Vocab) -> Option<Vec<String>> {
    let names: Vec<&str> = match vocab {
        Vocab::MediaFormat => provider.media().formats().to_vec(),
        Vocab::VolumeFormat => {
            let media = provider.media();
            media
                .formats()
                .iter()
                .copied()
                .filter(|f| media.volume_profile(f).is_some())
                .collect()
        }
        Vocab::RomRole => provider.firmware().roles().to_vec(),
        Vocab::ChipLane => provider.firmware().lanes().to_vec(),
        Vocab::Board => provider.firmware().boards().to_vec(),
        Vocab::MachineClass => provider.firmware().machine_classes().to_vec(),
        Vocab::SoftwarePlatform => provider.software_platforms().to_vec(),
        // A name rule is code, not a list.
        Vocab::DosName => return None,
    };
    Some(names.into_iter().map(str::to_owned).collect())
}

const TEXTS: &[Text] = &[
    Text::Text,
    Text::Line,
    Text::Line128,
    Text::Id,
    Text::Ref,
    Text::Sha256,
    Text::Sha1,
    Text::Md5,
    Text::Crc32,
    Text::Digest,
    Text::Repository,
    Text::Platform,
    Text::Release,
    Text::Version,
    Text::Url,
    Text::FileName,
    Text::Path,
    Text::Date,
    Text::Spdx,
];

fn text_name(form: Text) -> &'static str {
    match form {
        Text::Text => "text",
        Text::Line => "line",
        Text::Line128 => "line128",
        Text::Id => "id",
        Text::Ref => "reference",
        Text::Sha256 => "sha256",
        Text::Sha1 => "sha1",
        Text::Md5 => "md5",
        Text::Crc32 => "crc32",
        Text::Digest => "digest",
        Text::Repository => "repository",
        Text::Platform => "platform",
        Text::Release => "release",
        Text::Version => "version",
        Text::Url => "url",
        Text::FileName => "file-name",
        Text::Path => "path",
        Text::Date => "date",
        Text::Spdx => "spdx",
    }
}

/// No control character but a line break, and something not blank.
const TEXT: &str = "^[^\\u0000-\\u0009\\u000b-\\u001f\\u007f]*[^\\s\\u0000-\\u001f\\u007f][^\\u0000-\\u0009\\u000b-\\u001f\\u007f]*$";
/// No control character, and something not blank.
const LINE: &str =
    "^[^\\u0000-\\u001f\\u007f]*[^\\s\\u0000-\\u001f\\u007f][^\\u0000-\\u001f\\u007f]*$";
const ID: &str = "[a-z0-9][a-z0-9._-]{0,63}";
const SEGMENT: &str = "[^/\\\\\\u0000-\\u001f\\u007f]+";
const DOT_PART: &str = "(^|/)\\.\\.?(/|$)";
const COMPONENT: &str = "[a-z0-9]+((\\.|_|__|-+)[a-z0-9]+)*";

fn text_schema(form: Text) -> Value {
    let description = form.expected().trim_start_matches("expected ");
    let mut schema = match form {
        Text::Text => json!({ "type": "string", "pattern": TEXT }),
        Text::Line | Text::Spdx => json!({ "type": "string", "pattern": LINE }),
        Text::Line128 => json!({ "type": "string", "pattern": LINE, "maxLength": 128 }),
        Text::Id => json!({ "type": "string", "pattern": format!("^{ID}$") }),
        Text::Ref => json!({
            "type": "string",
            "pattern": format!("^{ID}(/{SEGMENT})*$"),
            "not": { "pattern": "/\\.\\.?(/|$)" }
        }),
        Text::Sha256 => hex(64),
        Text::Sha1 => hex(40),
        Text::Md5 => hex(32),
        Text::Crc32 => hex(8),
        Text::Digest => json!({ "type": "string", "pattern": "^sha256:[0-9a-f]{64}$" }),
        Text::Repository => json!({
            "type": "string",
            "maxLength": 255,
            "pattern": format!("^{COMPONENT}(/{COMPONENT}){{3}}$")
        }),
        Text::Platform => json!({ "type": "string", "pattern": "^[a-z0-9][a-z0-9._-]{0,63}$" }),
        Text::Release => json!({
            "type": "string",
            "maxLength": spec::MAX_RELEASE,
            "pattern": "^[a-z0-9]+([._][a-z0-9]+)*(-[a-z0-9]+([._][a-z0-9]+)*)*$",
            "not": { "pattern": "(^|-)r[0-9]+(-|$)" }
        }),
        Text::Version => {
            json!({ "type": "string", "pattern": "^[0-9]{1,9}\\.[0-9]{1,9}\\.[0-9]{1,9}$" })
        }
        Text::Url => {
            json!({ "type": "string", "pattern": "^https://[^\\s\\u0000-\\u001f\\u007f]+$" })
        }
        Text::FileName => json!({
            "type": "string",
            "maxLength": 128,
            "pattern": LINE,
            "not": { "anyOf": [{ "pattern": "[/\\\\]" }, { "enum": [".", ".."] }] }
        }),
        Text::Path => json!({
            "type": "string",
            "pattern": format!("^{SEGMENT}(/{SEGMENT})*$"),
            "not": { "anyOf": [{ "pattern": DOT_PART }, { "pattern": "^[A-Za-z]:" }] }
        }),
        Text::Date => json!({
            "type": "string",
            "pattern": "^[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])$"
        }),
    };
    schema["description"] = json!(description);
    schema
}

fn hex(digits: usize) -> Value {
    json!({ "type": "string", "pattern": format!("^[0-9a-f]{{{digits}}}$") })
}

fn local_time() -> Value {
    json!({
        "description": "a local date-time with even seconds, years 1980 to 2107 (FAT and zip); TOML writes it unquoted, the JSON mapping makes it a string",
        "type": "string",
        "pattern": "^(198[0-9]|199[0-9]|20[0-9][0-9]|210[0-7])-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][02468]$"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reference_names_a_definition() {
        let schema = schema(&[]);
        let defs = schema["$defs"].as_object().unwrap();
        let text = schema.to_string();
        for part in text.split("\"#/$defs/").skip(1) {
            let name = &part[..part.find('"').unwrap()];
            assert!(defs.contains_key(name), "{name}");
        }
        assert!(defs.values().all(|d| !d.is_null()));
    }

    #[test]
    fn regex_special_characters_are_escaped() {
        assert_eq!(escape("a.b+c"), "a\\.b\\+c");
        assert_eq!(escape("8088"), "8088");
    }
}
