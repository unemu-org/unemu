// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pin provenance at the lowering boundary; see machine-files § Errors.
//! Image integrity is still checked by the single lowering implementation.

use crate::{ImageReader, LowerError, MachineFile, Origins};

/// Lower a complete document, retaining the defining pin's source in mismatches.
/// Resolve inheritance first with [`crate::resolve`]. Assembly owns hardware
/// validation; image reads and hashing remain in the lowering implementation.
pub fn lower<R: ImageReader>(
    document: &MachineFile,
    origins: &Origins,
    reader: &mut R,
) -> Result<assembly::MachineConfig, LowerError> {
    lower_with_options(
        document,
        origins,
        reader,
        assembly::AssemblyOptions::default(),
    )
}

/// Lower with run-only firmware permissions while retaining source diagnostics.
/// The option relaxes catalogue membership alone, never format or compatibility.
pub fn lower_with_options<R: ImageReader>(
    document: &MachineFile,
    origins: &Origins,
    reader: &mut R,
    options: assembly::AssemblyOptions,
) -> Result<assembly::MachineConfig, LowerError> {
    crate::lower::lower(document, origins, reader, options).map_err(|mut error| {
        annotate_pin(document, origins, &mut error);
        error
    })
}

pub(crate) fn annotate_pin(document: &MachineFile, origins: &Origins, error: &mut LowerError) {
    // An inherited pin can come from a different layer than its image path.
    if error.message.starts_with("sha256 mismatch for ") {
        if let Some(field) = pin_field(document, origins, error) {
            error
                .message
                .push_str(&format!("; pin set at {}", origins.get(&field)));
        }
    }
}

fn pin_field(document: &MachineFile, origins: &Origins, error: &LowerError) -> Option<String> {
    let label = error.field.as_str();
    if !label.starts_with("card[") {
        return label
            .strip_suffix(".image")
            .map(|field| format!("{field}.sha256"));
    }
    for (i, card) in document.card.iter().flatten().enumerate() {
        let Some(id) = crate::resolve::card_id(card) else {
            continue;
        };
        for (j, drive) in card.drives.iter().flatten().enumerate() {
            if label
                == format!(
                    "card[{id}].drives[{}].image",
                    crate::resolve::drive_label(drive)
                )
                && origins.get(&format!("card[{i}].drives[{j}].image")) == &error.origin
            {
                return Some(format!("card[{i}].drives[{j}].sha256"));
            }
        }
    }
    None
}
