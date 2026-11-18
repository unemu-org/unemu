// SPDX-License-Identifier: AGPL-3.0-or-later
//! `upcli recipe check`: the static checks of recipes, which read the
//! recipe, its work file and whether its lock exists, and no input
//! (`backlog/docs/design/recipes.md` §6.11 and §6.15). `upcli recipe show`:
//! a media recipe's description merged with its work file (recipes §6.16),
//! printed as `upcli image media show` prints a set's title metadata. The
//! recipe providers are the platforms' ([`super::CliPlatform::recipes`]).

use super::{CliPlatform, Io};
use recipe::merge::{Merged, Origin, Sourced};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage: upcli recipe check FILE|FOLDER...
       upcli recipe show FILE
  check: checks recipes and work files; a folder is checked with every *.media.toml, *.firmware.toml, *.machine.toml and *.work.toml below it
  show: prints a media recipe's title record merged with its work file, with a comment on each value that came from the work file
  exit 0: no findings; 1: findings, which show prints instead of the record; 2: usage or a file that cannot be read
";

/// A parsed `upcli recipe` command line.
#[derive(Debug, PartialEq, Eq)]
pub enum RecipeCommand {
    Check(Vec<PathBuf>),
    Show(PathBuf),
    /// Words `recipe` does not take: its own usage text, exit 2.
    Usage,
}

/// Parse the words after `recipe`.
pub fn parse(args: &[OsString]) -> RecipeCommand {
    let operand = |p: &OsString| !p.is_empty() && !p.as_encoded_bytes().starts_with(b"-");
    match args {
        [command, paths @ ..]
            if command == "check" && !paths.is_empty() && paths.iter().all(operand) =>
        {
            RecipeCommand::Check(paths.iter().map(PathBuf::from).collect())
        }
        [command, path] if command == "show" && operand(path) => {
            RecipeCommand::Show(PathBuf::from(path))
        }
        _ => RecipeCommand::Usage,
    }
}

/// Run a parsed recipe command with every platform's recipe provider;
/// returns the exit code.
pub fn execute(command: RecipeCommand, io: &mut Io<'_>, platforms: &[&dyn CliPlatform]) -> u8 {
    let providers: Vec<&dyn recipe::provider::Provider> = platforms
        .iter()
        .filter_map(|platform| platform.recipes())
        .collect();
    let providers = recipe::Providers(&providers);
    match command {
        RecipeCommand::Check(paths) => check(&paths, providers, &mut io.output, &mut io.errors),
        RecipeCommand::Show(path) => show(&path, providers, &mut io.output, &mut io.errors),
        RecipeCommand::Usage => {
            let _ = io.errors.write_all(USAGE.as_bytes());
            2
        }
    }
}

fn check(
    paths: &[PathBuf],
    providers: recipe::Providers<'_>,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> u8 {
    let report = match recipe::check_paths(paths, providers) {
        Ok(report) => report,
        Err(error) => {
            let _ = writeln!(errors, "upcli recipe check: {error}");
            return 2;
        }
    };
    if output.write_all(report.render().as_bytes()).is_err() {
        return 2;
    }
    u8::from(!report.is_clean())
}

fn show(
    path: &Path,
    providers: recipe::Providers<'_>,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> u8 {
    if path.is_dir() {
        let _ = writeln!(
            errors,
            "upcli recipe show: {}: a folder; show takes one recipe file",
            path.display()
        );
        return 2;
    }
    let text = match recipe::merge_path(path, providers) {
        Ok(recipe::Shown::Merged(merged)) => record(&merged),
        // The check's own report: the findings, then the file's line.
        Ok(recipe::Shown::Refused(report)) => {
            return if output.write_all(report.render().as_bytes()).is_err() {
                2
            } else {
                1
            };
        }
        Ok(recipe::Shown::NotMedia(what)) => {
            let what = match what {
                recipe::What::Firmware => "a firmware recipe",
                recipe::What::Work => "a work file",
                recipe::What::Media => "a media recipe without a title record",
            };
            let _ = writeln!(
                errors,
                "upcli recipe show: {}: {what}; show prints the title record of a media recipe",
                path.display()
            );
            return 2;
        }
        Err(error) => {
            let _ = writeln!(errors, "upcli recipe show: {error}");
            return 2;
        }
    };
    if output.write_all(text.as_bytes()).is_err() {
        return 2;
    }
    0
}

/// The merged record, one line per value as `upcli image media show`
/// writes a set's catalogue fields: a field name, a tab and the value, text
/// quoted, in that command's names and order, with the title fields it
/// leaves out beside their neighbours. The first line names the tag, the
/// digest (`draft` without one) and the platform. A value that came from
/// the work file ends in a comment that names the file.
fn record(merged: &Merged) -> String {
    let work = merged.work_file.as_deref().unwrap_or_default();
    let mut lines: Vec<(String, Origin)> = Vec::new();
    let mut push = |text: String, from: Origin| lines.push((text, from));
    let recipe = Origin::Recipe;
    let one = |push: &mut dyn FnMut(String, Origin), key: &str, v: &Option<Sourced<String>>| {
        if let Some(v) = v {
            push(format!("{key}\t{:?}", v.value), v.from);
        }
    };
    let each =
        |push: &mut dyn FnMut(String, Origin), key: &str, v: &Option<Sourced<Vec<String>>>| {
            for value in v
                .iter()
                .flat_map(|v| v.value.iter().map(move |i| (i, v.from)))
            {
                push(format!("{key}\t{:?}", value.0), value.1);
            }
        };
    push(
        format!("title\t{:?}", merged.title.value),
        merged.title.from,
    );
    one(&mut push, "sort-title", &merged.sort_title);
    one(&mut push, "series", &merged.series);
    each(&mut push, "publisher", &merged.publishers);
    each(&mut push, "developer", &merged.developers);
    if let Some(year) = merged.year {
        push(format!("year\t{year}"), recipe);
    }
    each(&mut push, "genre", &merged.genres);
    one(&mut push, "platform", &merged.software_platform);
    if let Some(p) = &merged.players {
        push(format!("players\t{}\t{}", p.value.min, p.value.max), p.from);
    }
    for language in &merged.languages {
        push(format!("language\t{language:?}"), recipe);
    }
    for region in &merged.regions {
        push(format!("region\t{region:?}"), recipe);
    }
    one(&mut push, "description", &merged.description);
    for (key, value) in [
        ("version", &merged.version),
        ("edition", &merged.edition),
        ("source-media", &merged.source_media),
        ("requirements", &merged.requirements),
    ] {
        if let Some(value) = value {
            push(format!("{key}\t{value:?}"), recipe);
        }
    }
    for (kind, values) in &merged.needs {
        push(
            format!("need\t{kind}\t{}", values.value.join(",")),
            values.from,
        );
    }
    if let Some(id) = &merged.work {
        push(format!("work\t{id:?}"), recipe);
    }
    if let Some(synopsis) = &merged.synopsis {
        push(format!("synopsis\t{synopsis:?}"), Origin::Work);
    }
    for (catalogue, id) in &merged.ids {
        push(format!("work-id\t{catalogue}\t{:?}", id.value), id.from);
    }
    for label in &merged.labels {
        push(format!("set-label\t{:?}", label.value), label.from);
    }
    let mut out = format!(
        "{}:{}\t{}\t{}\n",
        merged.name,
        merged.tag(),
        merged.digest.as_deref().unwrap_or("draft"),
        image_store::platform::canonical(&merged.platform)
    );
    for (text, from) in lines {
        out.push_str(&text);
        if from == Origin::Work {
            out.push_str(&format!("\t# from {work}"));
        }
        out.push('\n');
    }
    out
}
