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
//! defaults and images checked with SHA-256 during lowering. Single volatile/readonly
//! images remain inline. Frontend loads retain verified disk descriptors through assembly.
//! Assembly owns hardware validation. Inheritance resolves before lowering.
//! This crate does not model hardware registers.

use std::{fmt, ops::Range, path::Path, path::PathBuf};

use serde::Deserialize;

mod firmware_discovery;
pub mod firmware_package;
mod integrity;
mod lower;
mod origin;
mod paths;
pub mod pc_names;
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
/// bytes inline. Journal disks require `ImageReader::verify_journal_base`, which
/// synthetic readers refuse by default. This configuration-only API retains the
/// digest and path; `load_for_assembly` also retains verified descriptors so a
/// frontend launch does not hash or copy its disk bases again.
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
    bases: std::collections::BTreeMap<PathBuf, media::VerifiedBase>,
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
            if let Some(list) = &mut drive.media {
                for medium in &mut list.entries {
                    if let assembly::WriteMode::Journal { path, .. } = &medium.write {
                        let assembly::ImageSource::File(base) = &medium.image else {
                            return Err(self
                                .diagnostics
                                .drive_error(index, "internal error: journal base is not a file"));
                        };
                        let message = inspect_journal(base, path, drive.kind.image_bytes())
                            .map_err(|message| self.diagnostics.drive_error(index, message))?;
                        if let Some(message) = message {
                            notes.push(self.diagnostics.drive_error(index, message).to_string());
                        }
                        medium.write = assembly::WriteMode::Volatile;
                    }
                }
            }
            if let assembly::WriteMode::Journal { path, .. } = &drive.write {
                let assembly::ImageSource::File(base) = &drive.image else {
                    return Err(self
                        .diagnostics
                        .drive_error(index, "internal error: journal base is not a file"));
                };
                let message = inspect_journal(base, path, drive.kind.image_bytes())
                    .map_err(|message| self.diagnostics.drive_error(index, message))?;
                if let Some(message) = message {
                    notes.push(self.diagnostics.drive_error(index, message).to_string());
                }
                drive.write = assembly::WriteMode::Volatile;
            }
        }
        for device in &mut config.devices {
            let assembly::DeviceKind::Rtc(rtc) = &mut device.kind else {
                continue;
            };
            let assembly::NvramSource::Journal {
                image,
                expected_sha256,
                journal,
                ..
            } = &rtc.nvram
            else {
                continue;
            };
            use sha2::{Digest, Sha256};
            let error = |message| {
                self.diagnostics
                    .nvram_error("board.rtc.nvram.image", message)
            };
            let bytes = std::fs::read(image)
                .map_err(|e| error(format!("image {}: {e}", image.display())))?;
            let actual: [u8; 32] = Sha256::digest(&bytes).into();
            if &actual != expected_sha256 {
                return Err(error(format!(
                    "image sha256 mismatch: expected {:x}, actual {:x}",
                    sha2::digest::Output::<Sha256>::from(*expected_sha256),
                    sha2::digest::Output::<Sha256>::from(actual)
                )));
            }
            let message = inspect_journal(image, journal, 64).map_err(|message| {
                self.diagnostics
                    .nvram_error("board.rtc.nvram.journal", message)
            })?;
            if let Some(message) = message {
                notes.push(
                    self.diagnostics
                        .nvram_error("board.rtc.nvram.journal", message)
                        .to_string(),
                );
            }
            // Hardware validation executes no guest code and must never open a writer.
            rtc.nvram = assembly::NvramSource::Blank;
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
        let mut assembled = assembly::assemble_with_verified_bases(
            &self.config,
            observation,
            self.options,
            &self.bases,
        )
        .map_err(|error| self.diagnostics.map(error))?;
        self.diagnostics
            .label_floppies(&self.config, &mut assembled);
        Ok(assembled)
    }
}

#[derive(Default)]
struct LaunchImageReader<'a> {
    bases: std::collections::BTreeMap<PathBuf, media::VerifiedBase>,
    /// Verified decoded ROMs from compressed firmware packages.
    memory: Option<&'a std::collections::BTreeMap<PathBuf, Vec<u8>>>,
}

impl ImageReader for LaunchImageReader<'_> {
    fn read(&mut self, path: &Path) -> std::io::Result<Vec<u8>> {
        if let Some(bytes) = self.memory.and_then(|memory| memory.get(path)) {
            return Ok(bytes.clone());
        }
        FileImageReader.read(path)
    }
    fn read_journal_base(&mut self, path: &Path) -> std::io::Result<Vec<u8>> {
        FileImageReader.read_journal_base(path)
    }
    fn verify_journal_base(&mut self, path: &Path) -> std::io::Result<media::VerifiedBase> {
        use media::SectorSource;
        if let Some(base) = self.bases.get(path) {
            base.check_unchanged()?;
            return Ok(base.clone());
        }
        let base = FileImageReader.verify_journal_base(path)?;
        self.bases.insert(path.into(), base.clone());
        Ok(base)
    }
}

/// Load for a frontend that needs card identities and positioned assembly errors.
/// Loading alone validates; `LoadedMachine::assemble` also checks ROM identity.
pub fn load_for_assembly(
    path: impl AsRef<Path>,
    options: assembly::AssemblyOptions,
) -> Result<LoadedMachine, LoadError> {
    load_for_assembly_with_rom_dirs(path, options, &[])
}

/// Load a launch configuration, resolving missing or mismatched pinned ROMs in
/// explicit host search directories. Correct declared paths win. The source
/// file is unchanged, and verified disk descriptors are retained as usual.
pub fn load_for_assembly_with_rom_dirs(
    path: impl AsRef<Path>,
    options: assembly::AssemblyOptions,
    roots: &[PathBuf],
) -> Result<LoadedMachine, LoadError> {
    use assembly::firmware_discovery::{FirmwareRoot, RootOrigin};
    let roots = roots
        .iter()
        .map(|path| FirmwareRoot::new(path, RootOrigin::CommandLine))
        .collect::<Vec<_>>();
    let mut warnings = Vec::new();
    let result = load_for_assembly_with_firmware_roots(path, options, &roots, &mut warnings);
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    result
}

/// Load with labelled host ROM roots, reporting skipped roots and entries even
/// if subsequent machine validation fails. Package lookup is disabled; folder
/// discovery requires explicit roots.
pub fn load_for_assembly_with_firmware_roots(
    path: impl AsRef<Path>,
    options: assembly::AssemblyOptions,
    roots: &[assembly::firmware_discovery::FirmwareRoot],
    warnings: &mut Vec<String>,
) -> Result<LoadedMachine, LoadError> {
    prepare_for_assembly_with_firmware_roots(path, roots, warnings)?.load(options)
}

/// A resolved host document that can cross the worker boundary without moving
/// verified disk handles or guest execution state between threads. Loading it
/// revalidates pins in place and performs no directory discovery.
pub struct PreparedMachineFile {
    firmware_sources: Vec<image_store::Digest>,
    /// Decoded ROMs of compressed firmware packages, keyed by the never-created
    /// path the document names; see [`PreparedMachineFile::firmware_image`].
    firmware_images: std::collections::BTreeMap<PathBuf, Vec<u8>>,
    document: MachineFile,
    origins: Origins,
}

/// Locate firmware on a host worker before launch; no guest state is created.
pub fn prepare_for_assembly_with_firmware_roots(
    path: impl AsRef<Path>,
    roots: &[assembly::firmware_discovery::FirmwareRoot],
    warnings: &mut Vec<String>,
) -> Result<PreparedMachineFile, LoadError> {
    prepare_for_assembly_with_firmware_store(path, roots, warnings, None)
}

/// Prepare using a frontend-selected store root. A root that does not exist yet
/// is a silent miss, because frontends pass the shared default before any store
/// has been created. Other unavailable store infrastructure is a warning and a
/// miss; corruption after selecting a package remains fatal.
pub fn prepare_for_assembly_with_store_root(
    path: impl AsRef<Path>,
    roots: &[assembly::firmware_discovery::FirmwareRoot],
    warnings: &mut Vec<String>,
    store_root: Option<&Path>,
) -> Result<PreparedMachineFile, LoadError> {
    let store_root = store_root.filter(|root| !matches!(root.try_exists(), Ok(false)));
    let store = match store_root.map(image_store::Store::open_existing) {
        Some(Ok(store)) => Some(store),
        Some(Err(error)) => {
            warnings.push(format!("firmware store unavailable: {error}"));
            None
        }
        None => None,
    };
    prepare_for_assembly_with_firmware_store(path, roots, warnings, store.as_ref())
}

/// Prepare against an explicit local store, or disable package lookup with None.
/// Import and readiness share this seam without changing global host settings.
pub fn prepare_for_assembly_with_firmware_store(
    path: impl AsRef<Path>,
    roots: &[assembly::firmware_discovery::FirmwareRoot],
    warnings: &mut Vec<String>,
    store: Option<&image_store::Store>,
) -> Result<PreparedMachineFile, LoadError> {
    let (mut document, origins) = resolve(&path)?;
    let roots = roots
        .iter()
        .filter_map(|root| match std::path::absolute(&root.path) {
            Ok(path) => Some(assembly::firmware_discovery::FirmwareRoot::new(
                path,
                root.origin,
            )),
            Err(error) => {
                warnings.push(format!(
                    "{}: skipped ({})",
                    root.origin.label(),
                    error.kind()
                ));
                None
            }
        })
        .collect::<Vec<_>>();
    let mut firmware_images = std::collections::BTreeMap::new();
    let firmware_sources = match store {
        Some(store) => {
            firmware_package::resolve_document(&mut document, store, warnings, &mut firmware_images)
                .map_err(|e| ParseError::new(path.as_ref(), None, e.to_string(), None))?
        }
        None => Vec::new(),
    };
    firmware_discovery::resolve(&mut document, &origins, &roots, warnings, &firmware_images)?;
    Ok(PreparedMachineFile {
        document,
        origins,
        firmware_sources,
        firmware_images,
    })
}

impl PreparedMachineFile {
    /// Contributing immutable packages, in first-use order; provenance only.
    pub fn firmware_sources(&self) -> &[image_store::Digest] {
        &self.firmware_sources
    }

    /// The verified decoded bytes of a ROM that a compressed firmware package
    /// supplied. The document names such a ROM by a path that does not exist,
    /// so readers of the resolved document must look here first.
    pub fn firmware_image(&self, path: &Path) -> Option<&[u8]> {
        self.firmware_images.get(path).map(Vec::as_slice)
    }

    /// Borrow the resolved, discovered document and its unchanged source origins.
    pub fn resolved(&self) -> (&MachineFile, &Origins) {
        (&self.document, &self.origins)
    }

    /// Load the already-resolved document, verifying images without scanning roots.
    pub fn load(&self, options: assembly::AssemblyOptions) -> Result<LoadedMachine, LoadError> {
        let document = &self.document;
        let origins = &self.origins;
        let mut reader = LaunchImageReader {
            memory: Some(&self.firmware_images),
            ..LaunchImageReader::default()
        };
        let (config, diagnostics) = lower::lower_tracked(document, origins, &mut reader, options)?;
        let cards = document
            .card
            .iter()
            .flatten()
            .filter_map(|card| {
                use schema::v1::CardKind::*;
                let device = match card.kind? {
                    Mda => assembly::DeviceId::Mda,
                    VgaText | Vga => assembly::DeviceId::Vga,
                    Uart16550 => assembly::DeviceId::Uart,
                    Fdc765 => assembly::DeviceId::Fdc,
                    Ide => assembly::DeviceId::Ide,
                    Sb16 => assembly::DeviceId::Sb16,
                    LotechEms => assembly::DeviceId::LotechEms,
                };
                Some((resolve::card_id(card)?.to_owned(), device))
            })
            .collect();
        Ok(LoadedMachine {
            config,
            cards,
            diagnostics,
            options,
            bases: reader.bases,
        })
    }
}

fn inspect_journal(
    base: &Path,
    path: &Path,
    geometry_length: u64,
) -> Result<Option<String>, String> {
    use media::offline::{Access, JournalTool, ToolError};
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
        media::scan_reader(&mut file, &base_bytes, geometry_length)
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

#[cfg(test)]
mod vga_tests {
    use super::*;

    #[test]
    fn register_vga_kind_parses_in_toml_and_yaml() {
        for document in [
            parse_toml("machine.toml", "schema=1\n[[card]]\nkind='vga'\n").unwrap(),
            parse_yaml("machine.yaml", "schema: 1\ncard:\n  - kind: vga\n").unwrap(),
        ] {
            let card = &document.card.as_ref().unwrap()[0];
            assert_eq!(card.kind, Some(schema::v1::CardKind::Vga));
        }
    }
}
