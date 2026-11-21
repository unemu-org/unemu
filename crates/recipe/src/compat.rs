// SPDX-License-Identifier: AGPL-3.0-or-later
//! The compatibility rows of recipes (recipes §7.1, §9; machine-images
//! §5.6): the first
//! release that builds each recipe version and each step kind. A recipe
//! artifact's `unemu.min-app-version` is the oldest release that can
//! *build* the recipe, the maximum over its version's row, the row of every
//! step it has, the rows of the artifact's own media types
//! ([`image_store::compat::artifact`], [`image_store::compat::RECIPE_TYPES`])
//! and what its pinned output needs, which the output's manifest declares
//! in its lock: that marker already covers the provider's rows of the media
//! formats, firmware roles and catalogue entries the output uses, as the
//! importer computed them when the recipe was pinned.
//!
//! A reader checks the marker on the manifest before it fetches the
//! config or any part ([`image_store::compat::Requirement::read`]), so a
//! newer recipe is refused early with "Needs unemupc X or newer"; the
//! parser still refuses a version or a verb it does not know, the fail-closed
//! codec behind the hint.
//!
//! Every recipe version the parser accepts and every verb of
//! [`crate::spec::VERBS`] must have a row; [`step`] matches the model's
//! steps exhaustively, so a new step kind does not compile without one.

use crate::model::{Op, Recipe};
use image_store::compat::{lookup, AppVersion, Requirement, Row};

/// Recipe versions, the value of the top-level `recipe` key.
pub const VERSIONS: &[Row] = &[("1.0", AppVersion::new(0, 0, 3))];

/// Step kinds, by verb.
pub const STEPS: &[Row] = &[
    ("unpack", AppVersion::new(0, 0, 3)),
    ("fat", AppVersion::new(0, 0, 3)),
    ("text", AppVersion::new(0, 0, 3)),
];

/// The rows a release computes the marker from: this build's are
/// [`TABLE`]; a test stands for a newer release with raised rows.
#[derive(Clone, Copy, Debug)]
pub struct Table<'a> {
    pub versions: &'a [Row],
    pub steps: &'a [Row],
}

/// This build's rows.
pub const TABLE: Table<'static> = Table {
    versions: VERSIONS,
    steps: STEPS,
};

/// The verb of a step; `None` for a step whose verb could not be told,
/// which the checks refuse.
pub fn verb(op: &Op) -> Option<&'static str> {
    match op {
        Op::Unpack { .. } => Some("unpack"),
        Op::Fat { .. } => Some("fat"),
        Op::Text { .. } => Some("text"),
        Op::Unknown { .. } => None,
    }
}

/// The release that first builds a step of this kind, in `table`.
pub fn step(op: &Op, table: &Table<'_>) -> Result<AppVersion, String> {
    let verb = verb(op).ok_or("a step whose verb is not known has no compatibility row")?;
    lookup(&[table.steps], verb).ok_or_else(|| format!("no compatibility row for step kind {verb}"))
}

/// The oldest release that builds `recipe`, whose pinned output declares
/// `output` (its locked manifest's marker): the maximum over the rows of
/// `table` for its version and steps, the recipe artifact's own media
/// types and `output`. A version, verb or media type without a row is
/// refused, as no release is known to read it.
pub fn required(
    recipe: &Recipe,
    output: &Requirement,
    table: &Table<'_>,
) -> Result<AppVersion, String> {
    let version = recipe
        .version
        .as_ref()
        .map(|v| v.value.as_str())
        .ok_or("the recipe states no version")?;
    let mut release = lookup(&[table.versions], version)
        .ok_or_else(|| format!("no compatibility row for recipe version {version}"))?;
    for step in &recipe.steps {
        release = release.max(self::step(&step.op, table)?);
    }
    for kind in [
        crate::artifact::artifact_type(recipe.kind),
        image_store::media_type::RECIPE_CONFIG,
        image_store::media_type::RECIPE_WORK,
        image_store::media_type::RECIPE_INPUT,
        image_store::media_type::RECIPE_LOCK,
    ] {
        let row = image_store::compat::artifact(kind)
            .or_else(|| image_store::compat::payload(kind))
            .ok_or_else(|| format!("no compatibility row for media type {kind}"))?;
        release = release.max(row);
    }
    let output = output.release().map_err(|e| {
        format!(
            "the locked output's {}: {e}",
            image_store::compat::MIN_APP_VERSION
        )
    })?;
    Ok(release.max(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every version the parser accepts and every verb of the specification
    /// has a row; a row never names a release above this build's level.
    #[test]
    fn every_recipe_version_and_step_kind_has_a_row() {
        assert!(lookup(&[VERSIONS], crate::spec::VERSION).is_some());
        assert_eq!(VERSIONS.len(), 1, "one row per version the parser takes");
        for verb in crate::spec::VERBS {
            assert!(lookup(&[STEPS], verb).is_some(), "{verb}");
        }
        assert_eq!(STEPS.len(), crate::spec::VERBS.len());
        for (_, release) in VERSIONS.iter().chain(STEPS) {
            assert!(*release <= image_store::compat::level());
        }
    }

    fn recipe(steps: &str) -> Recipe {
        let text = format!(
            "recipe = \"1.0\"\n\n[media]\nname = \"private/media/ibm-pc/x\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n\
             [title]\ntitle = \"X\"\n\n\
             [input.disk]\nname = \"x.zip\"\nsize = 1\nsha256 = \"{}\"\n\n{steps}\
             [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"1.44M\"\nfrom = \"disk\"\n",
            "0".repeat(64)
        );
        match crate::parse_text(&text) {
            Some(crate::model::Document::Recipe(recipe)) => *recipe,
            _ => panic!("{text}"),
        }
    }

    /// Each row kind raises the marker: a newer release that rates the
    /// version, one step kind or the output (its media format) higher
    /// computes that release, and a verb without a row is refused.
    #[test]
    fn the_marker_is_the_newest_row_of_version_steps_and_output() {
        let next = AppVersion::new(0, 0, 3);
        let newer = AppVersion::new(0, 0, 4);
        let plain = recipe("");
        let unpacked = recipe("[[step]]\nid = \"files\"\nunpack = \"disk\"\nmembers = []\n\n");
        assert_eq!(required(&plain, &Requirement::None, &TABLE), Ok(next));
        assert_eq!(required(&unpacked, &Requirement::None, &TABLE), Ok(next));
        let versions: &[Row] = &[("1.0", newer)];
        let raised = Table { versions, ..TABLE };
        assert_eq!(required(&plain, &Requirement::None, &raised), Ok(newer));
        let steps: &[Row] = &[("unpack", newer), ("fat", next), ("text", next)];
        let raised = Table { steps, ..TABLE };
        assert_eq!(required(&plain, &Requirement::None, &raised), Ok(next));
        assert_eq!(required(&unpacked, &Requirement::None, &raised), Ok(newer));
        assert_eq!(
            required(&plain, &Requirement::Release(newer), &TABLE),
            Ok(newer)
        );
        let none: &[Row] = &[];
        let missing = Table {
            steps: none,
            ..TABLE
        };
        assert_eq!(
            required(&unpacked, &Requirement::None, &missing),
            Err("no compatibility row for step kind unpack".into())
        );
        assert!(required(&plain, &Requirement::Invalid("soon".into()), &TABLE).is_err());
    }
}
