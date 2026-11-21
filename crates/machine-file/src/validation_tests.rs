// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host path diagnostics after catalogue expansion; persistent-media.md § Integration.
mod tests {
    use assembly::*;
    #[test]
    fn floppy_media_validation_reports_original_drive_indices_after_expansion() {
        let temp = assembly::test_support::TempDir::new("catalogue-diagnostics");
        let base = temp.path().join("base.img");
        std::fs::write(&base, vec![0; 1_474_560]).unwrap();
        let mut config = assembly::floppy_boot(vec![0xf4], vec![0; 1_474_560]);
        config.drives[0].media = Some(assembly::FloppyMediaList {
            empty: false,
            entries: (0..3)
                .map(|_| assembly::FloppyMedium {
                    image: ImageSource::File(base.clone()),
                    expected_sha256: None,
                    write: WriteMode::ReadOnly,
                    name: "base.img".into(),
                })
                .collect(),
            inserted: 0,
        });
        let mut second = config.drives[0].clone();
        second.unit = 1;
        second.media = None;
        second.write = WriteMode::journal(temp.path().join("second.upfj"));
        config.drives.push(second);
        assert!(
            matches!(
                crate::validate(&config),
                Err(ConfigError::Journal { drive_index: 1, .. })
            ),
            "journal-mode diagnostics must identify the original second drive"
        );
        config.drives[1].image = ImageSource::File(base.clone());
        config.drives[1].write = WriteMode::journal(&base);
        assert!(
            matches!(
                crate::validate(&config),
                Err(ConfigError::Journal { drive_index: 1, .. })
            ),
            "path diagnostics must identify the original second drive"
        );
        config.drives[1].write = WriteMode::Volatile;
        config.drives[1].image = ImageSource::File(temp.path().join("base.retain-probe"));
        assert!(
            matches!(
                crate::validate(&config),
                Err(ConfigError::DriveInput { drive_index: 1, .. })
            ),
            "input namespace diagnostics must identify the original second drive"
        );
        config.drives[1].image = ImageSource::File(base.clone());
        config.drives[1].write = WriteMode::journal(&base);
        config.drives[0].media.as_mut().unwrap().entries.clear();
        config.drives[0].media.as_mut().unwrap().empty = true;
        assert!(
            matches!(
                crate::validate(&config),
                Err(ConfigError::Journal { drive_index: 1, .. })
            ),
            "empty catalogues must not shift following drive diagnostics"
        );
    }
}
