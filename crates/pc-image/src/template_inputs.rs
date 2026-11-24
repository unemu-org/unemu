// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bound config inputs and draft recipes for the official machine templates.

use super::{recipe_build, Template, FIRMWARE, FIRMWARE_REVISION, TEMPLATES};
use crate::{invalid, Result};
use image_store::Digest;
use toml::{Table, Value};

/// The recipe-built generation; historical template builds retain their version.
pub const VERSION: &str = "1.3.0";

/// One deterministic publication input, independent of its destination directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub name: String,
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

fn encode(table: &Table) -> Result<Vec<u8>> {
    toml::to_string(table)
        .map(String::into_bytes)
        .map_err(|e| invalid(e.to_string()))
}

fn table(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Table {
    fields
        .into_iter()
        .map(|(key, value)| (key.into(), value))
        .collect()
}

fn documents(template: &Template) -> Result<[Document; 2]> {
    let hardware = template.hardware()?;
    let mut machine = hardware.table(true, true);
    let mut disks = Vec::new();
    let mut disk_index = 0;
    for card in machine
        .get_mut("card")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let kind = card
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        for drive in card
            .get_mut("drives")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            let drive = drive
                .as_table_mut()
                .ok_or_else(|| invalid("template drive table required"))?;
            if kind == "ide" {
                let chs = hardware
                    .disks
                    .iter()
                    .flatten()
                    .nth(disk_index)
                    .ok_or_else(|| invalid("template disk geometry missing"))?;
                let id = if disk_index == 0 {
                    "system".to_owned()
                } else {
                    format!("disk-{disk_index}")
                };
                disks.push(Value::Table(table([
                    ("id", id.clone().into()),
                    ("size", (chs.bytes() as i64).into()),
                ])));
                drive.insert("disk".into(), id.into());
                drive.insert("write".into(), "journal".into());
                disk_index += 1;
            } else {
                drive.insert("empty".into(), true.into());
            }
        }
    }
    machine.insert("disks".into(), disks.clone().into());
    let mut firmware = Table::new();
    let mut options = Vec::new();
    let mut uses = Table::new();
    for (index, (id, source)) in ["bios", "vga", "xub"].into_iter().zip(FIRMWARE).enumerate() {
        let entry = crate::firmware::catalogue()
            .iter()
            .find(|entry| entry.id == source.catalogue_id)
            .ok_or_else(|| invalid("template firmware catalogue entry missing"))?;
        let role = crate::firmware::role(entry.kind);
        let binding = assembly::capabilities::firmware_binding(entry);
        let mut rom = table([
            ("use", id.into()),
            ("rom", role.into()),
            ("role", role.into()),
            ("base", (binding.base as i64).into()),
        ]);
        if binding.offset != 0 {
            rom.insert("offset".into(), (binding.offset as i64).into());
        }
        if !binding.provides.is_empty() {
            rom.insert(
                "provides".into(),
                binding
                    .provides
                    .iter()
                    .map(|s| Value::from(s.to_string()))
                    .collect::<Vec<_>>()
                    .into(),
            );
        }
        if index == 0 {
            firmware.insert("bios".into(), rom.into());
        } else {
            options.push(Value::Table(rom));
        }
        let reference = format!("{}:{FIRMWARE_REVISION}", source.repository());
        uses.insert(
            id.into(),
            table([
                ("image", reference.clone().into()),
                ("digest", recipe_build::firmware_pin(&reference)?.into()),
            ])
            .into(),
        );
    }
    firmware.insert("option_roms".into(), options.into());
    machine.insert("firmware".into(), firmware.into());
    let config = Document {
        name: format!("{}.pc.toml", template.name),
        media_type: pc_config::launch::MEDIA_TYPE,
        bytes: encode(&table([
            ("schema", 1.into()),
            ("platform", crate::PLATFORM.into()),
            ("machine", machine.into()),
        ]))?,
    };
    let recipe = table([
        ("recipe", "1.1".into()),
        (
            "machine",
            table([
                (
                    "name",
                    format!("unemu/machines/{}/{}", crate::PLATFORM, template.name).into(),
                ),
                ("platform", crate::PLATFORM.into()),
                ("version", VERSION.into()),
            ])
            .into(),
        ),
        (
            "template",
            table([
                ("config", "config".into()),
                ("disks", disks.into()),
                ("title", template.title.into()),
                ("description", template.description.into()),
                ("labels", Value::Array(Vec::new())),
            ])
            .into(),
        ),
        ("license", table([("expression", "CC0-1.0".into())]).into()),
        (
            "input",
            table([(
                "config",
                table([
                    ("name", config.name.clone().into()),
                    ("size", (config.bytes.len() as i64).into()),
                    ("sha256", Digest::of(&config.bytes).hex().into()),
                    ("bundle", true.into()),
                ])
                .into(),
            )])
            .into(),
        ),
        ("use", uses.into()),
    ]);
    Ok([
        config,
        Document {
            name: format!("{}.machine.toml", template.name),
            media_type: image_store::media_type::RECIPE_CONFIG,
            bytes: encode(&recipe)?,
        },
    ])
}

/// Emit both complete profiles from the same definitions as the historical build.
/// Neither fixtures nor a store are needed to describe their pinned inputs.
pub fn emit() -> Result<Vec<Document>> {
    let mut result = Vec::new();
    for template in TEMPLATES {
        result.extend(documents(&template)?);
    }
    Ok(result)
}
