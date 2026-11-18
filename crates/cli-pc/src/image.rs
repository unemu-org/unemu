// SPDX-License-Identifier: AGPL-3.0-or-later
//! The PC's image commands; machine-images §§2–3, §5.1 and §8: `image
//! import` (a machine file to an image), `image run` and `image publish`
//! (a workspace's session and its publication), `image workspace
//! abandon-publication`, and through the submodules `image media import`,
//! `image firmware import` and `edit`, `image machine` and `image template
//! build`.

mod firmware;
mod machine;
mod media;
mod template;

pub use template::build as build_templates;

use cli_core::{
    image::{min_app_version, parse_workspace, refuse_machine},
    HostSources, Io,
};
use std::{ffi::OsString, io::Write, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum PcImageCommand {
    Run {
        store: PathBuf,
        tag: String,
        workspace: String,
        max_steps: u64,
        fresh: bool,
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
    Machine(machine::MachineCommand),
    Firmware(firmware::FirmwareCommand),
    Template(template::TemplateCommand),
    Import {
        machine: PathBuf,
        store: PathBuf,
        tag: String,
        create: bool,
        options: pc_image::ImportOptions,
    },
}

/// Parse the words after `image`, or `None` when they are not the PC's.
pub fn parse(args: &[OsString]) -> Option<Result<PcImageCommand, &'static str>> {
    let word = |index: usize, text: &str| args.get(index).is_some_and(|a| a == text);
    Some(
        if word(0, "run")
            || word(0, "publish")
            || (word(0, "workspace") && word(1, "abandon-publication"))
        {
            workspace(args)
        } else if word(0, "firmware") && (word(1, "import") || word(1, "edit")) {
            firmware::parse(&args[1..]).map(PcImageCommand::Firmware)
        } else if word(0, "media") && word(1, "import") {
            media::parse(&args[1..]).map(PcImageCommand::Media)
        } else if word(0, "machine") {
            machine::parse(&args[1..]).map(PcImageCommand::Machine)
        } else if word(0, "template") {
            template::parse(&args[1..]).map(PcImageCommand::Template)
        } else if word(0, "import") {
            import(args)
        } else {
            return None;
        },
    )
}

/// `image run`, `image publish` and `image workspace abandon-publication`,
/// whose words the neutral workspace commands share.
fn workspace(args: &[OsString]) -> Result<PcImageCommand, &'static str> {
    let words = parse_workspace(args)?;
    let store = words.store;
    match (words.action, words.sub) {
        ("workspace", Some("abandon-publication")) => Ok(PcImageCommand::Abandon {
            store,
            workspace: words.workspace.ok_or("workspace required")?,
        }),
        ("run", _) => Ok(PcImageCommand::Run {
            store,
            tag: words.tag.ok_or("tag required")?,
            workspace: words.workspace.ok_or("--workspace required")?,
            max_steps: words.max_steps.unwrap_or(1_000_000),
            fresh: words.fresh,
        }),
        ("publish", _) => Ok(PcImageCommand::Publish {
            store,
            tag: words.tag.ok_or("--tag required")?,
            workspace: words.workspace.ok_or("--from-workspace required")?,
            flatten: words.flatten,
        }),
        _ => Err("invalid image command"),
    }
}

/// `image import FILE OPTIONS...`; `args` starts with `import`.
fn import(args: &[OsString]) -> Result<PcImageCommand, &'static str> {
    let machine = PathBuf::from(
        args.get(1)
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("image import requires a machine file")?,
    );
    let (mut store, mut tag, mut create) = (None, None, false);
    let mut options = pc_image::ImportOptions::default();
    let mut args = args[2..].iter();
    while let Some(option) = args.next() {
        if option == "--no-compress" && !options.no_compress {
            options.no_compress = true;
            continue;
        }
        if option == "--create" {
            if create {
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
            Some("--tag") if tag.is_none() => {
                tag = Some(value.to_str().ok_or("invalid image tag")?.to_owned())
            }
            Some("--rom-dir") => options.rom_dirs.push(PathBuf::from(value)),
            Some("--min-app-version") if options.min_app_version.is_none() => {
                options.min_app_version = Some(min_app_version(value)?)
            }
            Some("--recover") => {
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
    Ok(PcImageCommand::Import {
        machine,
        store: store.ok_or("image commands require --store DIR")?,
        tag: tag.ok_or("image import requires --tag NAME:TAG")?,
        create,
        options,
    })
}

pub fn execute(command: PcImageCommand, io: &mut Io<'_>, host: &HostSources) -> anyhow::Result<u8> {
    let (output, errors) = (&mut io.output, &mut io.errors);
    match command {
        PcImageCommand::Run {
            store,
            tag,
            workspace,
            max_steps,
            fresh,
        } => {
            let store = image_store::Store::open_existing(store)?;
            if store.read_machines()?.get(&workspace).is_some() {
                // A machine's workspace starts as the machine does: with its
                // settings, pending inserts and last run.
                if fresh {
                    anyhow::bail!("--new requires a fresh workspace name");
                }
                let pinned = pc_image::machine::pinned_image(&store, &workspace)?;
                match store.tag_descriptor(&tag) {
                    Ok(requested) if requested.digest != pinned.digest => anyhow::bail!(
                        "workspace pins {}; requested tag points to {}",
                        pinned.digest,
                        requested.digest
                    ),
                    Ok(_) | Err(image_store::Error::TagNotFound(_)) => (),
                    Err(error) => return Err(error.into()),
                }
                let owner = pc_image::machine::own(&store, &workspace)?;
                machine::run_owned(&owner, max_steps, "workspace", output, errors)?;
                return Ok(0);
            }
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
            pc_harness::finish_persistence(&mut machine, errors)?;
            writeln!(output, "workspace:{workspace}\t{:?}", result?)?;
        }
        PcImageCommand::Abandon { store, workspace } => {
            let store = image_store::Store::open_existing(store)?;
            let owner = store.open_workspace(&workspace)?;
            pc_image::workspace::abandon_publication(&owner)?;
        }
        PcImageCommand::Publish {
            store,
            workspace,
            tag,
            flatten,
        } => {
            let store = image_store::Store::open_existing(store)?;
            // A machine's settings belong in its new image, and its record
            // follows the publication; only the machine command does both.
            refuse_machine(&store, &workspace, "image machine commit ID --tag NAME:TAG")?;
            let owner = store.open_workspace(&workspace)?;
            let manifest =
                pc_image::workspace::publish_with_options(&owner, &tag, None, flatten, |_| Ok(()))?;
            writeln!(output, "{tag}\t{}", manifest.digest)?;
        }
        PcImageCommand::Media(command) => media::execute(command, output)?,
        PcImageCommand::Machine(command) => machine::execute(command, output, errors)?,
        PcImageCommand::Firmware(command) => firmware::execute(command, output, host)?,
        PcImageCommand::Template(command) => template::execute(command, output)?,
        PcImageCommand::Import {
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
    }
    Ok(0)
}
