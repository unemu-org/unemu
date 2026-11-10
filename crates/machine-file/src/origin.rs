// SPDX-License-Identifier: AGPL-3.0-or-later
//! Format-neutral field origins for machine-file diagnostics.
//!
//! See `backlog/docs/design/machine-files.md` § Errors. Table positions are
//! sufficient to identify a card or image without coupling lowering to TOML.

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};

use serde::{de::IgnoredAny, Deserialize};

use crate::ParseError;

/// The defining file and optional one-based line of a field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub file: PathBuf,
    pub line: Option<usize>,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file.display())?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        Ok(())
    }
}

/// A small field-to-source table, also usable by a future layer merger.
#[derive(Clone, Debug)]
pub struct Origins {
    document: Origin,
    image_file: PathBuf,
    fields: BTreeMap<String, Origin>,
}

impl Origins {
    /// Origins for a programmatically constructed document without positions.
    pub fn new(file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        Self {
            image_file: file.clone(),
            document: Origin { file, line: None },
            fields: BTreeMap::new(),
        }
    }

    pub(crate) fn set_image_file(&mut self, file: PathBuf) {
        self.image_file = file;
    }

    pub(crate) fn image_path(&self, field: &str, image: &str) -> PathBuf {
        let origin = self.get(field);
        let file = if origin.file == self.document.file {
            &self.image_file
        } else {
            &origin.file
        };
        file.parent().unwrap_or(Path::new(".")).join(image)
    }

    /// Record a defining source, including a different file for inherited fields.
    pub fn insert(&mut self, field: impl Into<String>, origin: Origin) {
        self.fields.insert(field.into(), origin);
    }

    /// Look up the closest containing field, falling back to the document.
    pub fn get(&self, field: &str) -> &Origin {
        self.fields
            .iter()
            .filter(|(key, _)| {
                field == key.as_str()
                    || field
                        .strip_prefix(key.as_str())
                        .is_some_and(|tail| tail.starts_with('.') || tail.starts_with('['))
            })
            .max_by_key(|(key, _)| key.len())
            .map_or(&self.document, |(_, origin)| origin)
    }

    /// Retain TOML table spans separately from the format-independent schema.
    pub fn from_toml(file: impl Into<PathBuf>, input: &str) -> Result<Self, ParseError> {
        #[derive(Deserialize, Default)]
        struct Tables {
            board: Option<toml::Spanned<Board>>,
            cpu: Option<toml::Spanned<IgnoredAny>>,
            memory: Option<toml::Spanned<IgnoredAny>>,
            firmware: Option<toml::Spanned<Firmware>>,
            #[serde(default)]
            card: Vec<toml::Spanned<Card>>,
        }
        #[derive(Deserialize)]
        struct Board {
            rtc: Option<toml::Spanned<IgnoredAny>>,
        }
        #[derive(Deserialize)]
        struct Firmware {
            bios: Option<toml::Spanned<IgnoredAny>>,
            #[serde(default)]
            option_roms: Vec<toml::Spanned<IgnoredAny>>,
        }
        #[derive(Deserialize)]
        struct Card {
            #[serde(default)]
            drives: Vec<toml::Spanned<IgnoredAny>>,
        }
        let mut origins = Self::new(file);
        let tables: Tables = toml::from_str(input)
            .map_err(|error| ParseError::from_toml(&origins.document.file, input, error))?;
        let mut record = |field: String, start: usize| {
            let start = start + input[start..].len() - input[start..].trim_start().len();
            origins.insert(
                field,
                Origin {
                    file: origins.document.file.clone(),
                    line: Some(input[..start].bytes().filter(|&b| b == b'\n').count() + 1),
                },
            );
        };
        if let Some(board) = tables.board {
            record("board".into(), board.span().start);
            if let Some(rtc) = board.into_inner().rtc {
                record("board.rtc".into(), rtc.span().start);
            }
        }
        if let Some(cpu) = tables.cpu {
            record("cpu".into(), cpu.span().start);
        }
        if let Some(memory) = tables.memory {
            record("memory".into(), memory.span().start);
        }
        if let Some(firmware) = tables.firmware {
            record("firmware".into(), firmware.span().start);
            let firmware = firmware.into_inner();
            if let Some(bios) = firmware.bios {
                record("firmware.bios".into(), bios.span().start);
            }
            for (index, rom) in firmware.option_roms.into_iter().enumerate() {
                record(format!("firmware.option_roms[{index}]"), rom.span().start);
            }
        }
        for (index, card) in tables.card.into_iter().enumerate() {
            record(format!("card[{index}]"), card.span().start);
            for (drive_index, drive) in card.into_inner().drives.into_iter().enumerate() {
                record(
                    format!("card[{index}].drives[{drive_index}]"),
                    drive.span().start,
                );
            }
        }
        Ok(origins)
    }

    /// Retain YAML table locations without adding syntax wrappers to the schema.
    pub fn from_yaml(file: impl Into<PathBuf>, input: &str) -> Result<Self, ParseError> {
        use serde_saphyr::{Location, Spanned};
        #[derive(Deserialize)]
        struct Tables {
            board: Option<Spanned<Board>>,
            cpu: Option<Spanned<IgnoredAny>>,
            memory: Option<Spanned<IgnoredAny>>,
            firmware: Option<Spanned<Firmware>>,
            #[serde(default)]
            card: Vec<Spanned<Card>>,
        }
        #[derive(Deserialize)]
        struct Board {
            rtc: Option<Spanned<Rtc>>,
        }
        #[derive(Deserialize)]
        struct Rtc {
            start: Option<Spanned<IgnoredAny>>,
        }
        #[derive(Deserialize)]
        struct Firmware {
            bios: Option<Spanned<IgnoredAny>>,
            #[serde(default)]
            option_roms: Vec<Spanned<IgnoredAny>>,
        }
        #[derive(Deserialize)]
        struct Card {
            #[serde(default)]
            drives: Vec<Spanned<IgnoredAny>>,
        }
        let mut origins = Self::new(file);
        let tables: Tables = crate::yaml::decode(&origins.document.file, input)?;
        let mut record = |field: String, location: Location| {
            origins.insert(
                field,
                Origin {
                    file: origins.document.file.clone(),
                    line: usize::try_from(location.line())
                        .ok()
                        .filter(|line| *line > 0),
                },
            );
        };
        if let Some(board) = tables.board {
            record("board".into(), board.referenced);
            if let Some(rtc) = board.value.rtc {
                record("board.rtc".into(), rtc.referenced);
                if let Some(start) = rtc.value.start {
                    // YAML 1.2 leaves dates as strings, unlike TOML. Inspect
                    // the definition's spelling to enforce the shared quoted-date rule.
                    let location = start.defined;
                    let first = input
                        .trim_start_matches('\u{feff}')
                        .split('\n')
                        .nth(location.line().saturating_sub(1) as usize)
                        .and_then(|line| {
                            line.chars()
                                .nth(location.column().saturating_sub(1) as usize)
                        });
                    if !matches!(first, Some('"' | '\'')) {
                        return Err(crate::yaml::diagnostic(
                            &origins.document.file,
                            input,
                            Some(start.referenced),
                            "RTC start must be a quoted string".into(),
                        ));
                    }
                }
            }
        }
        if let Some(cpu) = tables.cpu {
            record("cpu".into(), cpu.referenced);
        }
        if let Some(memory) = tables.memory {
            record("memory".into(), memory.referenced);
        }
        if let Some(firmware) = tables.firmware {
            record("firmware".into(), firmware.referenced);
            if let Some(bios) = firmware.value.bios {
                record("firmware.bios".into(), bios.referenced);
            }
            for (index, rom) in firmware.value.option_roms.into_iter().enumerate() {
                record(format!("firmware.option_roms[{index}]"), rom.referenced);
            }
        }
        for (index, card) in tables.card.into_iter().enumerate() {
            record(format!("card[{index}]"), card.referenced);
            for (drive_index, drive) in card.value.drives.into_iter().enumerate() {
                record(
                    format!("card[{index}].drives[{drive_index}]"),
                    drive.referenced,
                );
            }
        }
        Ok(origins)
    }
}
