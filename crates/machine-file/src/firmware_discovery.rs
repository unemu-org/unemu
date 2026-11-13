// SPDX-License-Identifier: AGPL-3.0-or-later
//! Resolve host ROM paths before lowering, without editing the machine file.
//! See `backlog/docs/design/machine-files.md` § Frontends and § Validation.

use crate::{schema::v1, LoadError, LowerError, MachineFile, Origins};
use assembly::firmware_discovery::{
    locate_firmware_with_roots, FirmwarePin, FirmwareRoot, PinnedPart,
};
use std::path::Path;

pub(crate) fn resolve(
    document: &mut MachineFile,
    origins: &Origins,
    roots: &[FirmwareRoot],
    warnings: &mut Vec<String>,
) -> Result<(), LoadError> {
    if roots.is_empty() {
        return Ok(());
    }
    if let Some(firmware) = &mut document.firmware {
        if let Some(bios) = &mut firmware.bios {
            rom(bios, "firmware.bios", origins, roots, warnings)?;
        }
        for (index, option) in firmware.option_roms.iter_mut().flatten().enumerate() {
            rom(
                option,
                &format!("firmware.option_roms[{index}]"),
                origins,
                roots,
                warnings,
            )?;
        }
    }
    Ok(())
}

fn rom(
    rom: &mut v1::Rom,
    field: &str,
    origins: &Origins,
    roots: &[FirmwareRoot],
    warnings: &mut Vec<String>,
) -> Result<(), LoadError> {
    if (rom.image.is_some() && rom.parts.is_some()) || rom.parts.as_ref().is_some_and(Vec::is_empty)
    {
        return Ok(());
    }
    let pin = |pin: &v1::Sha256| {
        FirmwarePin::from_catalogue(
            pin.as_str(),
            assembly::BoardModel::Ibm5170.firmware_catalogue(),
            None,
        )
    };
    let image = rom
        .image
        .as_ref()
        .map(|image| origins.image_path(&format!("{field}.image"), image));
    let parts = rom
        .parts
        .iter()
        .flatten()
        .enumerate()
        .map(|(index, part)| PinnedPart {
            part: assembly::ImagePart {
                path: origins.image_path(&format!("{field}.parts[{index}].image"), &part.image),
                lane: match part.lane {
                    v1::Lane::Even => assembly::Lane::Even,
                    v1::Lane::Odd => assembly::Lane::Odd,
                },
                offset: part.offset,
            },
            pin: part.sha256.as_ref().map(pin),
        })
        .collect::<Vec<_>>();
    if rom.sha256.is_none() && parts.iter().all(|part| part.pin.is_none()) {
        return Ok(());
    }
    let source = locate_firmware_with_roots(
        roots,
        image.as_deref(),
        rom.sha256.as_ref().map(pin).as_ref(),
        &parts,
        warnings,
    )
    .map_err(|error| LowerError {
        origin: origins.get(field).clone(),
        field: field.into(),
        message: error.to_string(),
        pin_origin: None,
        validation: None,
    })?;
    match source {
        Some(assembly::ImageSource::File(path)) => {
            rom.image = Some(path_text(&path)?);
            rom.parts = None;
        }
        Some(assembly::ImageSource::Interleaved(found)) => {
            if let Some(parts) = &mut rom.parts {
                for (part, found) in parts.iter_mut().zip(found) {
                    part.image = path_text(&found.path)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn path_text(path: &Path) -> Result<String, LoadError> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        crate::ParseError::new(path, None, "discovered ROM path is not valid Unicode", None).into()
    })
}
