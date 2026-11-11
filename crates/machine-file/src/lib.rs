// SPDX-License-Identifier: AGPL-3.0-or-later
//! Parsed and validated machine descriptions for the IBM PC/AT emulator.
//!
//! TOML and YAML are decoded into schema-versioned layers described in
//! `backlog/docs/design/machine-files.md` § Schema and § Versioning, extends
//! and errors. Omitted fields remain omitted so the merger can distinguish
//! inheritance from explicit overrides. Recognised reserved names and
//! cycle-exact timing requests receive positioned diagnostics.
//!
//! Complete documents lower to `assembly::MachineConfig` with board/card
//! defaults and images checked with SHA-256 during lowering. Volatile/readonly
//! images remain inline; journal bases retain paths and are reread at mounting.
//! Assembly owns hardware validation. Inheritance resolves before lowering.
//! This crate does not model hardware registers.

use std::{fmt, ops::Range, path::Path, path::PathBuf};

use serde::Deserialize;

mod integrity;
mod lower;
mod origin;
mod paths;
mod resolve;
pub mod schema;
mod source;
mod yaml;

pub use integrity::{lower, lower_with_options};
pub use lower::{FileImageReader, ImageReader, LowerError};
pub use origin::{Origin, Origins};
pub use resolve::resolve;

pub use schema::v1::MachineFile;

pub(crate) const MAX_MACHINE_FILE_BYTES: usize = 1_048_576;

pub(crate) fn check_size(path: &Path, length: usize) -> Result<(), ParseError> {
    if length > MAX_MACHINE_FILE_BYTES {
        return Err(ParseError::new(
            path,
            None,
            "machine file exceeds size limit of 1048576 bytes",
            None,
        ));
    }
    Ok(())
}

/// A format-neutral diagnostic with optional source position and snippet.
#[derive(Debug, thiserror::Error)]
pub struct ParseError {
    path: PathBuf,
    line_column: Option<(usize, usize)>,
    message: String,
    snippet: Option<String>,
    span: Option<Range<usize>>,
}

impl ParseError {
    /// Construct a diagnostic without retaining a format-specific error chain.
    pub fn new(
        path: impl Into<PathBuf>,
        line_column: Option<(usize, usize)>,
        message: impl Into<String>,
        snippet: Option<String>,
    ) -> Self {
        Self {
            path: path.into(),
            line_column,
            message: message.into(),
            snippet,
            span: None,
        }
    }

    fn from_toml(path: &Path, input: &str, source: toml::de::Error) -> Self {
        let span = source.span();
        let prefix = span.as_ref().and_then(|span| input.get(..span.start));
        let line_column = prefix.and_then(|prefix| {
            let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
            let column = prefix.rsplit('\n').next()?.chars().count() + 1;
            Some((line, column))
        });
        let snippet = prefix.and_then(|prefix| {
            let start = prefix.rfind('\n').map_or(0, |index| index + 1);
            let line = input.get(start..)?.split('\n').next()?;
            let column = prefix.get(start..)?.chars().count();
            Some(source_snippet(line, column))
        });
        let mut error = Self::new(path, line_column, source.message(), snippet);
        error.span = span;
        error
    }

    /// The path supplied by the caller; parsing never resolves it.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// One-based line and character column, when the parser supplies a position.
    pub fn line_column(&self) -> Option<(usize, usize)> {
        self.line_column
    }

    /// The diagnostic without file, position or snippet decoration.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// A bounded source window and a caret, when available.
    pub fn snippet(&self) -> Option<&str> {
        self.snippet.as_deref()
    }

    /// The parser's byte range, when available, including missing-field spans.
    pub fn span(&self) -> Option<Range<usize>> {
        self.span.clone()
    }
}

fn source_snippet(line: &str, column: usize) -> String {
    const CONTEXT: usize = 40;

    let line = line.strip_suffix('\r').unwrap_or(line);
    let length = line.chars().count();
    let column = column.min(length);
    let first = column.saturating_sub(CONTEXT);
    let last = column.saturating_add(CONTEXT + 1).min(length);
    let mut text = String::new();
    let mut padding = String::new();
    if first > 0 {
        text.push_str("...");
        padding.push_str("   ");
    }
    for (index, character) in line.chars().enumerate().skip(first).take(last - first) {
        text.push(character);
        if index < column {
            padding.push(if character == '\t' { '\t' } else { ' ' });
        }
    }
    if last < length {
        text.push_str("...");
    }
    format!("{text}\n{padding}^")
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.path.display())?;
        if let Some((line, column)) = self.line_column {
            write!(f, "line {line}, column {column}: ")?;
        }
        f.write_str(&self.message)?;
        if let Some(snippet) = &self.snippet {
            write!(f, "\n{snippet}")?;
        }
        Ok(())
    }
}

/// Parse one TOML layer without I/O, merging, defaults or hardware validation.
///
/// The version peek intentionally ignores other fields, so a newer schema is
/// diagnosed before its fields are interpreted using the current schema.
pub fn parse_toml(path: impl AsRef<Path>, input: &str) -> Result<MachineFile, ParseError> {
    check_size(path.as_ref(), input.len())?;
    #[derive(Deserialize)]
    struct SchemaPeek {
        schema: schema::v1::SchemaVersion,
    }

    let parse = || -> Result<MachineFile, toml::de::Error> {
        let peek: SchemaPeek = toml::from_str(input)?;
        let _version = peek.schema;
        toml::from_str(input)
    };
    parse().map_err(|source| ParseError::from_toml(path.as_ref(), input, source))
}

/// Parse one hardened YAML layer into the same schema used by TOML.
pub fn parse_yaml(path: impl AsRef<Path>, input: &str) -> Result<MachineFile, ParseError> {
    yaml::parse(path.as_ref(), input).map(|(document, _)| document)
}

/// Failure while loading a machine document or lowering its images.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Lower(#[from] LowerError),
}

/// Read a TOML or YAML machine file and return an assembly-validated configuration.
/// The given name selects the format and is retained in diagnostics. Image
/// paths use the target file's directory when the machine path is a symlink.
pub fn load(path: impl AsRef<Path>) -> Result<assembly::MachineConfig, LoadError> {
    load_with_options(path, assembly::AssemblyOptions::default())
}

/// Load a machine file with run-only firmware permissions and source diagnostics.
pub fn load_with_options(
    path: impl AsRef<Path>,
    options: assembly::AssemblyOptions,
) -> Result<assembly::MachineConfig, LoadError> {
    load_with_reader_and_options(path, &mut FileImageReader, options)
}

/// Load a machine file with a caller-provided image reader.
///
/// SHA-256 pins are checked against the reader's bytes during lowering, and
/// hardware validation remains mandatory. Volatile/readonly images retain those
/// bytes inline. Journal bases require `ImageReader::read_journal_base`, which
/// synthetic readers refuse by default: persistent replay requires a reproducible
/// immutable file base. File-backed readers retain the verified digest alongside
/// the path, and assembly checks its single read against that digest before mounting.
pub fn load_with_reader<R: ImageReader>(
    path: impl AsRef<Path>,
    reader: &mut R,
) -> Result<assembly::MachineConfig, LoadError> {
    load_with_reader_and_options(path, reader, assembly::AssemblyOptions::default())
}

/// Load with explicit image I/O and run-only firmware permissions.
pub fn load_with_reader_and_options<R: ImageReader>(
    path: impl AsRef<Path>,
    reader: &mut R,
    options: assembly::AssemblyOptions,
) -> Result<assembly::MachineConfig, LoadError> {
    let (document, origins) = resolve(path)?;
    Ok(lower_with_options(&document, &origins, reader, options)?)
}

/// A validated configuration retaining card identities and assembly diagnostics.
/// See machine-files § Frontends: catalogue checks need the resolved ROM bytes.
pub struct LoadedMachine {
    pub config: assembly::MachineConfig,
    /// Card ids in file order, paired with their assembly roles.
    pub cards: Vec<(String, assembly::DeviceId)>,
    diagnostics: lower::AssemblyDiagnostics,
    options: assembly::AssemblyOptions,
}

impl LoadedMachine {
    /// Check hardware and inspect journals without creating, recovering or closing them.
    /// Existing stable locks use the offline inspector's shared-lock path. With
    /// no lock entry, the same scanner runs without a lock and reports that fact.
    /// Guest drives assemble as volatile; no guest instruction is executed.
    pub fn check(&self) -> Result<Vec<String>, LowerError> {
        let mut config = self.config.clone();
        let mut notes = Vec::new();
        for (index, drive) in config.drives.iter_mut().enumerate() {
            if let assembly::WriteMode::Journal { path, .. } = &drive.write {
                let message = inspect_journal(drive, path)
                    .map_err(|message| self.diagnostics.drive_error(index, message))?;
                if let Some(message) = message {
                    notes.push(self.diagnostics.drive_error(index, message).to_string());
                }
                drive.write = assembly::WriteMode::Volatile;
            }
        }
        let assembled = assembly::assemble_with_observation_and_options(
            &config,
            &assembly::ObservationPolicy::default(),
            self.options,
        )
        .map_err(|error| self.diagnostics.map(error))?;
        notes.extend(assembled.unlisted_firmware.iter().map(ToString::to_string));
        Ok(notes)
    }

    /// Assemble without running the guest, preserving defining-layer positions.
    pub fn assemble(
        &self,
        observation: &assembly::ObservationPolicy,
    ) -> Result<assembly::AssembledMachine, LowerError> {
        assembly::assemble_with_observation_and_options(&self.config, observation, self.options)
            .map_err(|error| self.diagnostics.map(error))
    }
}

/// Load for a frontend that needs card identities and positioned assembly errors.
/// Loading alone validates; `LoadedMachine::assemble` also checks ROM identity.
pub fn load_for_assembly(
    path: impl AsRef<Path>,
    options: assembly::AssemblyOptions,
) -> Result<LoadedMachine, LoadError> {
    let (document, origins) = resolve(path)?;
    let (config, diagnostics) =
        lower::lower_tracked(&document, &origins, &mut FileImageReader, options)?;
    let cards = document
        .card
        .iter()
        .flatten()
        .filter_map(|card| {
            use schema::v1::CardKind::*;
            let device = match card.kind? {
                Mda => assembly::DeviceId::Mda,
                VgaText => assembly::DeviceId::Vga,
                Uart16550 => assembly::DeviceId::Uart,
                Fdc765 => assembly::DeviceId::Fdc,
                Ide => assembly::DeviceId::Ide,
            };
            Some((resolve::card_id(card)?.to_owned(), device))
        })
        .collect();
    Ok(LoadedMachine {
        config,
        cards,
        diagnostics,
        options,
    })
}

fn inspect_journal(drive: &assembly::DriveConfig, path: &Path) -> Result<Option<String>, String> {
    use media::offline::{Access, JournalTool, ToolError};
    let assembly::ImageSource::File(base) = &drive.image else {
        return Err("internal error: journal base is not a file".into());
    };
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some("note: journal will be created".into()))
        }
        Err(error) => return Err(format!("journal {}: {error}", path.display())),
        Ok(_) => {}
    }
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("journal {}: {error}", path.display()))?;
    let mut lock_path = canonical.as_os_str().to_os_string();
    lock_path.push(".lock");
    let no_lock = match std::fs::symlink_metadata(Path::new(&lock_path)) {
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(format!("journal lock: {error}")),
    };
    let report = if no_lock {
        // The offline tool may create a missing stable lock. A check cannot do
        // that, so inspect this snapshot with its scanner and disclose no lock.
        let base_bytes =
            std::fs::read(base).map_err(|error| format!("base {}: {error}", base.display()))?;
        let mut file = std::fs::File::open(&canonical)
            .map_err(|error| format!("journal {}: {error}", path.display()))?;
        media::scan_reader(&mut file, &base_bytes, drive.kind.image_bytes())
            .map_err(|error| format!("journal {}: {error}", path.display()))?
            .report
    } else {
        let mut tool = match JournalTool::open(base, path, Access::Inspect) {
            Ok(tool) => tool,
            Err(ToolError::PathIo { source, .. })
                if source.kind() == std::io::ErrorKind::WouldBlock =>
            {
                return Ok(Some(
                    "warning: journal is in use and was not inspected".into(),
                ));
            }
            Err(error) => return Err(error.to_string()),
        };
        tool.verify().map_err(|error| error.to_string())?.report
    };
    if let media::Status::Refused(refusal) = &report.status {
        return Err(format!(
            "journal {}: verification refused at {}: {}",
            path.display(),
            refusal.failure_offset,
            refusal.reason
        ));
    }
    let mut notes = Vec::new();
    if no_lock {
        notes.push("note: journal lock is absent; inspected without a lock");
    }
    if report.needs_recovery() {
        notes.push("warning: journal recovery will run on the next mount");
    }
    Ok((!notes.is_empty()).then(|| notes.join("; ")))
}
