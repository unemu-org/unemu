// SPDX-License-Identifier: AGPL-3.0-or-later
//! The PC's image commands; machine-images §§2–3, §5.1 and §8: `image
//! import` (a machine file to an image) and `image publish`
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
        if word(0, "publish") || (word(0, "workspace") && word(1, "abandon-publication")) {
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

/// `image publish` and `image workspace abandon-publication`,
/// whose words the neutral workspace commands share.
fn workspace(args: &[OsString]) -> Result<PcImageCommand, &'static str> {
    let words = parse_workspace(args)?;
    let store = words.store;
    match (words.action, words.sub) {
        ("workspace", Some("abandon-publication")) => Ok(PcImageCommand::Abandon {
            store,
            workspace: words.workspace.ok_or("workspace required")?,
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
    if let PcImageCommand::Import { options, .. } = &command {
        cli_core::image::check_min_app_version(
            options.min_app_version,
            host.compatibility.levels(pc_image::PLATFORM),
        )?;
    }
    let (output, errors) = (&mut io.output, &mut io.errors);
    match command {
        PcImageCommand::Abandon { store, workspace } => {
            let store = image_store::Store::open_existing(store)?
                .with_compatibility(host.compatibility.clone());
            let owner = store.open_workspace(&workspace)?;
            pc_image::workspace::abandon_publication(&owner)?;
        }
        PcImageCommand::Publish {
            store,
            workspace,
            tag,
            flatten,
        } => {
            let store = image_store::Store::open_existing(store)?
                .with_compatibility(host.compatibility.clone());
            // A machine's settings belong in its new image, and its record
            // follows the publication; only the machine command does both.
            refuse_machine(&store, &workspace, "image machine commit ID --tag NAME:TAG")?;
            let owner = store.open_workspace(&workspace)?;
            let manifest =
                pc_image::workspace::publish_with_options(&owner, &tag, None, flatten, |_| Ok(()))?;
            writeln!(output, "{tag}\t{}", manifest.digest)?;
        }
        PcImageCommand::Media(command) => {
            media::execute_with_compatibility(command, output, &host.compatibility)?
        }
        PcImageCommand::Machine(command) => {
            machine::execute(command, output, errors, &host.compatibility)?
        }
        PcImageCommand::Firmware(command) => firmware::execute(command, output, host)?,
        PcImageCommand::Template(command) => {
            template::execute_with_compatibility(command, output, &host.compatibility)?
        }
        PcImageCommand::Import {
            machine,
            store,
            tag,
            create,
            options,
        } => {
            let store = if create {
                image_store::Store::open(store)?.with_compatibility(host.compatibility.clone())
            } else {
                image_store::Store::open_existing(store)?
                    .with_compatibility(host.compatibility.clone())
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

/// Keep the typed PC for the persistence operation absent from the contract.
struct PcRun<T>(T);

impl<T: std::borrow::BorrowMut<unemu_pc_core::PcMachine>> cli_core::image::RunMachine for PcRun<T> {
    fn machine(&mut self) -> &mut dyn unemu_core_api::Machine {
        self.0.borrow_mut()
    }

    fn failed(
        &mut self,
        error: unemu_core_api::Error,
        errors: &mut dyn Write,
    ) -> (anyhow::Error, anyhow::Result<unemu_core_api::Finished>) {
        let machine = self.0.borrow_mut();
        let error = match machine.take_guest_failure() {
            Some(failure) => anyhow::Error::new(failure),
            None => anyhow::Error::new(error),
        };
        let persisted = pc_harness::finish_persistence(machine.assembled(), &mut &mut *errors)
            .map(|()| unemu_core_api::Finished::new())
            .map_err(anyhow::Error::from);
        (error, persisted)
    }
}

pub(crate) fn prepare_run<'a>(
    owner: &'a cli_core::image::OwnedRun<'_>,
    request: &cli_core::image::RunRequest<'_>,
    core: &'static dyn unemu_core_api::Core,
) -> anyhow::Result<cli_core::image::StoredRun<'a>> {
    let prepared = if owner.machine {
        pc_image::launch::prepare_machine(&owner.owner, core, owner.owner.name().to_owned())?
    } else {
        pc_image::launch::prepare_workspace(
            &owner.owner,
            request
                .tag
                .ok_or_else(|| anyhow::anyhow!(cli_core::image::NO_STORED_RUN))?,
            request.fresh,
            core,
            request.tag.unwrap_or_default().to_owned(),
        )?
    };
    let mut stored = cli_core::image::StoredRun::new(prepared.prepared, prepared.image);
    stored.start = |prepared| {
        use unemu_core_api::Core;
        if prepared.core().description().id != unemu_pc_core::PcCore.description().id {
            return Ok(Box::new(core_launch::start(prepared)?));
        }
        Ok(Box::new(PcRun(core_launch::start_with(
            prepared,
            |request| unemu_pc_core::PcCore.create_machine(request),
        )?)))
    };
    if owner.machine {
        stored.created = Box::new(move || {
            pc_image::machine::confirm(&owner.owner, &prepared.inserted, machine::host_time())?;
            Ok(prepared
                .inserted
                .iter()
                .map(|insert| format!("inserted:{}\t{}", insert.drive, insert.medium))
                .collect())
        });
    }
    Ok(stored)
}

#[cfg(test)]
fn run_started(
    machine: &mut unemu_pc_core::PcMachine,
    max_steps: u64,
    errors: &mut impl Write,
) -> anyhow::Result<unemu_core_api::Stop> {
    cli_core::image::run_started(&mut PcRun(machine), max_steps, errors)
}

#[cfg(test)]
use cli_core::image::write_finish_warnings;

#[cfg(test)]
mod tests {
    use super::run_started;
    use unemu_core_api::{Machine, Stop};
    use unemu_pc_core::PcMachine;

    fn start(config: &assembly::MachineConfig) -> PcMachine {
        let assembled = machine_file::assemble_with_observation_and_options(
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
        let temp = machine_file::test_support::TempDir::new("stored-run-refusal");
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
            expected_sha256: journal_files::VerifiedBase::from_bytes(&base)
                .unwrap()
                .identity()
                .base_digest,
            journal: journal.clone(),
            keep_generations: 3,
        };
        let mut direct = machine_file::assemble(&config).unwrap();
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
        let scan = journal_files::scan(&actual, &base, 64);
        assert_eq!(scan.image.unwrap()[0x10], 0x5a);
        assert_eq!(scan.report.last_closed_generation, Some(1));
        assert!(scan.report.unsealed_tip.is_none());
        assert_eq!(actual, expected);
    }

    #[test]
    fn the_run_hook_persists_a_failed_pc_and_keeps_its_diagnostic() {
        pc_image::test_support::with_synthetic_firmware(|| {
            let temp = machine_file::test_support::TempDir::new("run-hook-failure");
            let refused = assembly::test_support::refused_gate_call();
            let guest = refused
                .images
                .iter()
                .find_map(|image| match &image.source {
                    assembly::ImageSource::Inline(bytes) if image.base == 0x7c00 => Some(bytes),
                    _ => None,
                })
                .unwrap();
            let mut rom = vec![0xf4; 32768];
            let mut loader = vec![
                0xfa, 0xb8, 0x00, 0xf8, 0x8e, 0xd8, 0x31, 0xc0, 0x8e, 0xc0, 0xfc, 0xbe, 0x00, 0x01,
                0xbf, 0x00, 0x7c, 0xb9,
            ];
            loader.extend((guest.len() as u16).to_le_bytes());
            loader.extend([
                0xf3, 0xa4, 0x31, 0xc0, 0x8e, 0xd8, 0xb0, 0x10, 0xe6, 0x70, 0xb0, 0x5a, 0xe6, 0x71,
                0xea, 0x00, 0x7c, 0x00, 0x00,
            ]);
            rom[..loader.len()].copy_from_slice(&loader);
            rom[0x100..0x100 + guest.len()].copy_from_slice(guest);
            rom[0x7ff0..0x7ff5].copy_from_slice(&[0xea, 0x00, 0x00, 0x00, 0xf8]);
            std::fs::write(temp.path().join("bios.bin"), rom).unwrap();
            std::fs::write(temp.path().join("cmos.bin"), [0; 64]).unwrap();
            let file = temp.path().join("machine.toml");
            std::fs::write(&file, "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='journal'\njournal='source.upfj'\n[[card]]\nkind='vga-text'\n").unwrap();
            let store = image_store::Store::open(temp.path().join("store"))
                .unwrap()
                .with_compatibility(pc_image::compat::compatibility(
                    image_store::compat::EXTRACTION_BASELINE,
                ));
            pc_image::import_machine(
                &store,
                &file,
                "toy:failed",
                &pc_image::ImportOptions::default(),
            )
            .unwrap();
            let expected = {
                let owner = store.workspace("legacy").unwrap();
                let prepared =
                    pc_image::workspace::prepare_start(&owner, "toy:failed", false).unwrap();
                let mut machine = machine_file::assemble_with_verified_bases(
                    &prepared.machine,
                    &assembly::ObservationPolicy::default(),
                    assembly::AssemblyOptions {
                        allow_unlisted_firmware: true,
                    },
                    &prepared.bases,
                )
                .unwrap();
                let error = machine
                    .run_session(&assembly::RunPolicy::natural(
                        Some(100_000),
                        Some(1_000_000),
                    ))
                    .unwrap_err();
                pc_harness::finish_persistence(&mut machine, &mut Vec::new()).unwrap();
                format!("{:#}", anyhow::Error::new(error))
            };
            crate::with_platform(|pc| {
                let request = cli_core::image::RunRequest {
                    workspace: "hook",
                    tag: Some("toy:failed"),
                    fresh: false,
                };
                let owner = pc
                    .own_run(&store, &request, &mut Vec::new())
                    .unwrap()
                    .unwrap();
                let stored = pc
                    .prepare_run(&owner, &request, &unemu_pc_core::PcCore)
                    .unwrap();
                let prepared = stored
                    .prepared
                    .with_options(vec![("allow-unlisted-firmware".into(), "true".into())]);
                let mut machine = (stored.start)(&prepared).unwrap();
                (stored.created)().unwrap();
                let error =
                    cli_core::image::run_started(machine.as_mut(), 100_000, &mut Vec::new())
                        .unwrap_err();
                assert_eq!(format!("{error:#}"), expected);
            });
            let bytes = |name| {
                std::fs::read(store.workspace_path(name).unwrap().join("media/cmos.upfj")).unwrap()
            };
            let actual = bytes("hook");
            assert_eq!(actual, bytes("legacy"));
            let scan = journal_files::scan(&actual, &[0; 64], 64);
            assert_eq!(scan.image.unwrap()[0x10], 0x5a);
            assert_eq!(scan.report.last_closed_generation, Some(1));
            assert!(scan.report.unsealed_tip.is_none());
        });
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
