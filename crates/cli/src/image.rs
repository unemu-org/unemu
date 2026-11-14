// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in local image commands; machine-images §§2–3 and §8.

mod media;

use std::{ffi::OsString, io::Write, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum ImageCommand {
    Run {
        store: PathBuf,
        tag: String,
        workspace: String,
        max_steps: u64,
        fresh: bool,
    },
    Workspaces {
        store: PathBuf,
    },
    Discard {
        store: PathBuf,
        workspace: String,
        yes: bool,
    },
    Abandon {
        store: PathBuf,
        workspace: String,
    },
    Publish {
        store: PathBuf,
        workspace: String,
        tag: String,
        flatten: bool,
    },
    Media(media::MediaCommand),
    Import {
        machine: PathBuf,
        store: PathBuf,
        tag: String,
        create: bool,
        options: pc_image::ImportOptions,
    },
    List {
        store: PathBuf,
    },
}
pub fn parse(args: &[OsString]) -> Result<ImageCommand, &'static str> {
    if args
        .first()
        .is_some_and(|s| matches!(s.to_str(), Some("run" | "workspace" | "publish")))
    {
        return parse_workspace(args);
    }
    if args.first().is_some_and(|a| a == "media") {
        return media::parse(&args[1..]).map(ImageCommand::Media);
    }
    let action = args
        .first()
        .and_then(|a| a.to_str())
        .ok_or("expected image import or ls")?;
    let (machine, start) = match action {
        "import" => (
            Some(PathBuf::from(
                args.get(1)
                    .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
                    .ok_or("image import requires a machine file")?,
            )),
            2,
        ),
        "ls" => (None, 1),
        _ => return Err("expected image import or ls"),
    };
    let (mut store, mut tag, mut create) = (None, None, false);
    let mut options = pc_image::ImportOptions::default();
    let mut args = args[start..].iter();
    while let Some(option) = args.next() {
        if option == "--create" {
            if action != "import" || create {
                return Err("--create requires image import and is accepted once");
            }
            create = true;
            continue;
        }
        let value = args
            .next()
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing image option value")?;
        match option.to_str() {
            Some("--store") if store.is_none() => store = Some(PathBuf::from(value)),
            Some("--tag") if action == "import" && tag.is_none() => {
                tag = Some(value.to_str().ok_or("invalid image tag")?.to_owned())
            }
            Some("--rom-dir") if action == "import" => options.rom_dirs.push(PathBuf::from(value)),
            Some("--recover") if action == "import" => {
                let (id, choice) = value
                    .to_str()
                    .and_then(|s| s.split_once('='))
                    .ok_or("recover expects MEDIUM=tip or MEDIUM=generation:N")?;
                if id.is_empty()
                    || id.len() > 32
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                {
                    return Err("invalid recovery medium id");
                }
                let choice = if choice == "tip" {
                    pc_image::RecoveryChoice::Recover
                } else {
                    let n = choice
                        .strip_prefix("generation:")
                        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                        .and_then(|s| s.parse().ok())
                        .ok_or("recover expects MEDIUM=tip or MEDIUM=generation:N")?;
                    pc_image::RecoveryChoice::Generation(n)
                };
                if options.recovery.insert(id.into(), choice).is_some() {
                    return Err("duplicate recovery medium");
                }
            }
            _ => return Err("unknown or duplicate image option"),
        }
    }
    let store = store.ok_or("image commands require --store DIR")?;
    match machine {
        Some(machine) => Ok(ImageCommand::Import {
            machine,
            store,
            tag: tag.ok_or("image import requires --tag NAME:TAG")?,
            create,
            options,
        }),
        None => Ok(ImageCommand::List { store }),
    }
}
pub fn execute(
    command: ImageCommand,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> anyhow::Result<u8> {
    match command {
        ImageCommand::Run {
            store,
            tag,
            workspace,
            max_steps,
            fresh,
        } => {
            let store = image_store::Store::open_existing(store)?;
            let owner = store.workspace(&workspace)?;
            let prepared = pc_image::workspace::prepare_start(&owner, &tag, fresh)?;
            writeln!(output, "pinned-image:{}", prepared.image)?;
            let mut machine = assembly::assemble_with_verified_bases(
                &prepared.machine,
                &assembly::ObservationPolicy::default(),
                assembly::AssemblyOptions::default(),
                &prepared.bases,
            )?;
            let result = machine.run_session(&assembly::RunPolicy::natural(
                Some(max_steps),
                Some(1_000_000),
            ));
            crate::finish_persistence(&mut machine, errors)?;
            writeln!(output, "workspace:{workspace}\t{:?}", result?)?;
        }
        ImageCommand::Workspaces { store } => {
            let store = image_store::Store::open_existing(store)?;
            for name in store.list_workspaces()? {
                writeln!(output, "{name}")?;
            }
        }
        ImageCommand::Discard {
            store,
            workspace,
            yes,
        } => {
            let store = image_store::Store::open_existing(store)?;
            let owner = store.open_workspace(&workspace)?;
            for path in owner.contents()? {
                writeln!(output, "affected:{}", path.display())?;
            }
            if !yes {
                anyhow::bail!("discard requires --yes after reviewing affected media");
            }
            owner.discard()?;
        }
        ImageCommand::Abandon { store, workspace } => {
            let store = image_store::Store::open_existing(store)?;
            let owner = store.open_workspace(&workspace)?;
            pc_image::workspace::abandon_publication(&owner)?;
        }
        ImageCommand::Publish {
            store,
            workspace,
            tag,
            flatten,
        } => {
            let store = image_store::Store::open_existing(store)?;
            let owner = store.open_workspace(&workspace)?;
            let manifest =
                pc_image::workspace::publish_with_options(&owner, &tag, None, flatten, |_| Ok(()))?;
            writeln!(output, "{tag}\t{}", manifest.digest)?;
        }
        ImageCommand::Media(command) => media::execute(command, output)?,
        ImageCommand::Import {
            machine,
            store,
            tag,
            create,
            options,
        } => {
            let store = if create {
                image_store::Store::open(store)?
            } else {
                image_store::Store::open_existing(store)?
            };
            let record = pc_image::import_machine(&store, &machine, &tag, &options)?;
            for warning in record.warnings {
                writeln!(errors, "warning: {warning}")?;
            }
            writeln!(output, "{tag}\t{}", record.manifest.digest)?;
            for g in record.generations {
                write!(output, "{}\tpolicy:{}", g.medium, g.policy.as_str())?;
                if let Some(generation) = g.generation {
                    write!(
                        output,
                        "\tgeneration:{generation}\trecovered:{}",
                        g.recovered
                    )?;
                }
                writeln!(output)?;
            }
        }
        ImageCommand::List { store } => {
            let store = image_store::Store::open_existing(store)?;
            let mut after = None;
            loop {
                let page = store.list_tags(after.as_deref(), 1000)?;
                for tag in page.tags {
                    use image_store::BlobSource;
                    let kind = store
                        .read_verified(&tag.manifest, image_store::MAX_SMALL_BLOB)
                        .and_then(|bytes| image_store::Manifest::decode_for_summary(&bytes))
                        .map(|m| {
                            if m.artifact_type() == image_store::media_type::MEDIA_SET {
                                "media-set"
                            } else {
                                "machine"
                            }
                        })
                        .unwrap_or("unknown");
                    writeln!(output, "{}\t{}\t{}", tag.name, tag.manifest.digest, kind)?;
                }
                after = page.next_after;
                if after.is_none() {
                    break;
                }
            }
        }
    }
    Ok(0)
}

fn parse_workspace(args: &[OsString]) -> Result<ImageCommand, &'static str> {
    let action = args[0].to_str().ok_or("invalid image action")?;
    let mut index = 1;
    let mut tag = None;
    let mut workspace = None;
    let mut sub = None;
    if action == "run" {
        tag = Some(
            args.get(index)
                .and_then(|s| s.to_str())
                .filter(|s| !s.starts_with("--"))
                .ok_or("image run requires NAME:TAG")?
                .to_owned(),
        );
        index += 1;
    } else if action == "workspace" {
        sub = Some(
            args.get(index)
                .and_then(|s| s.to_str())
                .ok_or("expected workspace ls, discard or abandon-publication")?,
        );
        index += 1;
        match sub {
            Some("ls") => (),
            Some("discard" | "abandon-publication") => {
                workspace = Some(
                    args.get(index)
                        .and_then(|s| s.to_str())
                        .ok_or("workspace discard requires NAME")?
                        .to_owned(),
                );
                index += 1;
            }
            _ => return Err("expected workspace ls, discard or abandon-publication"),
        }
    }
    let mut store = None;
    let mut max_steps = None;
    let (mut fresh, mut flatten, mut yes) = (false, false, false);
    while index < args.len() {
        let option = args[index].to_str().ok_or("invalid image option")?;
        index += 1;
        match option {
            "--new" if action == "run" && !fresh => {
                fresh = true;
                continue;
            }
            "--flatten" if action == "publish" && !flatten => {
                flatten = true;
                continue;
            }
            "--yes" if sub == Some("discard") && !yes => {
                yes = true;
                continue;
            }
            _ => (),
        }
        let value = args
            .get(index)
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing image option value")?;
        index += 1;
        match option {
            "--store" if store.is_none() => store = Some(PathBuf::from(value)),
            "--workspace" if action == "run" && workspace.is_none() => {
                workspace = Some(value.to_str().ok_or("invalid workspace")?.into())
            }
            "--from-workspace" if action == "publish" && workspace.is_none() => {
                workspace = Some(value.to_str().ok_or("invalid workspace")?.into())
            }
            "--tag" if action == "publish" && tag.is_none() => {
                tag = Some(value.to_str().ok_or("invalid tag")?.into())
            }
            "--max-steps" if action == "run" && max_steps.is_none() => {
                max_steps = Some(
                    value
                        .to_str()
                        .and_then(|s| s.parse().ok())
                        .filter(|n| *n > 0)
                        .ok_or("invalid step limit")?,
                )
            }
            _ => return Err("unknown or duplicate image option"),
        }
    }
    let store = store.ok_or("image commands require --store DIR")?;
    match (action, sub) {
        ("workspace", Some("ls")) => Ok(ImageCommand::Workspaces { store }),
        ("workspace", Some("discard")) => Ok(ImageCommand::Discard {
            store,
            workspace: workspace.ok_or("workspace required")?,
            yes,
        }),
        ("workspace", Some("abandon-publication")) => Ok(ImageCommand::Abandon {
            store,
            workspace: workspace.ok_or("workspace required")?,
        }),
        ("run", _) => Ok(ImageCommand::Run {
            store,
            tag: tag.ok_or("tag required")?,
            workspace: workspace.ok_or("--workspace required")?,
            max_steps: max_steps.unwrap_or(1_000_000),
            fresh,
        }),
        ("publish", _) => Ok(ImageCommand::Publish {
            store,
            tag: tag.ok_or("--tag required")?,
            workspace: workspace.ok_or("--from-workspace required")?,
            flatten,
        }),
        _ => Err("invalid image command"),
    }
}
