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
            let prepared = pc_image::launch::prepare_workspace(
                &owner,
                &tag,
                fresh,
                &unemu_pc_core::PcCore,
                tag.clone(),
            )?;
            writeln!(output, "pinned-image:{}", prepared.image)?;
            let mut machine = core_launch::start_with(&prepared.prepared, |request| {
                unemu_pc_core::PcCore.create_machine(request)
            })?;
            let stop = run_started(&mut machine, max_steps, errors)?;
            writeln!(output, "workspace:{workspace}\t{stop:?}")?;
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

/// Preserve stored-image persistence on errors as well as successful runs.
fn run_started(
    machine: &mut unemu_pc_core::PcMachine,
    max_steps: u64,
    errors: &mut impl Write,
) -> anyhow::Result<unemu_core_api::Stop> {
    use unemu_core_api::Machine;

    let result = machine
        .run(unemu_core_api::RunLimit::steps(max_steps))
        .map_err(|error| match machine.take_guest_failure() {
            Some(failure) => anyhow::Error::new(failure),
            None => anyhow::Error::new(error),
        });
    // The contract forbids finish after failure, but these PC commands have
    // always saved CMOS and closed journals before returning a run error.
    let finished = if result.is_err() {
        pc_harness::finish_persistence(machine.assembled(), errors)
            .map(|()| unemu_core_api::Finished::new())
            .map_err(anyhow::Error::from)
    } else {
        machine.finish().map_err(anyhow::Error::from)
    };
    let mut messages = Vec::new();
    machine.drain_messages(&mut messages);
    for message in messages {
        let written = writeln!(errors, "{}", message.text);
        if message.level != unemu_core_api::MessageLevel::Warning {
            written?;
        }
    }
    if let Ok(finished) = &finished {
        write_finish_warnings(finished, errors);
    }
    finished?;
    result
}

fn write_finish_warnings(finished: &unemu_core_api::Finished, errors: &mut impl Write) {
    for warning in &finished.warnings {
        let _ = writeln!(errors, "warning: {warning}");
    }
}

#[cfg(test)]
mod tests {
    use super::run_started;
    use unemu_core_api::{Machine, Stop};
    use unemu_pc_core::PcMachine;

    fn start(config: &assembly::MachineConfig) -> PcMachine {
        let assembled = assembly::assemble_with_observation_and_options(
            config,
            &assembly::ObservationPolicy::default(),
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        PcMachine::from_assembled(config, assembled).unwrap()
    }

    #[test]
    fn a_stored_run_continues_a_guest_reset_and_drains_its_message() {
        let mut rom = vec![0; 0x8000];
        rom[..5].copy_from_slice(&[0xb0, 0xfe, 0xe6, 0x64, 0xf4]);
        rom[0x7ff0..0x7ff2].copy_from_slice(&[0xfa, 0xf4]);
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xf800, ip: 0 };
        let mut machine = start(&config);
        let mut errors = Vec::new();
        assert_eq!(
            run_started(&mut machine, 100, &mut errors).unwrap(),
            Stop::Halted
        );
        assert_eq!(machine.cpu_resets(), 1);
        assert_eq!(errors, b"cpu reset\n");
    }

    #[test]
    fn a_stored_run_has_no_idle_cutoff_and_uses_the_requested_step_limit() {
        let config = assembly::at_keyboard(vec![0xfb, 0xf4, 0xeb, 0xfd]);
        let mut machine = start(&config);
        let mut errors = Vec::new();
        assert_eq!(
            run_started(&mut machine, 2_000_000, &mut errors).unwrap(),
            Stop::Steps
        );
        assert_eq!(machine.steps(), 2_000_000);
        assert!(errors.is_empty());
    }

    #[test]
    fn a_stored_run_persists_cmos_and_seals_its_generation_after_a_cpu_refusal() {
        let temp = assembly::test_support::TempDir::new("stored-run-refusal");
        let base = [0; 64];
        let image = temp.path().join("cmos.bin");
        let journal = temp.path().join("cmos.upfj");
        std::fs::write(&image, base).unwrap();
        let mut config = assembly::test_support::refused_gate_call();
        config.images.push(assembly::LoadImage {
            base: 0x7000,
            source: assembly::ImageSource::Inline(vec![
                0xfa, 0xb0, 0x10, 0xe6, 0x70, 0xb0, 0x5a, 0xe6, 0x71, 0xea, 0x00, 0x7c, 0, 0,
            ]),
            kind: assembly::ImageKind::RamLoad,
            provides: vec![],
        });
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0, ip: 0x7000 };
        let rtc = config
            .devices
            .iter_mut()
            .find_map(|device| {
                if let assembly::DeviceKind::Rtc(rtc) = &mut device.kind {
                    Some(rtc)
                } else {
                    None
                }
            })
            .unwrap();
        rtc.nvram = assembly::NvramSource::Journal {
            image,
            expected_sha256: media::VerifiedBase::from_bytes(&base)
                .unwrap()
                .identity()
                .base_digest,
            journal: journal.clone(),
            keep_generations: 3,
        };
        let mut direct = assembly::assemble(&config).unwrap();
        let old_error = direct
            .run_session(&assembly::RunPolicy::natural(
                Some(100_000),
                Some(1_000_000),
            ))
            .unwrap_err();
        direct.finish_persistence().unwrap();
        drop(direct);
        let expected = std::fs::read(&journal).unwrap();
        std::fs::remove_file(&journal).unwrap();

        let mut machine = start(&config);
        let error = run_started(&mut machine, 100_000, &mut Vec::new()).unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            format!("{:#}", anyhow::Error::new(old_error))
        );
        assert_eq!(machine.status(), unemu_core_api::Status::Failed);
        assert_eq!(
            machine.finish().unwrap_err().kind,
            unemu_core_api::ErrorKind::Usage
        );
        drop(machine);
        let actual = std::fs::read(&journal).unwrap();
        let scan = media::scan(&actual, &base, 64);
        assert_eq!(scan.image.unwrap()[0x10], 0x5a);
        assert_eq!(scan.report.last_closed_generation, Some(1));
        assert!(scan.report.unsealed_tip.is_none());
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_stored_run_returns_the_guest_error_after_persistence() {
        let config = assembly::test_support::refused_gate_call();
        let mut machine = start(&config);
        let mut errors = Vec::new();
        let error = run_started(&mut machine, 100_000, &mut errors).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<machine::MachineError>(),
            Some(machine::MachineError::Cpu(_))
        ));
    }

    #[test]
    fn a_finish_warning_ignores_output_failures() {
        struct RefusedOutput;
        impl std::io::Write for RefusedOutput {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let finished = unemu_core_api::Finished::new().with_warnings(vec!["maintenance".into()]);
        let mut output = Vec::new();
        super::write_finish_warnings(&finished, &mut output);
        assert_eq!(output, b"warning: maintenance\n");
        super::write_finish_warnings(&finished, &mut RefusedOutput);
    }
}
