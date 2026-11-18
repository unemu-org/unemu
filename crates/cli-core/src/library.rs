// SPDX-License-Identifier: AGPL-3.0-or-later
//! `upcli library`: the folders a recipe's inputs are searched in, and their
//! index (`backlog/docs/design/recipes.md` §3.4 and §6.15).
//!
//! The libraries are a list in the shared preferences file (or `--config`),
//! the key `libraries`, absolute paths in the order they are searched; `add`
//! and `remove` change only that key, under the preferences lock every
//! writer takes. `scan` lists the files of each library, the directories of
//! its zip archives and the rows of its catalogues into the index cache in
//! the store's `cache/libraries/` ([`recipe::library`]); it hashes nothing,
//! as no recipe says which sizes are wanted. `ls` prints each library and
//! what its index holds.

use super::HostSources;
use anyhow::Context;
use image_registry::config;
use recipe::library::{self, Library};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The preferences key of the library list.
pub const KEY: &str = "libraries";

/// A parsed `upcli library` command.
#[derive(Debug, PartialEq, Eq)]
pub enum LibraryCommand {
    List {
        store: Option<PathBuf>,
        config: Option<PathBuf>,
    },
    Add {
        dir: PathBuf,
        config: Option<PathBuf>,
    },
    Remove {
        dir: PathBuf,
        config: Option<PathBuf>,
    },
    Scan {
        dirs: Vec<PathBuf>,
        store: Option<PathBuf>,
        config: Option<PathBuf>,
    },
}

/// Parse the words after `library`.
pub fn parse(args: &[OsString]) -> Result<LibraryCommand, &'static str> {
    let Some((command, rest)) = args.split_first() else {
        return Err("expected library ls, add, remove or scan");
    };
    let mut operands = Vec::new();
    let (mut store, mut config) = (None, None);
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        let slot = if word == "--store" {
            &mut store
        } else if word == "--config" {
            &mut config
        } else if word.as_encoded_bytes().starts_with(b"-") || word.is_empty() {
            return Err("unknown library option");
        } else {
            operands.push(PathBuf::from(word));
            continue;
        };
        let value = words.next().ok_or("an option without its value")?;
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err("an option given twice");
        }
    }
    match (command.to_str(), operands.as_slice()) {
        (Some("ls"), []) => Ok(LibraryCommand::List { store, config }),
        (Some("add"), [dir]) if store.is_none() => Ok(LibraryCommand::Add {
            dir: dir.clone(),
            config,
        }),
        (Some("remove"), [dir]) if store.is_none() => Ok(LibraryCommand::Remove {
            dir: dir.clone(),
            config,
        }),
        (Some("scan"), dirs) => Ok(LibraryCommand::Scan {
            dirs: dirs.to_vec(),
            store,
            config,
        }),
        (Some("add" | "remove"), _) => Err("expected library add|remove DIR [--config FILE]"),
        (Some("ls"), _) => Err("expected library ls [--store DIR] [--config FILE]"),
        _ => Err("expected library ls, add, remove or scan"),
    }
}

/// The preferences file: `--config`, or the shared one.
pub(crate) fn preferences(config: &Option<PathBuf>) -> anyhow::Result<PathBuf> {
    match config {
        Some(path) => Ok(path.clone()),
        None => Ok(image_store::location::preferences::path()?),
    }
}

/// The libraries the preferences list, in order.
pub(crate) fn configured(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let table = config::read_preferences(path)?;
    let Some(list) = table.get(KEY) else {
        return Ok(Vec::new());
    };
    let list = list
        .as_array()
        .with_context(|| format!("{}: `{KEY}` is not a list", path.display()))?;
    list.iter()
        .map(|item| {
            item.as_str()
                .map(PathBuf::from)
                .with_context(|| format!("{}: `{KEY}` holds something not a path", path.display()))
        })
        .collect()
}

/// The store root a command uses: `--store`, or the executable's default.
pub(crate) fn store_root(store: &Option<PathBuf>, host: &HostSources) -> Option<PathBuf> {
    store.clone().or_else(|| host.store_root.clone())
}

/// Run a parsed library command; returns the exit code.
pub fn execute(
    command: LibraryCommand,
    output: &mut dyn Write,
    errors: &mut dyn Write,
    host: &HostSources,
) -> anyhow::Result<u8> {
    match command {
        LibraryCommand::List { store, config } => {
            let root = store_root(&store, host);
            for dir in configured(&preferences(&config)?)? {
                let state = match (dir.canonicalize(), &root) {
                    (Err(_), _) => "missing".to_owned(),
                    (Ok(_), None) => "no store, no index".to_owned(),
                    (Ok(absolute), Some(root)) => {
                        match library::cache_summary(&library::cache_file(root, &absolute)) {
                            Some(s) => format!(
                                "{} files, {} zip archives ({} members), {} hashed",
                                s.files, s.archives, s.members, s.hashed
                            ),
                            None => "not scanned".to_owned(),
                        }
                    }
                };
                writeln!(output, "{}\t{state}", dir.display())?;
            }
            Ok(0)
        }
        LibraryCommand::Add { dir, config } => {
            let absolute = dir
                .canonicalize()
                .with_context(|| format!("{}", dir.display()))?;
            anyhow::ensure!(absolute.is_dir(), "{}: not a folder", dir.display());
            let path = preferences(&config)?;
            let text = absolute.to_string_lossy().into_owned();
            let mut outcome = None;
            config::update_preferences(&path, |table| {
                let list = table
                    .entry(KEY)
                    .or_insert_with(|| toml::Value::Array(Vec::new()));
                if let toml::Value::Array(items) = list {
                    let known = items.iter().any(|item| same(item.as_str(), &absolute));
                    if !known {
                        items.push(toml::Value::String(text.clone()));
                    }
                    outcome = Some(!known);
                }
                Ok(())
            })?;
            let added =
                outcome.with_context(|| format!("{}: `{KEY}` is not a list", path.display()))?;
            if added {
                writeln!(output, "added library {text}")?;
            } else {
                writeln!(output, "already a library: {text}")?;
            }
            Ok(0)
        }
        LibraryCommand::Remove { dir, config } => {
            let path = preferences(&config)?;
            let absolute = dir.canonicalize().ok();
            let mut removed = Vec::new();
            config::update_preferences(&path, |table| {
                if let Some(toml::Value::Array(items)) = table.get_mut(KEY) {
                    items.retain(|item| {
                        let hit = item.as_str().is_some_and(|text| {
                            Path::new(text) == dir
                                || absolute.as_deref().is_some_and(|a| same(Some(text), a))
                        });
                        if hit {
                            removed.push(item.as_str().unwrap_or_default().to_owned());
                        }
                        !hit
                    });
                }
                Ok(())
            })?;
            anyhow::ensure!(!removed.is_empty(), "not a library: {}", dir.display());
            for text in removed {
                writeln!(output, "removed library {text}")?;
            }
            Ok(0)
        }
        LibraryCommand::Scan {
            dirs,
            store,
            config,
        } => {
            // A library of the preferences may be on a disk not attached
            // now: a warning, and the others are scanned. A folder named on
            // the command line must be there.
            let from_preferences = dirs.is_empty();
            let dirs = if from_preferences {
                configured(&preferences(&config)?)?
            } else {
                dirs
            };
            let root = store_root(&store, host).context(
                "no store: give --store DIR; the index lives in the store's cache folder",
            )?;
            let store = image_store::Store::open(&root)?;
            for dir in dirs {
                let absolute = match dir.canonicalize() {
                    Ok(path) => path,
                    Err(error) if from_preferences => {
                        writeln!(
                            errors,
                            "upcli library scan: warning: library {} skipped: {error}",
                            dir.display()
                        )?;
                        continue;
                    }
                    Err(error) => anyhow::bail!("{}: {error}", dir.display()),
                };
                let cache = library::cache_file(store.root(), &absolute);
                let library = Library::scan(&dir, Some(&cache))?;
                library.save(&cache)?;
                let s = &library.stats;
                writeln!(
                    output,
                    "{}: {} files, {} zip archives ({} members), {} catalogue rows; {} hashes kept, {} stale entries dropped",
                    dir.display(),
                    s.files,
                    s.archives,
                    s.members,
                    s.catalogue_rows,
                    s.kept,
                    s.dropped
                )?;
                for (place, error) in &s.unreadable {
                    writeln!(output, "  unreadable: {place}: {error}")?;
                }
            }
            Ok(0)
        }
    }
}

/// A listed path names this folder, as written or once resolved.
fn same(item: Option<&str>, absolute: &Path) -> bool {
    item.is_some_and(|text| {
        Path::new(text) == absolute
            || Path::new(text)
                .canonicalize()
                .is_ok_and(|path| path == absolute)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<OsString> {
        text.split_whitespace().map(OsString::from).collect()
    }

    #[test]
    fn the_command_lines_parse_and_wrong_ones_are_refused() {
        assert_eq!(
            parse(&words("ls --store s --config c")),
            Ok(LibraryCommand::List {
                store: Some("s".into()),
                config: Some("c".into())
            })
        );
        assert_eq!(
            parse(&words("scan a b --store s")),
            Ok(LibraryCommand::Scan {
                dirs: vec!["a".into(), "b".into()],
                store: Some("s".into()),
                config: None
            })
        );
        assert!(matches!(
            parse(&words("add d --config c")),
            Ok(LibraryCommand::Add { .. })
        ));
        for refused in [
            "",
            "ls x",
            "add",
            "add a b",
            "add d --store s",
            "remove",
            "scan --store",
            "scan --json",
            "scan --store a --store b",
            "index",
        ] {
            assert!(parse(&words(refused)).is_err(), "{refused}");
        }
    }

    #[test]
    fn add_ls_scan_and_remove_keep_every_other_preference() {
        let temp = std::env::temp_dir().join(format!("cli-library-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let lib = temp.join("roms");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("a.bin"), b"bytes").unwrap();
        let prefs = temp.join("preferences.toml");
        std::fs::write(&prefs, "rom_dirs = [\"/roms\"]\n").unwrap();
        let config = Some(prefs.clone());
        let host = HostSources::default();
        let mut out = Vec::new();
        let run = |command: LibraryCommand, out: &mut Vec<u8>| {
            out.clear();
            let code = execute(command, out, &mut std::io::sink(), &host);
            (
                code.map_err(|e| e.to_string()),
                String::from_utf8(out.clone()).unwrap(),
            )
        };
        let absolute = lib.canonicalize().unwrap().display().to_string();
        let add = || LibraryCommand::Add {
            dir: lib.clone(),
            config: config.clone(),
        };
        assert_eq!(
            run(add(), &mut out),
            (Ok(0), format!("added library {absolute}\n"))
        );
        assert_eq!(
            run(add(), &mut out),
            (Ok(0), format!("already a library: {absolute}\n"))
        );
        let table = config::read_preferences(&prefs).unwrap();
        assert_eq!(table["rom_dirs"][0].as_str(), Some("/roms"));
        assert_eq!(table[KEY].as_array().unwrap().len(), 1);

        let store = Some(temp.join("store"));
        let list = || LibraryCommand::List {
            store: store.clone(),
            config: config.clone(),
        };
        assert_eq!(
            run(list(), &mut out),
            (Ok(0), format!("{absolute}\tnot scanned\n"))
        );
        let (code, text) = run(
            LibraryCommand::Scan {
                dirs: Vec::new(),
                store: store.clone(),
                config: config.clone(),
            },
            &mut out,
        );
        assert_eq!(code, Ok(0));
        assert_eq!(
            text,
            format!("{absolute}: 1 files, 0 zip archives (0 members), 0 catalogue rows; 0 hashes kept, 0 stale entries dropped\n")
        );
        assert_eq!(
            run(list(), &mut out),
            (
                Ok(0),
                format!("{absolute}\t1 files, 0 zip archives (0 members), 0 hashed\n")
            )
        );
        let remove = || LibraryCommand::Remove {
            dir: lib.clone(),
            config: config.clone(),
        };
        assert_eq!(
            run(remove(), &mut out),
            (Ok(0), format!("removed library {absolute}\n"))
        );
        let (code, _) = run(remove(), &mut out);
        assert_eq!(code, Err(format!("not a library: {}", lib.display())));
        assert_eq!(
            config::read_preferences(&prefs).unwrap()["rom_dirs"][0].as_str(),
            Some("/roms")
        );
        std::fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn scan_warns_of_a_missing_library_of_the_preferences_and_refuses_a_missing_named_one() {
        let temp = std::env::temp_dir().join(format!("cli-library-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let lib = temp.join("roms");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("a.bin"), b"bytes").unwrap();
        let gone = temp.join("unplugged");
        let prefs = temp.join("preferences.toml");
        std::fs::write(
            &prefs,
            format!(
                "libraries = [{:?}, {:?}]\n",
                gone.display().to_string(),
                lib.display().to_string()
            ),
        )
        .unwrap();
        let host = HostSources::default();
        let store = Some(temp.join("store"));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute(
            LibraryCommand::Scan {
                dirs: Vec::new(),
                store: store.clone(),
                config: Some(prefs.clone()),
            },
            &mut out,
            &mut err,
            &host,
        );
        assert_eq!(code.unwrap(), 0);
        let (out, err) = (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        );
        assert!(
            err.starts_with(&format!(
                "upcli library scan: warning: library {} skipped: ",
                gone.display()
            )),
            "{err}"
        );
        assert!(out.contains(": 1 files, "), "{out}");
        let named = execute(
            LibraryCommand::Scan {
                dirs: vec![gone.clone(), lib.clone()],
                store,
                config: Some(prefs),
            },
            &mut Vec::new(),
            &mut Vec::new(),
            &host,
        );
        assert!(named
            .unwrap_err()
            .to_string()
            .starts_with(&gone.display().to_string()));
        std::fs::remove_dir_all(&temp).unwrap();
    }
}
