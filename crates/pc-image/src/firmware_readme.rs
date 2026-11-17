// SPDX-License-Identifier: AGPL-3.0-or-later
//! The generated README layer of a PC firmware package; machine-images §5.3.
//! Plain UTF-8 Markdown (RFC 7763) with LF line ends, generated only from the
//! package config and its layers' licences, so the same package always gets
//! byte-identical text and a reader needs nothing but a text viewer.

use crate::firmware::{role_label, Fit};
use image_store::{firmware::Config, license::Expression};
use std::fmt::Write;

/// Licences of the package's layers, as the import or edit marks them.
pub struct Marks<'a> {
    /// The ROM images' licence, `None` when they are user-supplied.
    pub roms: Option<&'a Expression>,
    /// The source archive's own licence, `None` when it is user-supplied.
    pub source: Option<&'a Expression>,
    /// One entry per config extra, in order.
    pub extras: &'a [Option<Expression>],
    /// Every layer other than the README and the config is open.
    pub all_open: bool,
}

fn or_unknown(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("not recorded")
}
fn list(items: &[String]) -> String {
    if items.is_empty() {
        "unspecified".into()
    } else {
        items.join(", ")
    }
}

/// The README text for `config` (its own `readme` field is ignored).
pub fn generate(config: &Config, marks: &Marks<'_>) -> String {
    let mut out = String::new();
    // Writing to a String cannot fail.
    let mut line = |text: &str| {
        out.push_str(text);
        out.push('\n');
    };
    line(&format!("# {}", config.name));
    line("");
    if let Some(description) = &config.description {
        line(description.trim_end());
        line("");
    }
    line(&format!(
        "- Platform: {}",
        crate::platform_name(&config.platform)
    ));
    line(&format!("- Vendor: {}", or_unknown(&config.vendor)));
    line(&format!("- Version: {}", or_unknown(&config.version)));
    line(&format!("- Date: {}", or_unknown(&config.date)));
    if !config.labels.is_empty() {
        line(&format!("- Labels: {}", config.labels.join(", ")));
    }
    line("");
    line("## Fits");
    line("");
    let fit = Fit::decode(&config.fits).unwrap_or_default();
    line(&format!("- Boards: {}", list(&fit.boards)));
    line(&format!(
        "- Machine classes: {}",
        list(&fit.machine_classes)
    ));
    let catalogue: Vec<String> = config
        .roms
        .iter()
        .filter_map(|r| r.catalogue_id.clone())
        .collect();
    if catalogue.len() == config.roms.len() {
        line(&format!("- Firmware catalogue: {}", catalogue.join(", ")));
    } else {
        line("- Not in the firmware catalogue: the fit and role claims are the user's and unverified.");
    }
    line("");
    line("## ROM images");
    for rom in &config.roms {
        line("");
        line(&format!("### {} ({})", rom.id, role_label(&rom.role)));
        line("");
        if let Some(id) = &rom.catalogue_id {
            line(&format!("- Catalogue id: {id}"));
        }
        line(&format!("- Size: {} bytes", rom.size));
        line(&format!("- SHA-256: {}", rom.sha256));
        if !rom.requires.is_empty() {
            line(&format!("- Requires: {}", rom.requires.join(", ")));
        }
        let mut chip = String::new();
        if let Some(position) = &rom.position {
            let _ = write!(chip, "socket {position}");
        }
        if let Some(number) = &rom.part_number {
            let _ = write!(
                chip,
                "{}part {number}",
                if chip.is_empty() { "" } else { ", " }
            );
        }
        if !chip.is_empty() {
            line(&format!("- Chip: {chip}"));
        }
        if !rom.parts.is_empty() {
            line("- Chips, interleaved into the image above:");
            for p in &rom.parts {
                let mut marking = p.position.clone().unwrap_or_else(|| "unlabelled".into());
                if let Some(number) = &p.part_number {
                    let _ = write!(marking, ", part {number}");
                }
                line(&format!(
                    "  - {marking}: {} lane at offset {}, {} bytes, SHA-256 {}",
                    p.lane, p.offset, p.size, p.sha256
                ));
            }
        }
    }
    line("");
    line("## Provenance");
    line("");
    match &config.provenance {
        None => line("Not recorded."),
        Some(p) => {
            let how = match p.kind {
                Some(kind) => match kind {
                    image_store::firmware::ProvenanceKind::Dumped => "Dumped from",
                    image_store::firmware::ProvenanceKind::Downloaded => "Downloaded from",
                    image_store::firmware::ProvenanceKind::Built => "Built from",
                },
                None => "Origin:",
            };
            line(&format!("{how} {}", p.from.trim_end()));
            line("");
            if let Some(revision) = &p.revision {
                line(&format!("- Revision: {revision}"));
            }
            if let Some(url) = &p.url {
                line(&format!("- Address: {url}"));
            }
            line(&format!("- Date: {}", or_unknown(&p.date)));
        }
    }
    if let Some(notes) = &config.notes {
        line("");
        line("## Notes and known quirks");
        line("");
        line(notes.trim_end());
    }
    line("");
    line("## Licence");
    line("");
    match marks.roms {
        Some(license) => line(&format!(
            "- ROM images: {license}, open (redistributable)."
        )),
        None => line(
            "- ROM images: user-supplied. No redistribution right is recorded; keep this package in private stores and registries.",
        ),
    }
    for source in &config.sources {
        let class = match marks.source {
            Some(license) => format!("{license}, open"),
            None => "no licence recorded, user-supplied".into(),
        };
        line(&format!(
            "- Source archive {} ({}, {} bytes, SHA-256 {}): the corresponding source of {}; {class}.",
            source.filename,
            source.format.as_str(),
            source.size,
            source.blob.hex(),
            source.roms.join(", ")
        ));
    }
    let missing = config.sources.is_empty()
        && marks
            .roms
            .is_some_and(|l| l.needs_source(image_store::license::ACTIVE));
    if missing {
        line("- Source archive: not included. Copyleft firmware without its source is published only to private registries.");
    }
    for (extra, license) in config.extras.iter().zip(marks.extras) {
        match license {
            Some(license) => line(&format!("- Document {}: {license}, open.", extra.filename)),
            None => line(&format!(
                "- Document {}: no licence recorded, user-supplied.",
                extra.filename
            )),
        }
    }
    if marks.all_open {
        line(&format!(
            "- This README and the package config: {}, open.",
            crate::firmware::METADATA_LICENSE
        ));
    } else {
        line("- This README and the package config: user-supplied, like the package.");
    }
    out
}
