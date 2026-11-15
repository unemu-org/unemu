// SPDX-License-Identifier: AGPL-3.0-or-later
//! `image machine`: named local instances of images, mirroring the desktop's
//! machine actions; machine-images §3.2.
//!
//! Every command opens the store through [`pc_image::machine::open_store`],
//! which first makes older workspaces machines; adopted ids are reported on
//! the error stream as `migrated:ID`, and workspaces left as they are as
//! `migration-skipped:NAME\tREASON`. Output lines are tab-separated.

use pc_image::machine::{self, Listing};
use std::{ffi::OsString, io::Write, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    List,
    New {
        image: String,
        name: Option<String>,
    },
    Rename {
        id: String,
        name: String,
    },
    Duplicate {
        id: String,
        name: Option<String>,
    },
    Reset {
        id: String,
        yes: bool,
    },
    Remove {
        id: String,
        yes: bool,
    },
    Settings {
        id: String,
        /// Dotted paths and their value text.
        set: Vec<(String, String)>,
        clear: bool,
        dry_run: bool,
    },
    Commit {
        id: String,
        tag: String,
        title: Option<String>,
        keep_on_old_image: bool,
    },
    Run {
        id: String,
        max_steps: u64,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct MachineCommand {
    store: PathBuf,
    action: Action,
}

fn word(args: &[OsString], index: usize, missing: &'static str) -> Result<String, &'static str> {
    args.get(index)
        .and_then(|a| a.to_str())
        .filter(|a| !a.is_empty() && !a.starts_with("--"))
        .map(str::to_owned)
        .ok_or(missing)
}

/// `PATH=VALUE`: a dotted path into the hardware tables and a value.
fn setting(text: &str) -> Result<(String, String), &'static str> {
    let (path, value) = text
        .split_once('=')
        .filter(|(path, _)| {
            !path.is_empty()
                && path.split('.').all(|key| {
                    !key.is_empty()
                        && key
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                })
        })
        .ok_or("--set expects PATH=VALUE, such as memory.extended_kib=3072")?;
    Ok((path.into(), value.into()))
}

/// A TOML value; text that is not one is a string.
fn value(text: &str) -> toml::Value {
    format!("value = {text}")
        .parse::<toml::Table>()
        .ok()
        .and_then(|mut t| t.remove("value"))
        .unwrap_or_else(|| toml::Value::String(text.into()))
}

pub fn parse(args: &[OsString]) -> Result<MachineCommand, &'static str> {
    const EXPECTED: &str =
        "expected image machine ls, new, rename, duplicate, reset, rm, settings, commit or run";
    let verb = args.first().and_then(|a| a.to_str()).ok_or(EXPECTED)?;
    let (id, mut index) = match verb {
        "ls" => (String::new(), 1),
        "new" => (word(args, 1, "image machine new requires NAME:TAG")?, 2),
        "rename" | "duplicate" | "reset" | "rm" | "settings" | "commit" | "run" => (
            word(args, 1, "image machine commands require a machine ID")?,
            2,
        ),
        _ => return Err(EXPECTED),
    };
    let new_name = if verb == "rename" {
        index += 1;
        Some(word(args, 2, "image machine rename requires ID NAME")?)
    } else {
        None
    };
    let (mut store, mut name, mut tag, mut title, mut max_steps) = (None, None, None, None, None);
    let (mut yes, mut clear, mut dry_run, mut keep) = (false, false, false, false);
    let mut set = Vec::new();
    while index < args.len() {
        let option = args[index].to_str().ok_or("invalid machine option")?;
        index += 1;
        match option {
            "--yes" if matches!(verb, "reset" | "rm") && !yes => {
                yes = true;
                continue;
            }
            "--clear" if verb == "settings" && !clear => {
                clear = true;
                continue;
            }
            "--dry-run" if verb == "settings" && !dry_run => {
                dry_run = true;
                continue;
            }
            "--keep-on-old-image" if verb == "commit" && !keep => {
                keep = true;
                continue;
            }
            _ => (),
        }
        let value = args
            .get(index)
            .and_then(|a| a.to_str())
            .filter(|s| !s.is_empty() && !s.starts_with("--"))
            .ok_or("missing machine option value")?
            .to_owned();
        index += 1;
        match option {
            "--store" if store.is_none() => store = Some(PathBuf::from(value)),
            "--name" if matches!(verb, "new" | "duplicate") && name.is_none() => name = Some(value),
            "--tag" if verb == "commit" && tag.is_none() => tag = Some(value),
            "--title" if verb == "commit" && title.is_none() => title = Some(value),
            "--set" if verb == "settings" => set.push(setting(&value)?),
            "--max-steps" if verb == "run" && max_steps.is_none() => {
                max_steps = Some(
                    value
                        .parse()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or("invalid step limit")?,
                )
            }
            _ => return Err("unknown or duplicate machine option"),
        }
    }
    if clear && !set.is_empty() {
        return Err("--clear and --set exclude each other");
    }
    let action = match verb {
        "ls" => Action::List,
        "new" => Action::New { image: id, name },
        "rename" => Action::Rename {
            id,
            name: new_name.ok_or("image machine rename requires ID NAME")?,
        },
        "duplicate" => Action::Duplicate { id, name },
        "reset" => Action::Reset { id, yes },
        "rm" => Action::Remove { id, yes },
        "settings" => Action::Settings {
            id,
            set,
            clear,
            dry_run,
        },
        "commit" => Action::Commit {
            id,
            tag: tag.ok_or("image machine commit requires --tag NAME:TAG")?,
            title,
            keep_on_old_image: keep,
        },
        _ => Action::Run {
            id,
            max_steps: max_steps.unwrap_or(1_000_000),
        },
    };
    Ok(MachineCommand {
        store: store.ok_or("image commands require --store DIR")?,
        action,
    })
}

/// Host text for the library's last-run and creation times. The clock is read
/// here, in the frontend, never in the execution path.
pub fn host_time() -> Option<String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| image_store::machine::utc_text(d.as_secs()))
}

fn row(listing: &Listing) -> String {
    let m = &listing.machine;
    let mut line = format!(
        "{}\t{}\t{}\t{}\t{}\tlast-run:{}\tchanges:{}",
        listing.id,
        m.name,
        if m.source.is_empty() {
            "(no tag)"
        } else {
            &m.source
        },
        m.image.digest,
        listing.state,
        m.last_run.as_deref().unwrap_or("never"),
        listing.change_bytes
    );
    if listing.detached() {
        line.push_str("\tdetached");
    }
    if let Some(moved) = &listing.moved_to {
        line.push_str(&format!("\ttag-moved:{}", moved.digest));
    }
    for insert in &m.pending {
        line.push_str(&format!("\tinsert-pending:{}", insert.drive));
    }
    if listing.publication_pending {
        line.push_str("\tpublication-pending");
    }
    if let Some(problem) = &listing.problem {
        line.push_str(&format!("\tproblem:{problem}"));
    }
    line
}

fn set_path(table: &mut toml::Table, path: &str, value: toml::Value) -> anyhow::Result<()> {
    let mut keys = path.split('.').peekable();
    let mut table = table;
    while let Some(key) = keys.next() {
        if keys.peek().is_none() {
            table.insert(key.into(), value);
            return Ok(());
        }
        table = table
            .entry(key)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("{path}: {key} is not a table"))?;
    }
    Ok(())
}

pub fn execute(
    command: MachineCommand,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> anyhow::Result<()> {
    let (store, migration) = machine::open_store(&command.store)?;
    for id in migration.adopted {
        writeln!(errors, "migrated:{id}")?;
    }
    for (name, reason) in migration.skipped {
        writeln!(errors, "migration-skipped:{name}\t{reason}")?;
    }
    match command.action {
        Action::List => {
            for listing in machine::list(&store)? {
                writeln!(output, "{}", row(&listing))?;
            }
        }
        Action::New { image, name } => {
            let id = machine::create_from_tag(&store, &image, name.as_deref(), host_time())?;
            writeln!(output, "{id}\t{}", machine::get(&store, &id)?.machine.name)?;
        }
        Action::Rename { id, name } => machine::rename(&store, &id, &name)?,
        Action::Duplicate { id, name } => {
            let copy = machine::duplicate(&store, &id, name.as_deref(), host_time())?;
            writeln!(
                output,
                "{copy}\t{}",
                machine::get(&store, &copy)?.machine.name
            )?;
        }
        Action::Reset { id, yes } | Action::Remove { id, yes } if !yes => {
            let listing = machine::get(&store, &id)?;
            writeln!(output, "{}", row(&listing))?;
            anyhow::bail!("this drops the machine's changes; repeat with --yes");
        }
        Action::Reset { id, .. } => machine::reset(&store, &id)?,
        Action::Remove { id, .. } => machine::delete(&store, &id)?,
        Action::Settings {
            id,
            set,
            clear,
            dry_run,
        } => {
            let current = machine::settings(&store, &id)?;
            let (diff, cmos, note) = if clear && !dry_run {
                let change = machine::clear_settings(&store, &id)?;
                (change.diff, change.cmos_changes, change.cmos_note)
            } else if clear || !set.is_empty() {
                let mut table = if clear {
                    current.image
                } else {
                    current.machine
                };
                for (path, text) in set {
                    set_path(&mut table, &path, value(&text))?;
                }
                let change = machine::change_settings(&store, &id, table, !dry_run)?;
                (change.diff, change.cmos_changes, change.cmos_note)
            } else {
                (current.diff, Vec::new(), None)
            };
            for line in diff {
                match line {
                    pc_image::revision::DiffLine::Section(section) => {
                        writeln!(output, "{section}")?
                    }
                    pc_image::revision::DiffLine::Removed(text) => writeln!(output, "-{text}")?,
                    pc_image::revision::DiffLine::Added(text) => writeln!(output, "+{text}")?,
                }
            }
            for c in cmos {
                writeln!(
                    output,
                    "cmos:{:02X}h\told:{:02X}\tnew:{:02X}\t{}",
                    c.offset, c.old, c.new, c.field
                )?;
            }
            if let Some(note) = note {
                writeln!(output, "cmos-note:{note}")?;
            }
        }
        Action::Commit {
            id,
            tag,
            title,
            keep_on_old_image,
        } => {
            // Media inserted from installation sets are not part of the image;
            // moving onto the new image starts their journals afresh too.
            let inserted: Vec<String> = machine::get(&store, &id)?
                .changes
                .into_iter()
                .filter(|c| c.drive == machine::INSERTED)
                .map(|c| c.medium)
                .collect();
            if !inserted.is_empty() {
                writeln!(
                    errors,
                    "warning: writes to inserted media are not saved into the image{}: {}",
                    if keep_on_old_image {
                        ""
                    } else {
                        " and are dropped with the machine's journals"
                    },
                    inserted.join(", ")
                )?;
            }
            let image =
                machine::commit(&store, &id, &tag, None, title.as_deref(), keep_on_old_image)?;
            writeln!(output, "{tag}\t{}", image.digest)?;
            let now = machine::pinned_image(&store, &id)?;
            writeln!(output, "machine:{id}\tpinned-image:{}", now.digest)?;
        }
        Action::Run { id, max_steps } => {
            let owner = machine::own(&store, &id)?;
            run_owned(&owner, max_steps, "machine", output, errors)?;
        }
    }
    Ok(())
}

/// Start and run a machine in its owned workspace (from `machine::own`),
/// then finish its journals. The launch is confirmed (last run, consumed
/// inserts) only once the machine has assembled. The result line starts with
/// `label`, so `image run` keeps its output.
pub fn run_owned(
    owner: &image_store::workspace::Workspace<'_>,
    max_steps: u64,
    label: &str,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> anyhow::Result<()> {
    let prepared = machine::prepare(owner)?;
    writeln!(output, "pinned-image:{}", prepared.session.image)?;
    let mut assembled = assembly::assemble_with_verified_bases(
        &prepared.session.machine,
        &assembly::ObservationPolicy::default(),
        assembly::AssemblyOptions::default(),
        &prepared.session.bases,
    )?;
    machine::confirm(owner, &prepared.inserted, host_time())?;
    for insert in &prepared.inserted {
        writeln!(output, "inserted:{}\t{}", insert.drive, insert.medium)?;
    }
    let result = assembled.run_session(&assembly::RunPolicy::natural(
        Some(max_steps),
        Some(1_000_000),
    ));
    crate::finish_persistence(&mut assembled, errors)?;
    writeln!(output, "{label}:{}\t{:?}", owner.name(), result?)?;
    Ok(())
}
