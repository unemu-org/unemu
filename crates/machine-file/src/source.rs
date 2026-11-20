// SPDX-License-Identifier: AGPL-3.0-or-later
//! Recursive source positions for layer values; see machine-files § extends.
//! The typed parser runs first, so this pass only records provenance.
//!
//! The walk and its TOML adapter are `pc-config`'s (`pc_config::source`);
//! this crate keeps the YAML parser, so it adds the YAML adapter.

use crate::{Origin, Origins, ParseError};
use pc_config::source::{children, LineIndex, Node, Shape};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
struct YamlNode(serde_saphyr::Spanned<Shape<YamlNode>>);

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
    if !yaml {
        return pc_config::source::record_toml(file, input, origins);
    }
    let lines = LineIndex::new(input);
    let tree: Shape<YamlNode> = crate::yaml::decode(file, input)?;
    children(tree, &lines, "", origins);
    Ok(())
}
