// SPDX-License-Identifier: AGPL-3.0-or-later
//! The YAML half of the format-neutral field origins for machine-file
//! diagnostics.
//!
//! The origins and their TOML decoder are `pc-config`'s; this crate keeps the
//! YAML parser, so it adds the YAML decoder. See
//! `backlog/docs/design/machine-files.md` § Errors.

use std::path::PathBuf;

use serde::{de::IgnoredAny, Deserialize};

use crate::{Origin, Origins, ParseError};

/// The YAML decoder of [`Origins`]: `Origins::from_yaml`, beside
/// `pc-config`'s `Origins::from_toml`.
pub trait YamlOrigins: Sized {
    /// Retain YAML table locations without adding syntax wrappers to the schema.
    fn from_yaml(file: impl Into<PathBuf>, input: &str) -> Result<Self, ParseError>;
}

impl YamlOrigins for Origins {
    fn from_yaml(file: impl Into<PathBuf>, input: &str) -> Result<Self, ParseError> {
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
        let file = file.into();
        let mut origins = Self::new(file.clone());
        let tables: Tables = crate::yaml::decode(&file, input)?;
        let mut record = |field: String, location: Location| {
            origins.insert(
                field,
                Origin {
                    file: file.clone(),
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
                            &file,
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
        let mut values = Self::new(file.clone());
        crate::source::record(&file, input, true, &mut values)?;
        origins.keep_pins(values);
        Ok(origins)
    }
}
