// SPDX-License-Identifier: AGPL-3.0-or-later
//! `upcli recipe check`: the static checks of recipes, which read the
//! recipe, its work file and whether its lock exists, and no input
//! (`backlog/docs/design/recipes.md` §6.11 and §6.15).

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

const USAGE: &str = "usage: upcli recipe check FILE|FOLDER...
  checks recipes and work files; a folder is checked with every *.media.toml, *.firmware.toml, *.machine.toml and *.work.toml below it
  exit 0: no findings; 1: findings; 2: usage or a file that cannot be read
";

/// The platforms whose recipes this build checks.
fn providers() -> [&'static dyn recipe::provider::Provider; 1] {
    [&pc_image::recipe::PcRecipes]
}

/// Run `upcli recipe ARGS...`; returns the exit code.
pub(crate) fn run(args: &[OsString], output: &mut impl Write, errors: &mut impl Write) -> u8 {
    let paths = match args {
        [command, paths @ ..]
            if command == "check"
                && !paths.is_empty()
                && paths
                    .iter()
                    .all(|p| !p.is_empty() && !p.as_encoded_bytes().starts_with(b"-")) =>
        {
            paths.iter().map(PathBuf::from).collect::<Vec<_>>()
        }
        _ => {
            let _ = errors.write_all(USAGE.as_bytes());
            return 2;
        }
    };
    let providers = providers();
    let report = match recipe::check_paths(&paths, recipe::Providers(&providers)) {
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
