// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-backed firmware catalogue checks; machine-files.md § Firmware.
mod tests {
    use crate::assemble;
    use assembly::*;
    fn config() -> MachineConfig {
        assembly::bios(vec![0; 32768], Some(vec![0; 1_474_560]))
    }
    fn fixture() -> Option<Vec<u8>> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/kiselev-bios/bios-xi8088.bin");
        match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let message = format!(
                    "missing fixture {}; run vendor/kiselev-bios/fetch.sh",
                    path.display()
                );
                assert!(
                    std::env::var_os("UNEMUPC_REQUIRE_FIXTURES").as_deref()
                        != Some(std::ffi::OsStr::new("1")),
                    "{message}"
                );
                eprintln!("skipping: {message}");
                None
            }
            Err(error) => panic!("cannot read {}: {error}", path.display()),
        }
    }

    #[test]
    fn real_kiselev_image_passes_inline_and_file_catalogue_checks() {
        let Some(bytes) = fixture() else { return };
        assert_eq!(bytes.len(), 32768);
        let mut config = config();
        for source in [
            ImageSource::Inline(bytes),
            ImageSource::File(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../vendor/kiselev-bios/bios-xi8088.bin"),
            ),
        ] {
            config.images[0].source = source;
            assert!(assemble(&config).unwrap().unlisted_firmware.is_empty());
        }
    }

    #[test]
    fn catalogue_services_supply_omitted_or_equal_declarations() {
        let Some(bytes) = fixture() else {
            return;
        };
        let mut config = config();
        for source in [
            ImageSource::Inline(bytes),
            ImageSource::File(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../vendor/kiselev-bios/bios-xi8088.bin"),
            ),
        ] {
            config.images[0].source = source;
            for declared in [Vec::new(), vec![FirmwareService::Floppy]] {
                config.images[0].provides = declared;
                assert_eq!(crate::validate(&config), Ok(()));
                assert!(assemble(&config).unwrap().unlisted_firmware.is_empty());
            }
        }
    }
}
