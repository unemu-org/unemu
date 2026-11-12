// SPDX-License-Identifier: AGPL-3.0-or-later
//! IBM Personal Computer AT Technical Reference, Real-Time Clock/CMOS RAM
//! Information, Equipment Byte (Hex 14): coprocessor selection and firmware probes.

use std::{io, path::Path};

use assembly::*;
use machine_file::{lower, parse_toml, parse_yaml, ImageReader, Origins, ParseError};

fn probe_rom() -> Vec<u8> {
    let code = [
        0xFA, 0x31, 0xC0, 0x8E, 0xD8, // CLI; zero DS
        0xB0, 0x14, 0xE6, 0x70, 0xE4, 0x71, 0xA2, 0x02, 0x05, // CMOS equipment
        0xB0, 0x2E, 0xE6, 0x70, 0xE4, 0x71, 0xA2, 0x03, 0x05, // checksum high
        0xB0, 0x2F, 0xE6, 0x70, 0xE4, 0x71, 0xA2, 0x04, 0x05, // checksum low
        0xC7, 0x06, 0x00, 0x05, 0x5A, 0x5A, // presence-check sentinel
        0xDB, 0xE3, // FNINIT
        0xD9, 0x3E, 0x00, 0x05, // FNSTCW [0500h]
        0xDD, 0x3E, 0x06, 0x05, // FNSTSW [0506h]
        0xB8, 0x5A, 0xA5, 0xDF, 0xE0, 0xA3, 0x08, 0x05, // FNSTSW AX; save AX
        0x9B, 0xF4, // WAIT; HLT
    ];
    let mut rom = vec![0xF4; 0x8000];
    rom[..code.len()].copy_from_slice(&code);
    rom[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0, 0, 0, 0xF8]);
    rom
}

struct Images;
impl ImageReader for Images {
    fn read(&mut self, _: &Path) -> io::Result<Vec<u8>> {
        Ok(probe_rom())
    }
}

fn twins(fpu: Option<&str>) -> (MachineConfig, MachineConfig) {
    let toml = format!(
        "schema = 1\n[board]\nmodel = 'ibm-5170'\n[firmware.bios]\nimage = 'bios.bin'\n{}",
        fpu.map(|name| format!("[cpu]\nfpu = '{name}'\n"))
            .unwrap_or_default()
    );
    let yaml = format!(
        "schema: 1\nboard: {{model: ibm-5170}}\nfirmware: {{bios: {{image: bios.bin}}}}\n{}",
        fpu.map(|name| format!("cpu: {{fpu: '{name}'}}\n"))
            .unwrap_or_default()
    );
    let toml_path = Path::new("machines/probe.toml");
    let yaml_path = Path::new("machines/probe.yaml");
    let toml_config = lower(
        &parse_toml(toml_path, &toml).unwrap(),
        &Origins::from_toml(toml_path, &toml).unwrap(),
        &mut Images,
    )
    .unwrap();
    let yaml_config = lower(
        &parse_yaml(yaml_path, &yaml).unwrap(),
        &Origins::from_yaml(yaml_path, &yaml).unwrap(),
        &mut Images,
    )
    .unwrap();
    (toml_config, yaml_config)
}

fn probe(config: &MachineConfig, present: bool, equipment: u8, checksum: [u8; 2]) {
    let mut assembled = assemble_with_options(
        config,
        AssemblyOptions {
            allow_unlisted_firmware: true,
        },
    )
    .unwrap();
    let outcome = assembled.machine.run_bounded(Some(100));
    let mut result = [0; 10];
    assembled
        .machine
        .cpu()
        .read_mem(0x500, &mut result)
        .unwrap();
    assert_eq!(
        &result[..2],
        if present { &[0x7F, 3] } else { &[0xFF, 0xFF] }
    );
    assert_eq!(result[2], equipment);
    assert_eq!(&result[3..5], &checksum);
    assert_eq!(&result[6..], if present { &[0; 4] } else { &[0xFF; 4] });
    assert_eq!(outcome.unwrap(), machine::RunOutcome::Halted);
}

#[test]
fn ibm_5170_default_probe_reports_absent_coprocessor() {
    let config = bios(probe_rom(), Some(vec![0; 1_474_560]));
    let seed = derive_cmos_config(&config).unwrap();
    assert_eq!(seed[0x14] & 2, 0);
    probe(&config, false, seed[0x14], [seed[0x2E], seed[0x2F]]);
}

#[test]
fn fpu_machine_file_twins_select_socket_and_cmos() {
    for (fpu, present) in [(None, false), (Some("none"), false), (Some("80287"), true)] {
        let (toml, yaml) = twins(fpu);
        assert_eq!(toml, yaml);
        let seed = derive_cmos_config(&toml).unwrap();
        assert_eq!(seed[0x14] & 2, if present { 2 } else { 0 });
        let checksum: u16 = seed[0x10..=0x2D].iter().map(|&v| u16::from(v)).sum();
        assert_eq!([seed[0x2E], seed[0x2F]], checksum.to_be_bytes());
        probe(&toml, present, seed[0x14], checksum.to_be_bytes());
    }
}

#[test]
fn explicit_cmos_image_is_preserved_with_80287() {
    let (mut config, _) = twins(Some("80287"));
    let DeviceKind::Rtc(rtc) = &mut config
        .devices
        .iter_mut()
        .find(|d| d.id == DeviceId::Rtc)
        .unwrap()
        .kind
    else {
        panic!("RTC required")
    };
    let mut image = [0; 64];
    image[0x14] = 0x41;
    image[0x2E] = 0xA5;
    image[0x2F] = 0x5A;
    rtc.nvram = NvramSource::Image(image);
    probe(&config, true, 0x41, [0xA5, 0x5A]);
}

#[test]
fn unsupported_fpu_names_have_exact_typed_diagnostics() {
    for name in ["80387", "8087", "present", "80288", "NONE"] {
        let toml = format!("schema = 1\n[cpu]\nfpu = '{name}'\n");
        let yaml = format!("schema: 1\ncpu: {{fpu: '{name}'}}\n");
        let errors: [ParseError; 2] = [
            parse_toml("probe.toml", &toml).unwrap_err(),
            parse_yaml("probe.yaml", &yaml).unwrap_err(),
        ];
        for error in errors {
            assert_eq!(
                error.message(),
                format!("unsupported cpu.fpu `{name}`; allowed values: `none`, `80287`")
            );
        }
    }
}
