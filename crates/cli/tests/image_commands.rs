// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in image command contracts; machine-images §8.

use std::{
    ffi::OsString,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct RemoveDir(PathBuf);
impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn run(args: &[OsString]) -> (u8, String, String) {
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli::command::run(args, &mut output, &mut errors);
    (
        code,
        String::from_utf8(output).unwrap(),
        String::from_utf8(errors).unwrap(),
    )
}
#[test]
fn image_commands_require_store_tag_and_valid_recovery_choice() {
    for args in [
        vec!["image"],
        vec!["image", "ls"],
        vec!["image", "import", "machine.toml", "--store", "store"],
        vec![
            "image",
            "import",
            "machine.toml",
            "--store",
            "store",
            "--tag",
            "test:a",
            "--recover",
            "automatic",
        ],
        vec!["image", "ls", "--store", "store", "--store", "other"],
        vec![
            "image",
            "ls",
            "--store",
            "store",
            "--allow-unlisted-firmware",
        ],
    ] {
        let args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
        let (code, output, errors) = run(&args);
        assert_eq!(code, 2);
        assert!(output.is_empty());
        assert!(errors.contains("usage:"));
    }
}
#[test]
fn cli_import_lists_a_tag_and_refuses_to_replace_it() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "image-command-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&root).unwrap();
    let _cleanup = RemoveDir(root.clone());
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml");
    let import = vec![
        "image".into(),
        "import".into(),
        source.into_os_string(),
        "--store".into(),
        root.clone().into_os_string(),
        "--tag".into(),
        "test:cli".into(),
        "--create".into(),
    ];
    let (code, output, errors) = run(&import);
    assert_eq!(code, 0, "{errors}");
    assert!(output.starts_with("test:cli\tsha256:"));
    assert!(output.contains("medium-0\tpolicy:volatile\n"));
    assert!(!output.contains("\tgeneration:0"));
    assert!(errors.is_empty());
    let (code, listed, errors) = run(&[
        "image".into(),
        "ls".into(),
        "--store".into(),
        root.clone().into_os_string(),
    ]);
    assert_eq!(code, 0, "{errors}");
    assert!(listed.starts_with("test:cli\tsha256:"));
    let (code, _, errors) = run(&import);
    assert_ne!(code, 0);
    assert!(errors.contains("tag conflict"));
}

#[test]
fn listing_or_import_without_create_does_not_initialize_a_store() {
    let temp = std::env::temp_dir().join(format!("image-command-absent-{}", std::process::id()));
    assert!(!temp.exists());
    let args = [
        "image".into(),
        "ls".into(),
        "--store".into(),
        temp.clone().into_os_string(),
    ];
    assert_ne!(run(&args).0, 0);
    assert!(!temp.exists());
    let args = [
        "image".into(),
        "import".into(),
        "missing.toml".into(),
        "--store".into(),
        temp.clone().into_os_string(),
        "--tag".into(),
        "test:a".into(),
    ];
    assert_ne!(run(&args).0, 0);
    assert!(!temp.exists());
}
#[test]
fn image_import_parses_named_recovery_and_rom_directories() {
    let args = [
        "image",
        "import",
        "machine.toml",
        "--store",
        "store",
        "--tag",
        "test:a",
        "--rom-dir",
        "roms",
        "--recover",
        "medium-0=generation:1",
        "--recover",
        "cmos=tip",
    ]
    .map(OsString::from);
    assert!(cli::command::parse(&args).is_ok());
    let mut duplicate = args.to_vec();
    duplicate.extend(["--recover".into(), "cmos=tip".into()]);
    assert!(cli::command::parse(&duplicate).is_err());
}
