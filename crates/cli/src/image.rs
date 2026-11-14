// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in local image commands; machine-images §§2–3 and §8.

use std::{ffi::OsString, io::Write, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum ImageCommand {
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
                    writeln!(output, "{}\t{}", tag.name, tag.manifest.digest)?;
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
