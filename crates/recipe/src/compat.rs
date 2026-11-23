// SPDX-License-Identifier: AGPL-3.0-or-later
//! Front-end compatibility rows of recipe versions and step kinds;
//! recipes §§7.1, 9 and machine-images §5.6. Parser features raise only
//! `unemu.min-frontend-version`. The locked output's core and packaging floors
//! remain independent. The platform provider rates the envelope platform spelling
//! in the core domain. Unknown versions and verbs still fail closed.

use crate::model::{Op, Recipe};
use image_store::compat::{lookup, AppVersion, Requirement, Row};

pub const NEXT_RELEASE: AppVersion = AppVersion::new(0, 0, 5);
pub const NEWEST_ROW: AppVersion = AppVersion::new(0, 0, 3);

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
    output_frontend: &Requirement,
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
    let output = output_frontend.release().map_err(|e| {
        format!(
            "the locked output's {}: {e}",
            image_store::compat::MIN_FRONTEND_VERSION
        )
    })?;
    Ok(release.max(output))
}

/// Compute parser and output requirements without comparing their release domains.
pub fn requirements(
    recipe: &Recipe,
    output: &image_store::compat::Requirements,
    table: &Table<'_>,
) -> Result<image_store::compat::Releases, String> {
    let frontend = required(recipe, &output.frontend, table)?;
    let core = output.core.release().map_err(|error| error.to_string())?;
    Ok(image_store::compat::Releases { core, frontend })
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
        assert_eq!(
            VERSIONS
                .iter()
                .chain(STEPS)
                .map(|(_, release)| *release)
                .max(),
            Some(NEWEST_ROW)
        );
        assert!(NEXT_RELEASE > NEWEST_ROW);
        for (_, release) in VERSIONS.iter().chain(STEPS) {
            assert!(*release <= image_store::compat::EXTRACTION_BASELINE);
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
    #[test]
    fn parser_features_raise_only_the_frontend_and_output_floors_keep_their_domain() {
        use image_store::compat::{Releases, Requirements};
        let newer = AppVersion::new(0, 0, 7);
        let versions: &[Row] = &[("1.0", newer)];
        let output = Requirements {
            core: Requirement::Release(AppVersion::new(0, 0, 5)),
            frontend: Requirement::None,
        };
        let rated = requirements(&recipe(""), &output, &Table { versions, ..TABLE }).unwrap();
        assert_eq!(
            rated,
            Releases {
                core: AppVersion::new(0, 0, 5),
                frontend: newer
            }
        );
        let output = Requirements {
            core: Requirement::None,
            frontend: Requirement::Release(AppVersion::new(0, 0, 8)),
        };
        let rated = requirements(&recipe(""), &output, &TABLE).unwrap();
        assert_eq!(
            rated,
            Releases {
                core: image_store::compat::BASELINE,
                frontend: AppVersion::new(0, 0, 8)
            }
        );
    }
}
