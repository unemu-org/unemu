// SPDX-License-Identifier: AGPL-3.0-or-later
//! The offline journal commands `media generations`, `verify`, `rollback`
//! and `commit` (`backlog/docs/design/persistent-media.md` § Offline
//! commands): they read and change a base image's journal through
//! `journal_files::offline`, with no emulator.

use journal_files::offline::{Access, JournalTool, ToolError};
use journal_files::{ClosingReason, Status, VerificationReport};
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub struct JournalCommand {
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

/// Parse `media OPERATION OPTIONS...`; `args` starts with `media`.
pub fn parse(args: &[OsString]) -> Result<JournalCommand, &'static str> {
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
    Ok(JournalCommand {
        base: base.ok_or("--base is required")?.into(),
        journal: journal.ok_or("--journal is required")?.into(),
        action,
    })
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

fn execute(command: JournalCommand, output: &mut impl Write) -> Result<u8, ToolError> {
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

/// Run a parsed journal command: a refused configuration exits 2, any
/// other failure 1, each printed on the error stream.
pub fn run(command: JournalCommand, output: &mut impl Write, errors: &mut impl Write) -> u8 {
    match execute(command, output) {
        Ok(code) => code,
        Err(error) => {
            let code = error_exit_code(&error);
            let _ = writeln!(errors, "{error}");
            code
        }
    }
}

fn error_exit_code(error: &ToolError) -> u8 {
    if matches!(error, ToolError::Config(_)) {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn canonical_path_race_is_exit_one() {
        let error = ToolError::PathIo {
            role: "journal",
            path: PathBuf::from("disk.upfj"),
            source: io::Error::other("journal canonical path changed after locking"),
        };
        assert_eq!(error_exit_code(&error), 1);
    }
}
