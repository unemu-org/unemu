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
/// bytes inline. Journal bases retain file paths: assembly rereads them at mount
/// time without rechecking the pin, even if the reader supplied synthetic bytes.
/// Checking the bytes actually mounted against their pin needs a later assembly
/// change; callers must keep journal bases unchanged between loading and assembly.
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
