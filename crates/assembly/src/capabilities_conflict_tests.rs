// SPDX-License-Identifier: AGPL-3.0-or-later
//! Regression tests for independent resource conflicts in pending settings.
//! See `backlog/docs/design/machine-images.md` §6.

use super::*;

#[test]
fn draft_collects_every_irq_dma_and_port_pair() {
    let mut config = crate::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
    let ids = [DeviceId::Uart, DeviceId::Sb16, DeviceId::LotechEms];
    for id in ids {
        let mut device = CardKind::Uart16550.device();
        device.id = id;
        device.irq = Some(4);
        device.dma = Some(DmaSpec {
            controller: DeviceId::Dma,
            channel: 1,
        });
        config.devices.retain(|d| d.id != id);
        config.devices.push(device);
    }
    let mut draft = Draft::new(config);
    draft.card_ids = ids.into_iter().map(|id| (id, format!("{id:?}"))).collect();
    let diagnostics = validate_draft(&draft).diagnostics;
    for code in ["irq-conflict", "dma-conflict", "port-conflict"] {
        assert_eq!(
            diagnostics.iter().filter(|d| d.code == code).count(),
            3,
            "{code}: {diagnostics:?}"
        );
    }
}

#[test]
fn second_irq_conflict_disables_a_candidate_despite_an_existing_pair() {
    let mut config = crate::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
    config.devices.push(CardKind::Sb16.device());
    let uart = config
        .devices
        .iter_mut()
        .find(|d| d.id == DeviceId::Uart)
        .unwrap();
    uart.irq = Some(6);
    let draft = Draft::new(config);
    let choice = choices(&draft, "card[sb16].irq")
        .into_iter()
        .find(|c| c.value == Value::Integer(5))
        .unwrap();
    assert!(choice.enabled);
    // Keep the first conflict unchanged while adding another claimant.
    let mut draft = draft;
    for device in &mut draft.config.devices {
        if matches!(device.id, DeviceId::Uart | DeviceId::Fdc) {
            device.irq = Some(7);
        }
    }
    let choice = choices(&draft, "card[sb16].irq")
        .into_iter()
        .find(|c| c.value == Value::Integer(7))
        .unwrap();
    assert!(!choice.enabled, "{choice:?}");
    assert!(choice
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("7")));
}

#[test]
fn optional_cards_use_shared_defaults_and_free_frame_choices() {
    let mut draft = Draft::new(crate::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560])));
    draft.set("card[sb16].enabled", Value::Name("installed".into()));
    draft.set("card[lotech-ems].enabled", Value::Name("installed".into()));
    let result = validate_draft(&draft);
    assert!(!result.has_errors(), "{:?}", result.diagnostics);
    result.config.validate().unwrap();
    for id in [DeviceId::Sb16, DeviceId::LotechEms] {
        assert!(result.config.devices.iter().any(|device| device.id == id));
    }
    assert!(result
        .config
        .memory
        .mmio
        .iter()
        .any(|window| window.device == DeviceId::LotechEms));
    draft.set("card[lotech-ems].enabled", Value::Name("none".into()));
    assert!(!validate_draft(&draft)
        .config
        .devices
        .iter()
        .any(|device| device.id == DeviceId::LotechEms));
    assert!(!validate_draft(&draft)
        .config
        .memory
        .mmio
        .iter()
        .any(|window| window.device == DeviceId::LotechEms));
}

#[test]
fn file_role_policy_rejects_cross_role_edits_after_they_are_staged() {
    let mut draft = Draft::new(crate::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560])));
    draft.preserve_card_roles = true;
    draft.set("card[mda].kind", Value::Name("sb16".into()));
    let result = validate_draft(&draft);
    assert!(result
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "field-domain"));
    assert!(result
        .config
        .devices
        .iter()
        .any(|device| matches!(device.kind, DeviceKind::Mda(_))));
}

fn pinned_firmware_draft() -> Draft {
    let mut draft = Draft::for_board(BoardModel::Ibm5170);
    draft.add_card("mda".into(), CardKind::Mda);
    draft.add_card("ide".into(), CardKind::Ide);
    let geometry = HardDiskGeometry {
        cylinders: 615,
        heads: 4,
        sectors_per_track: 17,
        bytes_per_sector: 512,
    };
    draft.config.drives.push(DriveConfig {
        kind: DriveKind::HardDisk { geometry },
        image: ImageSource::Blank {
            size: geometry.image_bytes(),
        },
        expected_sha256: None,
        controller: DeviceId::Ide,
        unit: 0,
        write: WriteMode::Volatile,
        media: None,
    });
    draft.config.images.push(LoadImage {
        base: 0x100000 - KISELEV_XI8088.size,
        source: ImageSource::File("pending-bios.bin".into()),
        kind: ImageKind::Rom,
        provides: KISELEV_XI8088.provides.to_vec(),
    });
    draft.config.system_bios_image = Some(0);
    draft.image_paths.push((0, "firmware.bios".into()));
    draft.firmware_ids.push((0, KISELEV_XI8088.id.into()));
    draft.set(
        "memory.conventional_kib",
        Value::Integer(CONVENTIONAL_KIB.default),
    );
    draft
}

#[test]
fn known_firmware_checks_services_before_its_bytes_are_available() {
    let mut draft = pinned_firmware_draft();
    assert!(validate_draft(&draft)
        .diagnostics
        .iter()
        .any(|d| d.code == "firmware-service" && d.severity == Severity::Error));
    draft.set(
        "firmware.option_roms",
        Value::Name(FirmwareEntry::XUB_R638_XTP_ATA1F0.id.into()),
    );
    let valid = validate_draft(&draft);
    assert!(!valid.has_errors(), "{:?}", valid.diagnostics);
    let removal = choices(&draft, "firmware.option_roms")
        .into_iter()
        .find(|choice| choice.value == Value::Disconnected)
        .unwrap();
    assert!(!removal.enabled);
    assert!(removal.reason.is_some());
}

#[test]
fn known_firmware_rejects_a_conflicting_service_declaration() {
    let mut draft = pinned_firmware_draft();
    draft.config.images[0].provides = vec![FirmwareService::FixedDisk];
    let result = validate_draft(&draft);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == "firmware-provides"
                && d.severity == Severity::Error
                && d.field_paths == ["firmware.bios"]),
        "{:?}",
        result.diagnostics
    );
}

#[test]
fn collecting_errors_retains_the_original_ems_conflict_when_overlap_recollection_is_empty() {
    let mut draft = Draft::new(crate::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560])));
    draft.add_card("ems".into(), CardKind::LotechEms);
    draft.set("card[ems].frame", Value::Integer(0xD000));
    let mut config = validate_draft(&draft).config;
    let mut uart = CardKind::Uart16550.device();
    if let DeviceKind::Uart(cfg) = &mut uart.kind {
        cfg.base = 0x260;
    }
    uart.ports = vec![PortWindow::overlapping(0x260, 0x267)];
    uart.allow_port_overlap = true;
    config.devices.retain(|device| device.id != DeviceId::Uart);
    config.devices.push(uart);
    let launch_error = config.validate().unwrap_err();
    assert!(
        matches!(launch_error, ConfigError::PortOverlap { .. }),
        "{launch_error:?}"
    );
    let diagnostics = validate_draft(&Draft::new(config)).diagnostics;
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "port-conflict"));
}
