// SPDX-License-Identifier: AGPL-3.0-or-later
//! Recursive source positions for layer values; see machine-files § extends.
//! The typed parser runs first, so this pass only records provenance.

use crate::{Origin, Origins, ParseError};
use serde::{
    de::{MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{collections::BTreeMap, fmt, path::Path};

enum Shape<N> {
    Leaf,
    Table(BTreeMap<String, N>),
    Array(Vec<N>),
}
impl<'de, N: Deserialize<'de>> Deserialize<'de> for Shape<N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ShapeVisitor<N>(std::marker::PhantomData<N>);
        impl<'de, N: Deserialize<'de>> Visitor<'de> for ShapeVisitor<N> {
            type Value = Shape<N>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a layer value")
            }
            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(Shape::Leaf)
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(Shape::Leaf)
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(Shape::Leaf)
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(Shape::Leaf)
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(Shape::Leaf)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                let mut entries = BTreeMap::new();
                while let Some((key, value)) = a.next_entry()? {
                    entries.insert(key, value);
                }
                Ok(Shape::Table(entries))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(value) = a.next_element()? {
                    entries.push(value);
                }
                Ok(Shape::Array(entries))
            }
        }
        d.deserialize_any(ShapeVisitor(std::marker::PhantomData))
    }
}

#[derive(Deserialize)]
struct TomlNode(toml::Spanned<Shape<TomlNode>>);
#[derive(Deserialize)]
struct YamlNode(serde_saphyr::Spanned<Shape<YamlNode>>);

pub(crate) struct LineIndex(Vec<usize>);
impl LineIndex {
    pub(crate) fn new(input: &str) -> Self {
        Self(
            std::iter::once(0)
                .chain(
                    input
                        .bytes()
                        .enumerate()
                        .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
                )
                .collect(),
        )
    }
    pub(crate) fn line(&self, offset: usize) -> usize {
        self.0.partition_point(|start| *start <= offset)
    }
}

trait Node: Sized {
    fn record(self, lines: &LineIndex, field: &str, origins: &mut Origins);
}
fn children<N: Node>(shape: Shape<N>, lines: &LineIndex, field: &str, origins: &mut Origins) {
    match shape {
        Shape::Table(entries) => {
            for (key, value) in entries {
                let path = if field.is_empty() {
                    key
                } else {
                    format!("{field}.{key}")
                };
                value.record(lines, &path, origins);
            }
        }
        Shape::Array(entries) => {
            for (index, value) in entries.into_iter().enumerate() {
                value.record(lines, &format!("{field}[{index}]"), origins);
            }
        }
        Shape::Leaf => {}
    }
}
impl Node for TomlNode {
    fn record(self, lines: &LineIndex, field: &str, origins: &mut Origins) {
        let start = self.0.span().start;
        if !matches!(self.0.get_ref(), Shape::Table(_)) || !origins.has_field(field) {
            origins.insert(
                field,
                Origin {
                    file: origins.get("").file.clone(),
                    line: Some(lines.line(start)),
                },
            );
        }
        children(self.0.into_inner(), lines, field, origins);
    }
}
impl Node for YamlNode {
    fn record(self, lines: &LineIndex, field: &str, origins: &mut Origins) {
        if !matches!(self.0.value, Shape::Table(_)) || !origins.has_field(field) {
            origins.insert(
                field,
                Origin {
                    file: origins.get("").file.clone(),
                    line: usize::try_from(self.0.referenced.line())
                        .ok()
                        .filter(|line| *line > 0),
                },
            );
        }
        children(self.0.value, lines, field, origins);
    }
}

pub(crate) fn record(
    file: &Path,
    input: &str,
    yaml: bool,
    origins: &mut Origins,
) -> Result<(), ParseError> {
    let lines = LineIndex::new(input);
    if yaml {
        let tree: Shape<YamlNode> = crate::yaml::decode(file, input)?;
        children(tree, &lines, "", origins);
    } else {
        let tree: Shape<TomlNode> =
            toml::from_str(input).map_err(|e| ParseError::from_toml(file, input, e))?;
        children(tree, &lines, "", origins);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_handles_line_starts_crlf_and_unicode() {
        let index = LineIndex::new("first\r\né界𝄞\nlast");
        for (offset, line) in [
            (0, 1),
            (5, 1),
            (6, 1),
            (7, 2),
            (15, 2),
            (16, 2),
            (17, 3),
            (21, 3),
        ] {
            assert_eq!(index.line(offset), line, "offset {offset}");
        }
    }
}
