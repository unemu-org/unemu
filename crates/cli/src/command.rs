// SPDX-License-Identifier: AGPL-3.0-or-later
//! Frontend argument parsing, machine-file runs and offline media exit policy;
//! `backlog/docs/design/machine-files.md` § Frontends,
//! `backlog/docs/design/persistent-media.md` § Offline commands and
//! `backlog/docs/design/firmware-and-machine-classes.md` §6.

use anyhow::Context;
use media::offline::{Access, JournalTool, ToolError};
use media::{ClosingReason, Status, VerificationReport};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;

const USAGE: &str = "usage: unemupc98 --boot (Real16 timer demo)
       unemupc98 --bios PATH [--allow-unlisted-firmware]
       unemupc98 machine check FILE [--allow-unlisted-firmware]
       unemupc98 run --machine FILE [--max-steps N] [--idle-limit N] [--print CARD] [--allow-unlisted-firmware]
       unemupc98 media generations --base IMG --journal J
       unemupc98 media verify --base IMG --journal J [--repair-tail --backup PATH]
       unemupc98 media rollback --base IMG --journal J --to N --backup PATH
       unemupc98 media commit (alias compact) --base IMG --journal J --to N --output OUT
DOS boot and interactive sessions: see crates/cli/tests/*dos*golden.rs\n";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Usage,
    Boot,
    Bios {
        path: PathBuf,
        options: assembly::AssemblyOptions,
    },
    Check {
        path: PathBuf,
        options: assembly::AssemblyOptions,
    },
    Run {
        path: PathBuf,
        max_steps: u64,
        idle_limit: u64,
        print: Option<String>,
        options: assembly::AssemblyOptions,
    },
    Media(MediaCommand),
}

#[derive(Debug, PartialEq, Eq)]
pub struct MediaCommand {
    pub base: PathBuf,
    pub journal: PathBuf,
    pub action: Action,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Generations,
    Verify,
    Repair { backup: PathBuf },
    Rollback { generation: u64, backup: PathBuf },
    Commit { generation: u64, output: PathBuf },
}

pub fn parse(args: &[OsString]) -> Result<Command, &'static str> {
    if args.is_empty() {
        return Ok(Command::Usage);
    }
    let mut options = assembly::AssemblyOptions::default();
    let frontend: Vec<_> = args
        .iter()
        .filter(|arg| {
            if *arg == "--allow-unlisted-firmware" {
                options.allow_unlisted_firmware = true;
                false
            } else {
                true
            }
        })
        .collect();
    if frontend.len() == 1 && frontend[0] == "--boot" {
        return Ok(Command::Boot);
    }
    if frontend.len() == 2 && frontend[0] == "--bios" {
        return Ok(Command::Bios {
            path: frontend[1].into(),
            options,
        });
    }
    if frontend.first().is_some_and(|arg| *arg == "machine") {
        return if frontend.len() == 3
            && frontend[1] == "check"
            && !frontend[2].is_empty()
            && !frontend[2].as_encoded_bytes().starts_with(b"--")
        {
            Ok(Command::Check {
                path: frontend[2].into(),
                options,
            })
        } else {
            Err("expected machine check FILE")
        };
    }
    if frontend.first().is_some_and(|arg| *arg == "run") {
        let (mut path, mut max_steps, mut idle_limit, mut print) = (None, None, None, None);
        let mut args = frontend[1..].iter();
        while let Some(arg) = args.next() {
            let value = args
                .next()
                .filter(|value| !value.is_empty() && !value.as_encoded_bytes().starts_with(b"--"))
                .ok_or("missing option value")?;
            match arg.to_str() {
                Some("--machine") if path.is_none() => path = Some(PathBuf::from(value)),
                Some("--max-steps") if max_steps.is_none() => max_steps = Some(decimal(value)?),
                Some("--idle-limit") if idle_limit.is_none() => idle_limit = Some(decimal(value)?),
                Some("--print") if print.is_none() => {
                    print = Some(value.to_str().ok_or("invalid card id")?.to_owned())
                }
                _ => return Err("unknown or duplicate run option"),
            }
        }
        return Ok(Command::Run {
            path: path.ok_or("run requires --machine FILE")?,
            max_steps: max_steps.unwrap_or(crate::BIOS_MAX_RETIRED),
            idle_limit: idle_limit.unwrap_or(crate::BIOS_IDLE_LIMIT),
            print,
            options,
        });
    }
    if args.len() < 2 || args[0] != "media" {
        return Err("unknown or extra arguments");
    }
    let operation = args[1].to_str().ok_or("invalid media command")?;
    if !matches!(
        operation,
        "generations" | "verify" | "rollback" | "commit" | "compact"
    ) {
        return Err("unknown media command");
    }
    let (mut base, mut journal, mut backup, mut generation) = (None, None, None, None);
    let mut output = None;
    let mut repair = false;
    let mut index = 2;
    while index < args.len() {
        let option = args[index].to_str().ok_or("invalid option")?;
        index += 1;
        if option == "--repair-tail" {
            if repair || operation != "verify" {
                return Err("--repair-tail is only accepted once by verify");
            }
            repair = true;
            continue;
        }
        if !matches!(
            option,
            "--base" | "--journal" | "--backup" | "--to" | "--output"
        ) {
            return Err("unknown option");
        }
        let value = args
            .get(index)
            .filter(|v| !v.is_empty() && !v.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing option value")?;
        index += 1;
        let slot = match option {
            "--base" => &mut base,
            "--journal" => &mut journal,
            "--backup" => &mut backup,
            "--to" => &mut generation,
            "--output" => &mut output,
            _ => unreachable!(),
        };
        if slot.replace(value.clone()).is_some() {
            return Err("duplicate option");
        }
    }
    if output.is_some() && !matches!(operation, "commit" | "compact") {
        return Err("--output is only accepted by commit/compact");
    }
    let action = match operation {
        "generations" if backup.is_none() && generation.is_none() => Action::Generations,
        "verify" if generation.is_none() => match (repair, backup) {
            (false, None) => Action::Verify,
            (true, Some(backup)) => Action::Repair {
                backup: backup.into(),
            },
            _ => return Err("--repair-tail and --backup must be used together"),
        },
        "rollback" | "commit" | "compact" => {
            if operation != "rollback" && backup.is_some() {
                return Err("commit does not accept --backup");
            }
            let value = generation.ok_or("this command requires --to N")?;
            let value = value
                .to_str()
                .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .ok_or("generation must be an unsigned decimal integer")?;
            let generation = value.parse().map_err(|_| "generation is out of range")?;
            if operation == "rollback" {
                Action::Rollback {
                    generation,
                    backup: backup.ok_or("rollback requires --backup")?.into(),
                }
            } else {
                Action::Commit {
                    generation,
                    output: output.ok_or("commit requires --output")?.into(),
                }
            }
        }
        _ => return Err("options are not valid for this command"),
    };
    Ok(Command::Media(MediaCommand {
        base: base.ok_or("--base is required")?.into(),
        journal: journal.ok_or("--journal is required")?.into(),
        action,
    }))
}

fn decimal(value: &std::ffi::OsStr) -> Result<u64, &'static str> {
    value
        .to_str()
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("expected an unsigned decimal integer")?
        .parse()
        .map_err(|_| "integer is out of range")
}

/// 0: valid/closed; 1: corruption, binding or I/O; 2: usage/config; 3: recovery.
pub fn report_exit_code(report: &VerificationReport) -> u8 {
    match report.status {
        Status::Refused(_) => 1,
        _ if report.needs_recovery() => 3,
        _ => 0,
    }
}

fn reason(reason: ClosingReason) -> &'static str {
    match reason {
        ClosingReason::SessionEnd => "session end",
        ClosingReason::Checkpoint => "explicit checkpoint",
        ClosingReason::Recovered => "recovered interrupted session",
        ClosingReason::RetentionAnchor => "retention anchor",
    }
}

pub fn render_report(report: &VerificationReport) -> String {
    let mut text = String::new();
    if let Some(header) = &report.header {
        let digest: String = header
            .base_digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        text.push_str(&format!(
            "base sha256: {digest}\nretained anchor: {}\n",
            header.anchor_generation
        ));
        if header.anchor_generation == 0 {
            text.push_str("generation 0: immutable base, end 128, commands 0, sectors 0\n");
        }
    } else {
        text.push_str("base sha256: unavailable (header or binding refused)\n");
    }
    for g in &report.generations {
        text.push_str(&format!(
            "generation {}: {}, end {}, commands {}, sectors {}\n",
            g.id,
            reason(g.reason),
            g.end_offset,
            g.commands,
            g.sectors
        ));
    }
    if let Some(tip) = &report.unsealed_tip {
        text.push_str(&format!(
            "unsealed generation {}: commands {}, sectors {}\n",
            tip.generation, tip.commands, tip.sectors
        ));
    }
    text.push_str(&format!(
        "recoverable-tail bytes: {}\n",
        report.discarded_bytes
    ));
    match &report.status {
        Status::Refused(refusal) => {
            text.push_str(&format!(
                "verification refused at {}: {}\n",
                refusal.failure_offset, refusal.reason
            ));
            if let Some(offset) = refusal.later_generation_end {
                text.push_str(&format!(
                    "interior damage: later intact generation marker at {offset}\n"
                ));
            }
            if refusal.incomplete_anchor {
                text.push_str("incomplete anchor: recovery refused\n");
            }
            text.push_str("latest reconstructible state: unavailable; restore a valid backup\n");
        }
        status => {
            if let Status::TornTail(tail) = status {
                text.push_str(&format!("recoverable tail: truncate at {}, discard {} bytes; failure at {}: {}\ntail classification assumes append-crash damage; discarded bytes are uncertain\n",
                    report.safe_offset, report.discarded_bytes, tail.failure_offset, tail.reason));
            }
            if let Some(tip) = &report.unsealed_tip {
                text.push_str(&format!(
                    "latest reconstructible state: generation {} (unsealed), end {}\n",
                    tip.generation, report.safe_offset
                ));
            } else if let Some(id) = report.last_closed_generation {
                text.push_str(&format!(
                    "latest reconstructible state: generation {id} (closed), end {}\n",
                    report.safe_offset
                ));
            }
            text.push_str(if report.needs_recovery() {
                "status: recovery required\n"
            } else {
                "status: fully valid/closed\n"
            });
        }
    }
    text.push_str("closed markers do not establish filesystem health or bootability\n");
    text
}

fn execute(command: MediaCommand, output: &mut impl Write) -> Result<u8, ToolError> {
    let access = if matches!(
        command.action,
        Action::Repair { .. } | Action::Rollback { .. }
    ) {
        Access::Mutate
    } else {
        Access::Inspect
    };
    let mut tool = JournalTool::open(&command.base, &command.journal, access)?;
    if !tool.has_lock() {
        writeln!(
            output,
            "no lock was taken: the lock file is absent and its directory is read-only"
        )?;
    }
    let scan = match command.action {
        Action::Commit {
            generation,
            output: destination,
        } => {
            tool.commit(generation, &destination)?;
            writeln!(
                output,
                "exported generation {generation} to {}",
                destination.display()
            )?;
            return Ok(0);
        }
        Action::Generations | Action::Verify => tool.verify()?,
        Action::Repair { backup } => tool.repair_tail(&backup)?,
        Action::Rollback { generation, backup } => {
            tool.validate_rollback(generation)?;
            writeln!(output, "rollback to generation {generation}: later generations and any open tip will be removed")?;
            tool.rollback(generation, &backup)?
        }
    };
    output.write_all(render_report(&scan.report).as_bytes())?;
    Ok(report_exit_code(&scan.report))
}

/// The binary delegates here so argument, output and exit contracts are tested
/// without starting a process or assembling a guest for media operations.
pub fn run(args: &[OsString], output: &mut impl Write, errors: &mut impl Write) -> u8 {
    let command = match parse(args) {
        Ok(command) => command,
        Err(error) => {
            let _ = writeln!(errors, "{USAGE}{error}");
            return 2;
        }
    };
    let result = match command {
        Command::Usage => output
            .write_all(USAGE.as_bytes())
            .map(|_| 0)
            .map_err(ToolError::from),
        Command::Boot => crate::run_boot(crate::BOOT_PROGRAM)
            .map_err(|e| ToolError::Io(io::Error::other(e.to_string())))
            .and_then(|serial| {
                output
                    .write_all(&serial)
                    .map(|_| 0)
                    .map_err(ToolError::from)
            }),
        Command::Bios { path, options } => run_bios(&path, options, output, errors)
            .map_err(|error| ToolError::Io(io::Error::other(format!("Error: {error:?}")))),
        Command::Check { path, options } => {
            check_machine(&path, options, errors).map_err(frontend_error)
        }
        Command::Run {
            path,
            max_steps,
            idle_limit,
            print,
            options,
        } => run_machine(
            &path,
            options,
            max_steps,
            idle_limit,
            print.as_deref(),
            output,
            errors,
        )
        .map_err(frontend_error),
        Command::Media(command) => execute(command, output),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            let code = error_exit_code(&error);
            let _ = writeln!(errors, "{error}");
            code
        }
    }
}

fn frontend_error(error: anyhow::Error) -> ToolError {
    ToolError::Io(io::Error::other(error.to_string()))
}

fn check_machine(
    path: &std::path::Path,
    options: assembly::AssemblyOptions,
    errors: &mut impl Write,
) -> anyhow::Result<u8> {
    let loaded = machine_file::load_for_assembly(path, options)?;
    for note in loaded.check()? {
        writeln!(errors, "{note}")?;
    }
    Ok(0)
}

fn run_machine(
    path: &std::path::Path,
    options: assembly::AssemblyOptions,
    max_steps: u64,
    idle_limit: u64,
    print: Option<&str>,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> anyhow::Result<u8> {
    use assembly::{DeviceId, ObservationPolicy, ObservationRequest, OutputKind, RunPolicy};
    let loaded = machine_file::load_for_assembly(path, options)?;
    let printable =
        |device: DeviceId| matches!(device, DeviceId::Mda | DeviceId::Vga | DeviceId::Uart);
    let selected = if let Some(name) = print {
        let (_, device) = loaded
            .cards
            .iter()
            .find(|(id, _)| id == name)
            .ok_or_else(|| anyhow::anyhow!("output card {name:?} is not configured"))?;
        if !printable(*device) {
            writeln!(errors, "card {name:?} has no printable output")?;
            return Ok(2);
        }
        Some(*device)
    } else {
        loaded
            .cards
            .iter()
            .find(|(_, device)| matches!(device, DeviceId::Mda | DeviceId::Vga))
            .or_else(|| {
                loaded
                    .cards
                    .iter()
                    .find(|(_, device)| *device == DeviceId::Uart)
            })
            .map(|(_, device)| *device)
    };
    let mut observation = ObservationPolicy::default();
    if let Some(device) = selected {
        observation.routes.push(ObservationRequest {
            device,
            output: if device == DeviceId::Uart {
                OutputKind::Bytes(device::ByteStreamKind::SerialTx)
            } else {
                OutputKind::TextScanout
            },
            byte_limit: 1_048_576,
        });
    }
    let mut assembled = loaded.assemble(&observation)?;
    for warning in &assembled.unlisted_firmware {
        writeln!(errors, "{warning}")?;
    }
    let result = assembled.run_session(&RunPolicy::natural(Some(max_steps), Some(idle_limit)));
    let result = result.and_then(|outcome| {
        if matches!(
            outcome,
            machine::RunOutcome::Halted | machine::RunOutcome::RetiredCapReached
        ) {
            crate::finish_persistence(&mut assembled, errors)?;
        }
        Ok(outcome)
    });
    let mut stop = crate::classify_bios_stop(&assembled.machine, result);
    let captured = match selected {
        Some(DeviceId::Uart) => assembled
            .observation
            .drain_bytes(DeviceId::Uart, device::ByteStreamKind::SerialTx),
        Some(device) => assembled
            .capture_text(device)
            .and_then(|capture| display_dump(capture.frame, device)),
        None => Ok(Vec::new()),
    };
    let bytes = match captured {
        Ok(bytes) => bytes,
        Err(error) => {
            stop = crate::StopReason::ObservationFailed {
                detail: error.to_string(),
            };
            Vec::new()
        }
    };
    let (message, failed) = bios_stop(&stop);
    writeln!(errors, "{}", message.replace(['\r', '\n'], " "))?;
    output.write_all(&bytes)?;
    Ok(u8::from(failed))
}

fn run_bios(
    path: &std::path::Path,
    options: assembly::AssemblyOptions,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> anyhow::Result<u8> {
    let run = crate::run_bios_file(path, options, errors)?;
    output
        .write_all(&run.com1)
        .context("writing serial output")?;
    write_bios_stop(&run.stop, errors)
}

fn write_bios_stop(stop: &crate::StopReason, errors: &mut impl Write) -> anyhow::Result<u8> {
    let (message, failed) = bios_stop(stop);
    writeln!(errors, "{}", message.replace(['\r', '\n'], " "))?;
    Ok(u8::from(failed))
}

fn bios_stop(stop: &crate::StopReason) -> (String, bool) {
    use crate::StopReason::*;
    let (reason, failed) = match stop {
        Halt => ("halt".into(), false),
        ResetRequested => ("reset requested".into(), false),
        BudgetExhausted => ("budget exhausted".into(), false),
        Unsupported { detail } => (format!("unsupported instruction: {detail}"), true),
        Unmapped { detail } => (format!("execution error: {detail}"), true),
        Exception { vector } => (format!("exception {vector}"), true),
        Fault { detail } => (format!("cpu fault: {detail}"), true),
        ConfigFailed { detail } => (format!("configuration error: {detail}"), true),
        PolicyFailed { detail } => (format!("run policy error: {detail}"), true),
        ObservationFailed { detail } => (format!("observation error: {detail}"), true),
        PersistenceFailed { detail } => (format!("persistence error: {detail}"), true),
    };
    (format!("stopped: {reason}"), failed)
}

fn error_exit_code(error: &ToolError) -> u8 {
    if matches!(error, ToolError::Config(_)) {
        2
    } else {
        1
    }
}

fn display_dump(
    frame: video::Frame,
    device: assembly::DeviceId,
) -> Result<Vec<u8>, assembly::ObservationError> {
    let video::Frame::Text(grid) = frame else {
        return Err(assembly::ObservationError::GraphicsMode(device));
    };
    Ok(format!("{}\n", grid.to_dump()).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly::DeviceId;

    #[test]
    fn text_dump_reports_graphics_mode_explicitly_and_preserves_text_bytes() {
        let frame = video::Frame::Pixels(video::IndexedPixels {
            width: 1,
            height: 1,
            repeat: (1, 1),
            indices: vec![0],
            palette: vec![[0; 3]].into_boxed_slice(),
        });
        let error = display_dump(frame, DeviceId::Vga).unwrap_err();
        assert_eq!(
            error,
            assembly::ObservationError::GraphicsMode(DeviceId::Vga)
        );
        assert_eq!(error.to_string(), "display is in graphics mode: Vga");
        let grid = video::render_text_mode(&video::TextModeState {
            vram: &[b'A', 7],
            cols: 1,
            rows: 1,
            start_cell: 0,
            cursor_cell: 0,
            cursor_visible: false,
            attributes: video::TextAttributeSet::Colour {
                blink_enabled: false,
            },
        });
        let expected = format!("{}\n", grid.to_dump()).into_bytes();
        assert_eq!(
            display_dump(video::Frame::Text(grid), DeviceId::Vga).unwrap(),
            expected
        );
    }

    #[test]
    fn file_run_parser_preserves_policy_defaults_and_explicit_limits() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            parse(&args(&["run", "--machine", "at.toml"])).unwrap(),
            Command::Run {
                path: "at.toml".into(),
                max_steps: 20_000_000,
                idle_limit: 1_000_000,
                print: None,
                options: assembly::AssemblyOptions::default(),
            }
        );
        assert_eq!(
            parse(&args(&[
                "run",
                "--machine",
                "at.toml",
                "--max-steps",
                "42",
                "--idle-limit",
                "0",
                "--print",
                "screen",
                "--allow-unlisted-firmware"
            ]))
            .unwrap(),
            Command::Run {
                path: "at.toml".into(),
                max_steps: 42,
                idle_limit: 0,
                print: Some("screen".into()),
                options: assembly::AssemblyOptions {
                    allow_unlisted_firmware: true
                },
            }
        );
    }

    #[test]
    fn canonical_path_race_is_exit_one() {
        let error = ToolError::PathIo {
            role: "journal",
            path: PathBuf::from("disk.upfj"),
            source: io::Error::other("journal canonical path changed after locking"),
        };
        assert_eq!(error_exit_code(&error), 1);
    }

    #[test]
    fn synthetic_unmapped_cpu_stop_has_failure_diagnostic() {
        // Inject the stop so reporting does not depend on core memory accesses.
        let assembled = assembly::assemble(&assembly::at_boot(vec![0xF4])).unwrap();
        let stop = crate::classify_bios_stop(
            &assembled.machine,
            Err(assembly::RunError::Machine(machine::MachineError::Cpu(
                cpu::CpuError::Unmapped { addr: 0x100000 },
            ))),
        );
        let mut errors = Vec::new();
        assert_eq!(write_bios_stop(&stop, &mut errors).unwrap(), 1);
        let message = String::from_utf8(errors).unwrap();
        assert!(message
            .starts_with("stopped: execution error: unmapped guest access at 0x100000; cs:ip "));
        assert_eq!(message.lines().count(), 1);
        assert!(message.ends_with('\n'));
    }

    #[test]
    fn bios_stop_classifies_every_outcome() {
        use crate::StopReason::*;
        for stop in [Halt, ResetRequested, BudgetExhausted] {
            let (message, failed) = bios_stop(&stop);
            assert!(!failed);
            if stop == BudgetExhausted {
                assert_eq!(message, "stopped: budget exhausted");
            }
        }
        for stop in [
            PersistenceFailed {
                detail: "journal".into(),
            },
            Unsupported {
                detail: "opcode".into(),
            },
            Unmapped {
                detail: "address".into(),
            },
            Exception { vector: 13 },
            Fault {
                detail: "vector 13 at 0008:1000".into(),
            },
            ConfigFailed {
                detail: "config".into(),
            },
            PolicyFailed {
                detail: "budget".into(),
            },
            ObservationFailed {
                detail: "route".into(),
            },
        ] {
            assert!(bios_stop(&stop).1);
        }
    }

    #[test]
    fn bios_parser_preserves_firmware_flags_and_boot_mode() {
        for args in [
            vec!["--bios", "firmware.bin"],
            vec!["--bios", "firmware.bin", "--allow-unlisted-firmware"],
            vec!["--allow-unlisted-firmware", "--bios", "firmware.bin"],
            vec![
                "--bios",
                "--allow-unlisted-firmware",
                "firmware.bin",
                "--allow-unlisted-firmware",
            ],
        ] {
            let allow = args.contains(&"--allow-unlisted-firmware");
            let args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
            assert_eq!(
                parse(&args).unwrap(),
                Command::Bios {
                    path: PathBuf::from("firmware.bin"),
                    options: assembly::AssemblyOptions {
                        allow_unlisted_firmware: allow
                    },
                }
            );
        }
        assert_eq!(
            parse(&["--allow-unlisted-firmware".into(), "--boot".into()]).unwrap(),
            Command::Boot
        );
        for args in [
            vec!["--bios"],
            vec!["--bios", "firmware.bin", "extra"],
            vec!["--allow-unlisted-firmware"],
        ] {
            assert!(parse(&args.into_iter().map(OsString::from).collect::<Vec<_>>()).is_err());
        }
    }
}
