// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-default isolation uses a child with temporary data/cache roots.
//! Process probes stay separate from lock-holding tests; machine-images §5.3.

use std::{ffi::OsString, fs, path::PathBuf, process::Command};

#[test]
fn firmware_library_sources_do_not_use_host_defaults() {
    let root = std::env::temp_dir().join(format!("firmware-host-guard-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("probe.bin"), vec![0xa5; 32768]).unwrap();
    // The persistent index excludes freshly written files until their metadata
    // is outside its race window. Age this input before starting the probe.
    std::thread::sleep(std::time::Duration::from_millis(2200));
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "firmware_host_guard_child", "--nocapture"])
        .env("UNEMU_TEST_FIRMWARE_GUARD", &root)
        .env("HOME", root.join("home"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("LOCALAPPDATA", root.join("local"))
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
fn firmware_host_guard_child() {
    let Some(root) = std::env::var_os("UNEMU_TEST_FIRMWARE_GUARD") else {
        return;
    };
    let root = PathBuf::from(root);
    let default_root = image_store::location::default_store_root().unwrap();
    let index = assembly::firmware_discovery::firmware_cache_path();
    let store = image_store::Store::open(&default_root).unwrap();
    let bytes = vec![0xf4; 32768];
    let rom = root.join("store-source.bin");
    fs::write(&rom, &bytes).unwrap();
    let digest = image_store::Digest::of(&bytes);
    let config = image_store::firmware::Config::decode(format!("schema=1\nplatform='ibm-pc-at'\nname='Synthetic guard'\n[[roms]]\nid='system-bios'\nrole='system-bios'\nsha256='{}'\nsize=32768\nblob='{}'\n",digest.hex(),digest).as_bytes()).unwrap();
    let candidate = pc_image::firmware_import::Candidate {
        catalogue_id: "guard".into(),
        config,
        sources: vec![rom],
        distribution: assembly::FirmwareDistribution::UserSupplied,
    };
    pc_image::firmware_import::publish(&store, &candidate, &[], pc_image::firmware::catalogue())
        .unwrap();
    let machine = root.join("machine.toml");
    fs::write(&machine, format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='absent.bin'\nsha256='{}'\n[[card]]\nkind='mda'\n",digest.hex())).unwrap();
    let prepared =
        machine_file::prepare_for_assembly_with_firmware_roots(&machine, &[], &mut Vec::new())
            .unwrap();
    assert!(prepared.firmware_sources().is_empty());
    assert!(machine_file::load_for_assembly(
        &machine,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true
        }
    )
    .is_err());
    assert!(!default_root.join("cache/summaries/index.json").exists());
    let run_args = vec![
        OsString::from("run"),
        "--machine".into(),
        machine.into_os_string(),
        "--allow-unlisted-firmware".into(),
        "--max-steps".into(),
        "1".into(),
    ];
    let code = cli::command::run(&run_args, &mut Vec::new(), &mut Vec::new());
    assert_eq!(code, 1);
    let args = vec![
        OsString::from("image"),
        "firmware".into(),
        "import".into(),
        root.join("probe.bin").into_os_string(),
        "--store".into(),
        root.join("unused-store").into_os_string(),
        "--create".into(),
    ];
    assert_eq!(
        cli::command::run(&args, &mut Vec::new(), &mut Vec::new()),
        0
    );
    // Without --store, the library runner has no default store to fall back to.
    let listing = ["image", "firmware", "ls"].map(OsString::from);
    let mut errors = Vec::new();
    assert_ne!(cli::command::run(&listing, &mut Vec::new(), &mut errors), 0);
    assert!(String::from_utf8_lossy(&errors).contains("default store unavailable"));
    if let Some(index) = index {
        assert!(
            !index.exists(),
            "library command must not create the host ROM index"
        );
    }
    assert!(!default_root.join("cache/summaries/index.json").exists());
    let supplied = cli::command::FirmwareSources {
        store_root: Some(default_root.clone()),
        index: None,
    };
    let mut listed = Vec::new();
    assert_eq!(
        cli::command::run_with_firmware_sources(&listing, &mut listed, &mut Vec::new(), &supplied),
        0
    );
    assert!(String::from_utf8_lossy(&listed).contains("firmware/guard:1"));
    assert_eq!(
        cli::command::run_with_firmware_sources(
            &run_args,
            &mut Vec::new(),
            &mut Vec::new(),
            &supplied
        ),
        0
    );
    let mut disabled = run_args.clone();
    disabled.push("--no-store".into());
    assert_eq!(
        cli::command::run_with_firmware_sources(
            &disabled,
            &mut Vec::new(),
            &mut Vec::new(),
            &supplied
        ),
        1
    );
    let mut explicit = run_args;
    explicit.extend(["--store".into(), default_root.into_os_string()]);
    assert_eq!(
        cli::command::run_with_firmware_sources(
            &explicit,
            &mut Vec::new(),
            &mut Vec::new(),
            &Default::default()
        ),
        0
    );
}
