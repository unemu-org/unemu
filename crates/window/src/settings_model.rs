// SPDX-License-Identifier: AGPL-3.0-or-later
//! Headless property-sheet state: the PC library's model
//! (`pc_library::settings_model`), re-exported under its old path. The test
//! of a machine file outside any `machines` directory stays here, since it
//! reads where the desktop keeps its machine files.

pub(crate) use pc_library::settings_model::*;

#[cfg(test)]
mod tests {
    use super::*;
    // What the test took from the model's own imports.
    use std::{fs, path::Path};

    /// A machine file outside any `machines` directory keeps its custom layer
    /// in the library's machines directory, never in the tree the program was
    /// built in, and with no such directory Settings refuses instead of
    /// writing into the working directory.
    #[test]
    fn custom_layer_of_a_file_outside_machines_stays_out_of_the_build_tree() {
        let temp = assembly::test_support::TempDir::new("settings-outside-machines");
        fs::write(temp.path().join("bios.bin"), vec![0xF4; 32768]).unwrap();
        let parent = temp.path().join("example.toml");
        fs::write(&parent, "schema=1\nname='Example'\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n").unwrap();
        // The release environment: no switch and no cargo, a data directory.
        let library = crate::machines_dir_from(None, None, Some(temp.path().join("data")));
        let model = SettingsModel::open_with_library(&parent, false, library).unwrap();
        let expected = std::path::absolute(temp.path().join("data/unemu/machines/local"))
            .unwrap()
            .join("example-custom.toml");
        assert_eq!(model.target, expected);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        assert!(
            !model.target.starts_with(&root),
            "{}",
            model.target.display()
        );
        let refused = SettingsModel::open_with_library(&parent, false, None)
            .err()
            .unwrap();
        assert!(refused.contains("UNEMUPC_MACHINES_DIR"), "{refused}");
    }
}
