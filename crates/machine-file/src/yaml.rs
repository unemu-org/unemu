// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hardened YAML syntax adapter; see the machine-files design § YAML: comparison.
//! The schema and hardware validation are shared with TOML.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

mod strict;

use serde::Deserialize;
use serde_saphyr::{Location, MessageFormatter};
use std::cell::RefCell;

use crate::{
    schema::v1::SchemaVersion, source_snippet, MachineFile, Origins, ParseError, YamlOrigins,
};

pub(crate) fn decode<'de, T: Deserialize<'de>>(
    file: &Path,
    input: &'de str,
) -> Result<T, ParseError> {
    // Machine descriptions are small. Bound both the input event stream and
    // alias replay so compact anchors cannot expand into unbounded work.
    let options = options();
    let source = inspect(file, input)?;
    if source.shapes.is_empty() {
        return Err(diagnostic_at(
            file,
            input,
            Some((1, 1)),
            "missing field `schema`".into(),
        ));
    }
    let expected = RefCell::new(None);
    serde_saphyr::with_deserializer_from_str_with_options(input, options, |d| {
        T::deserialize(strict::Strict {
            inner: d,
            source: &source,
            expected: &expected,
        })
    })
    .map_err(|error| from_error(file, input, error, &source, expected.into_inner()))
}

fn options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        strict_booleans: true,
        budget: serde_saphyr::budget! {
            max_documents: 1,
            max_depth: 32,
            max_events: 20_000,
            max_nodes: 10_000,
            max_total_scalar_bytes: 1_048_576,
            max_anchors: 32,
            max_aliases: 64,
            max_recorded_anchor_events: 2_000,
            max_recorded_anchor_bytes: 65_536,
            max_merge_keys: 32,
        },
        alias_limits: serde_saphyr::alias_limits! {
            max_total_replayed_events: 2_000,
            max_replay_stack_depth: 8,
            max_alias_expansions_per_anchor: 16,
        },
    }
}

type Position = (usize, usize);
#[derive(Default)]
struct Source {
    values: BTreeMap<Position, Position>,
    shapes: BTreeMap<Position, &'static str>,
    scalars: BTreeMap<Position, String>,
    containers: BTreeSet<Position>,
    anchors: BTreeMap<usize, String>,
    aliases: BTreeMap<Position, String>,
}

impl Source {
    fn value_position(&self, point: Position, unexpected: Option<&str>) -> Position {
        // A block map can start at its first key's position. Its own type
        // error already points at the value node, not that key's child value.
        if matches!(unexpected, Some("map" | "sequence"))
            && self.containers.contains(&point)
            && self.shapes.get(&point).copied() == unexpected
        {
            point
        } else {
            self.values.get(&point).copied().unwrap_or(point)
        }
    }
}

fn integer_spelling(text: &str) -> Option<&'static str> {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.len() > 1
        && digits.starts_with('0')
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
    {
        Some("unexpected leading zero, expected nothing")
    } else if digits.bytes().any(|b| b.is_ascii_digit())
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_')
        && text.replace('_', "").parse::<i64>().is_err()
    {
        Some("integer is out of range")
    } else {
        None
    }
}

fn from_error(
    file: &Path,
    input: &str,
    error: serde_saphyr::Error,
    source: &Source,
    expected: Option<String>,
) -> ParseError {
    use serde_saphyr::{budget::BudgetBreach, Error};
    let mut position = error.location().and_then(position);
    let mut inner = &error;
    while let Error::WithSnippet { error, .. } | Error::AliasError { error, .. } = inner {
        inner = error;
    }
    if matches!(
        inner,
        Error::SerdeInvalidType { .. } | Error::SerdeInvalidValue { .. }
    ) || matches!(inner, Error::Message { msg, .. } if msg == "integer is out of range" || msg == "unexpected leading zero, expected nothing")
    {
        let unexpected = match inner {
            Error::SerdeInvalidType { unexpected, .. } => Some(unexpected.as_str()),
            _ => None,
        };
        position = position.map(|key| source.value_position(key, unexpected));
    }
    let message = match inner {
        Error::Unexpected { .. } if expected.is_some() => {
            let shape = position
                .and_then(|point| source.shapes.get(&point))
                .copied()
                .unwrap_or("value");
            format!(
                "invalid type: {shape}, expected {}",
                expected.as_deref().unwrap_or("a value")
            )
        }
        Error::AliasExpansionLimitExceeded {
            anchor_id,
            max_expansions_per_anchor,
            ..
        } => {
            let anchor = source
                .anchors
                .get(anchor_id)
                .map_or_else(|| "anchor".into(), |name| format!("anchor `{name}`"));
            format!("{anchor}: alias expansion limit of {max_expansions_per_anchor} exceeded")
        }
        Error::AliasReplayLimitExceeded {
            max_total_replayed_events,
            ..
        } => {
            let alias = position
                .and_then(|point| source.aliases.get(&point))
                .map_or_else(|| "alias replay".into(), |name| format!("alias `*{name}`"));
            format!("{alias}: replay event limit of {max_total_replayed_events} exceeded")
        }
        Error::AliasReplayStackDepthExceeded { max_depth, .. } => {
            let alias = position
                .and_then(|point| source.aliases.get(&point))
                .map_or_else(|| "alias replay".into(), |name| format!("alias `*{name}`"));
            format!("{alias}: replay stack depth limit of {max_depth} exceeded")
        }
        Error::DuplicateMappingKey { key, .. } => key.as_ref().map_or_else(
            || "duplicate mapping key".into(),
            |key| format!("duplicate mapping key `{key}`"),
        ),
        Error::NullIntoString { .. } => "null is not allowed; omit the field instead".into(),
        Error::RecursiveReferencesRequireWeakTypes { .. } => {
            "recursive aliases are not allowed".into()
        }
        Error::MultipleDocuments { .. }
        | Error::Budget {
            breach: BudgetBreach::Documents { .. },
            ..
        } => "exactly one YAML document is required".into(),
        Error::Budget {
            breach: BudgetBreach::Anchors { .. },
            ..
        } => "YAML anchor budget exceeded".into(),
        Error::Budget {
            breach: BudgetBreach::Aliases { .. },
            ..
        } => "YAML alias budget exceeded".into(),
        Error::Budget { .. } => "YAML parser resource budget exceeded".into(),
        Error::Message { .. }
        | Error::SerdeInvalidType { .. }
        | Error::SerdeInvalidValue { .. }
        | Error::SerdeInvalidLength { .. }
        | Error::SerdeUnknownVariant { .. }
        | Error::SerdeUnknownField { .. }
        | Error::SerdeMissingField { .. }
        | Error::InvalidScalar { .. }
        | Error::InvalidBooleanStrict { .. }
        | Error::NonFiniteFloat { .. }
        | Error::MergeValueNotMapOrSeqOfMaps { .. }
        | Error::MergeKeyNotAllowed { .. } => serde_saphyr::DefaultMessageFormatter
            .format_message(inner)
            .to_string(),
        _ => "invalid YAML value".into(),
    };
    let message = if matches!(inner, Error::SerdeInvalidType { expected, .. } | Error::SerdeInvalidValue { expected, .. } if expected.contains("integer") || expected.starts_with('u') || expected.starts_with('i'))
    {
        position
            .and_then(|point| source.scalars.get(&point))
            .and_then(|text| integer_spelling(text))
            .map_or(message.clone(), str::to_owned)
    } else {
        message
    };
    diagnostic_at(file, input, position, message)
}

fn position(location: Location) -> Option<Position> {
    let line = usize::try_from(location.line()).ok()?;
    let column = usize::try_from(location.column()).ok()?;
    (line > 0 && column > 0).then_some((line, column))
}

fn inspect(file: &Path, input: &str) -> Result<Source, ParseError> {
    use serde_saphyr::granit_parser::{Event, Parser, ScalarStyle};
    let mut source = Source::default();
    let limits = options().budget.unwrap_or_default();
    let mut events = 0;
    let mut scalar_bytes = 0;
    let mut documents = 0;
    // A map frame retains its next key until the corresponding value starts.
    // Sequence frames have no keys. Container starts consume a parent slot.
    let mut frames: Vec<Option<Option<Position>>> = Vec::new();
    let mut parser_options = serde_saphyr::granit_parser::Options::default();
    parser_options.emit_anchor_events = true;
    for event in Parser::new_from_str_with_options(input, parser_options).keep_tags(true) {
        let (event, span) = event.map_err(|error| {
            diagnostic_at(
                file,
                input,
                Some((error.marker().line(), error.marker().col() + 1)),
                error.info().to_owned(),
            )
        })?;
        let point = (span.start.line(), span.start.col() + 1);
        // The raw pass must be bounded too: tags and nulls are rejected before
        // saphyr interprets them. Alias replay remains budgeted by saphyr.
        events += 1;
        if events > limits.max_events {
            return Err(diagnostic_at(
                file,
                input,
                Some(point),
                "YAML parser event budget exceeded".into(),
            ));
        }
        if matches!(event, Event::DocumentStart(..)) {
            documents += 1;
            if documents > limits.max_documents {
                return Err(diagnostic_at(
                    file,
                    input,
                    Some(point),
                    "exactly one YAML document is required".into(),
                ));
            }
        }
        if let Event::Scalar(value, ..) = &event {
            scalar_bytes += value.len();
            if scalar_bytes > limits.max_total_scalar_bytes {
                return Err(diagnostic_at(
                    file,
                    input,
                    Some(point),
                    "YAML scalar byte budget exceeded".into(),
                ));
            }
        }
        if event.tag().is_some() {
            let marker = span.tag_start.unwrap_or(span.start);
            return Err(diagnostic_at(
                file,
                input,
                Some((marker.line(), marker.col() + 1)),
                "YAML tags are not allowed".into(),
            ));
        }
        if let Event::Scalar(value, ScalarStyle::Plain, ..) = &event {
            if matches!(value.as_ref(), "" | "~" | "null" | "Null" | "NULL") {
                return Err(diagnostic_at(
                    file,
                    input,
                    Some(if span.start.index() == span.end.index() {
                        frames
                            .last()
                            .and_then(|frame| *frame)
                            .flatten()
                            .unwrap_or(point)
                    } else {
                        point
                    }),
                    "null is not allowed; omit the field instead".into(),
                ));
            }
            if matches!(value.as_ref(), "True" | "TRUE" | "False" | "FALSE") {
                return Err(diagnostic_at(
                    file,
                    input,
                    Some(point),
                    "booleans must be spelled true or false".into(),
                ));
            }
        }
        if let Event::Scalar(value, ScalarStyle::Plain, ..) = &event {
            let unsigned = value.strip_prefix(['+', '-']).unwrap_or(value);
            for (prefix, expected) in [("0X", "0x"), ("0O", "0o"), ("0B", "0b")] {
                if unsigned.starts_with(prefix) {
                    return Err(diagnostic_at(
                        file,
                        input,
                        Some(point),
                        format!("radix must be lowercase, expected `{expected}`"),
                    ));
                }
            }
            source.scalars.insert(point, value.to_string());
        }
        if event.is_node() {
            let shape = match &event {
                Event::MappingStart(..) => "map",
                Event::SequenceStart(..) => "sequence",
                _ => "scalar",
            };
            source.shapes.entry(point).or_insert(shape);
            if let Some(Some(key)) = frames.last_mut() {
                if let Some(key) = key.take() {
                    if matches!(event, Event::MappingStart(..) | Event::SequenceStart(..)) {
                        source.containers.insert(point);
                    }
                    source.values.insert(key, point);
                } else {
                    *key = Some(point);
                }
            }
        }
        match event {
            Event::Anchor(id, name) => {
                source.anchors.insert(id, name.to_string());
            }
            Event::Alias(id) => {
                if let Some(name) = source.anchors.get(&id) {
                    source.aliases.insert(point, name.clone());
                }
            }
            Event::MappingStart(..) => frames.push(Some(None)),
            Event::SequenceStart(..) => frames.push(None),
            Event::MappingEnd | Event::SequenceEnd => {
                frames.pop();
            }
            _ => {}
        }
        if frames.len() > limits.max_depth {
            return Err(diagnostic_at(
                file,
                input,
                Some(point),
                "YAML nesting depth budget exceeded".into(),
            ));
        }
    }
    Ok(source)
}

pub(crate) fn diagnostic(
    file: &Path,
    input: &str,
    location: Option<Location>,
    message: String,
) -> ParseError {
    diagnostic_at(file, input, location.and_then(position), message)
}

fn diagnostic_at(
    file: &Path,
    input: &str,
    position: Option<Position>,
    message: String,
) -> ParseError {
    let snippet = position.and_then(|(line, column)| {
        input
            .trim_start_matches('\u{feff}')
            .split('\n')
            .nth(line - 1)
            .map(|text| source_snippet(text, column - 1))
    });
    ParseError::new(file, position, message, snippet)
}

pub(crate) fn parse(file: &Path, input: &str) -> Result<(MachineFile, Origins), ParseError> {
    crate::check_size(file, input.len())?;
    #[derive(Deserialize)]
    struct SchemaPeek {
        schema: SchemaVersion,
    }
    let peek: SchemaPeek = decode(file, input)?;
    let _version = peek.schema;
    let document = decode(file, input)?;
    let origins = Origins::from_yaml(file, input)?;
    Ok((document, origins))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_stack_limit_uses_the_written_alias_and_limit() {
        // The parser flattens alias chains while recording them. A zero limit
        // exercises its stack guard with one real alias, without fabricating an error.
        let input = "schema: 1\nname: &word value\ndescription: *word\n";
        let mut limits = options();
        limits.alias_limits.max_replay_stack_depth = 0;
        let error = serde_saphyr::from_str_with_options::<serde::de::IgnoredAny>(input, limits)
            .unwrap_err();
        let source = inspect(Path::new("stack.yaml"), input).unwrap();
        let error = from_error(Path::new("stack.yaml"), input, error, &source, None);
        assert_eq!(
            error.message(),
            "alias `*word`: replay stack depth limit of 0 exceeded"
        );
        assert_eq!(error.line_column(), Some((3, 14)));
    }
}
