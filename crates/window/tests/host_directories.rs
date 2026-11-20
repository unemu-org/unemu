// SPDX-License-Identifier: AGPL-3.0-or-later
//! Installed-desktop machine-library discovery without opening a window;
//! core-contract §12.13 Q5. Help stops before GUI initialization.

use std::{fs, path::PathBuf, process::Command};

#[test]
fn desktop_keeps_the_legacy_machine_tree_until_it_is_moved() {
    let temp = assembly::test_support::TempDir::new("desktop-directory-upgrade");
    let home = temp.path().join("home");
    let data = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        temp.path().join("data")
    };
    let old = data.join("unemupc/machines");
    let new = data.join("unemu/machines");
    fs::create_dir_all(old.join("local")).unwrap();
    fs::write(old.join("machine.toml"), "extends='local/base.toml'\n").unwrap();
    fs::write(old.join("local/base.toml"), "schema=1\n").unwrap();
    fs::create_dir_all(old.join("machine.toml.local/media")).unwrap();
    fs::write(old.join("machine.toml.local/media/disk.upfj"), b"journal").unwrap();
    let run = |root: Option<PathBuf>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_unemu"));
        command
            .arg("--help")
            .env("HOME", &home)
            .env("XDG_DATA_HOME", temp.path().join("data"))
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("LOCALAPPDATA", temp.path().join("data"))
            .env_remove("CARGO_MANIFEST_DIR")
            .env_remove("UNEMUPC_MACHINES_DIR");
        if let Some(root) = &root {
            command.env("UNEMUPC_MACHINES_DIR", root);
        }
        let output = command.output().unwrap();
        assert!(output.status.success());
        let selected = root.unwrap_or_else(|| {
            if old.exists() {
                old.clone()
            } else {
                new.clone()
            }
        });
        assert!(String::from_utf8_lossy(&output.stdout)
            .contains(&format!("machine files: {}", selected.display())));
    };
    run(Some(temp.path().join("explicit")));
    assert!(
        !new.exists(),
        "an explicit root does not import the default"
    );
    for _ in 0..2 {
        run(None);
        assert!(!new.exists());
        assert_eq!(
            fs::read(old.join("machine.toml.local/media/disk.upfj")).unwrap(),
            b"journal"
        );
    }
    fs::create_dir_all(&new).unwrap();
    run(None);
    assert_eq!(fs::read_dir(&new).unwrap().count(), 0);
    fs::rename(&old, data.join("machines-moved")).unwrap();
    run(None);
}
