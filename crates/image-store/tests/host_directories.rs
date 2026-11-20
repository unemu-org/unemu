// SPDX-License-Identifier: AGPL-3.0-or-later
//! Upgrade probes use child processes with temporary HOME/XDG roots so parallel
//! tests never change each other's environment; core-contract §12.13 Q5.

use image_store::location::{self, frontend, preferences};
use std::{fs, path::Path, path::PathBuf, process::Command};

fn probe(name: &str) {
    let root = std::env::temp_dir().join(format!("host-directories-{name}-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "host_directory_child", "--nocapture"])
        .env("UNEMU_DIRECTORY_PROBE", name)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("APPDATA", root.join("config"))
        .env("LOCALAPPDATA", root.join("data"))
        .output()
        .unwrap();
    fs::remove_dir_all(&root).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn preferences_read_the_old_place_once() {
    probe("preferences");
}
#[test]
fn machine_files_stay_at_the_old_place() {
    probe("machines");
}
#[test]
fn diagnostics_stay_at_the_old_place() {
    probe("diagnostics");
}
#[test]
fn saves_stay_at_the_old_place() {
    probe("saves");
}
#[test]
fn states_stay_at_the_old_place() {
    probe("states");
}
#[test]
fn image_store_already_uses_unemu() {
    probe("store");
}
#[test]
fn library_index_caches_already_use_unemu() {
    probe("indexes");
}
#[test]
fn workspaces_already_use_unemu() {
    probe("workspaces");
}
#[test]
fn preferences_keep_the_original_upgrade_path() {
    probe("original-preferences");
}
#[cfg(unix)]
#[test]
fn external_links_are_never_traversed_or_copied() {
    probe("links");
}

#[cfg(unix)]
#[test]
fn private_directory_permissions_stay_private() {
    probe("permissions");
}

#[test]
fn abandoned_tree_staging_is_neither_published_nor_deleted() {
    probe("abandoned-tree");
}

#[test]
fn concurrent_starts_select_the_same_tree_and_complete_preferences() {
    probe("concurrent");
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn config_root() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME").unwrap()).join("Library/Application Support")
    } else {
        PathBuf::from(std::env::var_os("APPDATA").unwrap())
    }
}

#[test]
fn host_directory_child() {
    let Some(name) = std::env::var_os("UNEMU_DIRECTORY_PROBE") else {
        return;
    };
    let data = location::data_directory().unwrap();
    match name.to_str().unwrap() {
        "preferences" | "original-preferences" => {
            let config = config_root();
            let original = config
                .join(preferences::FORMER_DIRECTORY)
                .join("preferences.toml");
            let old = config.join("unemupc/preferences.toml");
            let new = config.join("unemu/preferences.toml");
            write(&original, b"reduced_motion = false\n");
            let source = if name == "preferences" {
                write(&old, b"reduced_motion = true\nrom_dirs = ['roms']\n");
                &old
            } else {
                &original
            };
            let bytes = fs::read(source).unwrap();
            let modified = fs::metadata(source).unwrap().modified().unwrap();
            assert_eq!(preferences::path().unwrap(), new);
            assert_eq!(fs::read(&new).unwrap(), bytes);
            assert_eq!(fs::read(source).unwrap(), bytes);
            assert_eq!(fs::metadata(source).unwrap().modified().unwrap(), modified);
            write(&new, b"reduced_motion = false\n");
            assert_eq!(preferences::path().unwrap(), new);
            assert_eq!(fs::read(&new).unwrap(), b"reduced_motion = false\n");
            assert_eq!(fs::read(source).unwrap(), bytes);
            assert_eq!(fs::read(&original).unwrap(), b"reduced_motion = false\n");
        }
        "store" | "indexes" | "workspaces" => {
            let root = location::default_store_root().unwrap();
            assert_eq!(root, data.join("unemu"));
            let paths: &[&str] = match name.to_str().unwrap() {
                "store" => &["images/index.json", "cache/downloads/input"],
                "indexes" => &[
                    "cache/summaries/index.json",
                    "cache/libraries/library.json",
                    "cache/decoded/index.json",
                ],
                _ => &["workspaces/session/media/disk.upfj"],
            };
            for suffix in paths {
                let path = root.join(suffix);
                write(&path, b"existing");
                write(&data.join("unemupc").join(suffix), b"old");
                assert_eq!(location::default_store_root().unwrap().join(suffix), path);
                assert_eq!(fs::read(&path).unwrap(), b"existing");
                assert_eq!(fs::read(data.join("unemupc").join(suffix)).unwrap(), b"old");
            }
        }
        #[cfg(unix)]
        "links" => {
            let old = data.join("unemupc/machines");
            let outside = data.join("store");
            write(&outside.join("large-image"), b"immutable base");
            fs::create_dir_all(&old).unwrap();
            std::os::unix::fs::symlink(&outside, old.join("external")).unwrap();
            assert_eq!(frontend::directory(&data, "machines").unwrap(), old);
            assert!(!data.join("unemu").exists());
            std::os::unix::fs::symlink("missing", old.join("dangling")).unwrap();
            std::os::unix::fs::symlink(&old, old.join("cycle")).unwrap();
            assert_eq!(frontend::directory(&data, "machines").unwrap(), old);
            assert!(!data.join("unemu").exists());
            assert!(fs::symlink_metadata(old.join("external"))
                .unwrap()
                .is_symlink());
            assert_eq!(
                fs::read(outside.join("large-image")).unwrap(),
                b"immutable base"
            );
        }
        #[cfg(unix)]
        "permissions" => {
            use std::os::unix::fs::PermissionsExt;
            let old = data.join("unemupc/saves");
            write(&old.join("private/record"), b"saved");
            for path in [&old, &old.join("private")] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let selected = frontend::directory(&data, "saves").unwrap();
            for path in [&selected, &selected.join("private")] {
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
            assert_eq!(selected, old);
            assert!(!data.join("unemu").exists());
        }
        "abandoned-tree" => {
            let old = data.join("unemupc/machines");
            write(&old.join("machine.toml"), b"complete original");
            let staging = data.join("unemu/.machines-import");
            write(&staging.join("machine.toml"), b"partial");
            for _ in 0..2 {
                assert_eq!(frontend::directory(&data, "machines").unwrap(), old);
                assert!(!data.join("unemu/machines").exists());
                assert_eq!(
                    fs::read(old.join("machine.toml")).unwrap(),
                    b"complete original"
                );
                assert_eq!(fs::read(staging.join("machine.toml")).unwrap(), b"partial");
            }
        }
        "concurrent" => {
            let config = config_root();
            write(
                &config.join("unemupc/preferences.toml"),
                b"reduced_motion = true\n",
            );
            for name in ["machines", "diagnostics", "saves", "states"] {
                write(&data.join("unemupc").join(name).join("record"), b"original");
            }
            let mut children = Vec::new();
            for _ in 0..2 {
                children.push(
                    Command::new(std::env::current_exe().unwrap())
                        .args(["--exact", "host_directory_child", "--nocapture"])
                        .env("UNEMU_DIRECTORY_PROBE", "concurrent-worker")
                        .spawn()
                        .unwrap(),
                );
            }
            for mut child in children {
                assert!(child.wait().unwrap().success());
            }
            assert_eq!(
                fs::read(config.join("unemu/preferences.toml")).unwrap(),
                b"reduced_motion = true\n"
            );
            for name in ["machines", "diagnostics", "saves", "states"] {
                assert!(!data.join("unemu").join(name).exists());
                assert_eq!(
                    fs::read(data.join("unemupc").join(name).join("record")).unwrap(),
                    b"original"
                );
            }
        }
        "concurrent-worker" => {
            assert_eq!(
                fs::read(preferences::path().unwrap()).unwrap(),
                b"reduced_motion = true\n"
            );
            for name in ["machines", "diagnostics", "saves", "states"] {
                assert_eq!(
                    frontend::directory(&data, name).unwrap(),
                    data.join("unemupc").join(name)
                );
            }
        }
        name => {
            let old = data.join("unemupc").join(name);
            let new = data.join("unemu").join(name);
            assert_eq!(frontend::directory(&data, name).unwrap(), new);
            assert!(!new.exists(), "a fresh install creates nothing yet");
            write(&old.join("nested/record"), b"original");
            let modified = fs::metadata(old.join("nested/record"))
                .unwrap()
                .modified()
                .unwrap();
            for _ in 0..2 {
                assert_eq!(frontend::directory(&data, name).unwrap(), old);
                assert!(!new.exists());
                assert_eq!(
                    fs::metadata(old.join("nested/record"))
                        .unwrap()
                        .modified()
                        .unwrap(),
                    modified
                );
            }
            fs::create_dir_all(&new).unwrap();
            assert_eq!(
                frontend::directory(&data, name).unwrap(),
                old,
                "an empty new tree must not hide old data"
            );
            write(&new.join("record"), b"new data");
            assert_eq!(
                frontend::directory(&data, name).unwrap(),
                old,
                "keep a single tree until the user moves it"
            );
            fs::rename(&old, old.with_extension("moved")).unwrap();
            assert_eq!(frontend::directory(&data, name).unwrap(), new);
            assert_eq!(fs::read(new.join("record")).unwrap(), b"new data");
        }
    }
}
