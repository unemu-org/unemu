// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in image command contracts; machine-images §8.

use std::{
    ffi::OsString,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

struct RemoveDir(PathBuf);
impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn run(args: &[OsString]) -> (u8, String, String) {
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run(args, &mut output, &mut errors);
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
        vec!["image", "sweep"],
        vec!["image", "sweep", "--store", "store", "--tag", "test:a"],
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
    // The shipped machine file names the fetched open BIOS and FreeDOS floppy.
    if fixtures::fixture("kiselev-bios/bios-xi8088.bin").is_none()
        || fixtures::fixture("freedos/freedos.img").is_none()
    {
        return;
    }
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
        "--no-compress".into(),
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
    assert!(listed.contains("stored:") && listed.contains("decoded:"));
    let compress = [
        "image".into(),
        "compress".into(),
        "test:cli".into(),
        "--tag".into(),
        "test:compressed".into(),
        "--store".into(),
        root.clone().into_os_string(),
    ];
    let (code, output, errors) = run(&compress);
    assert_eq!(code, 0, "{errors}");
    assert!(output.starts_with("test:compressed\tsha256:"));
    let store = image_store::Store::open_existing(&root).unwrap();
    let raw = store.resolve("test:cli", &pc_image::PcProvider).unwrap();
    let compressed = store
        .resolve("test:compressed", &pc_image::PcProvider)
        .unwrap();
    assert_eq!(
        raw.manifest.config.digest,
        compressed.manifest.config.digest
    );
    assert_ne!(raw.image.digest, compressed.image.digest);
    assert!(compressed.manifest.layers.iter().any(|d| d.is_compressed()));
    assert_ne!(run(&compress).0, 0);
    let store_args = |action: &str| -> Vec<OsString> {
        vec![
            "image".into(),
            action.into(),
            "--store".into(),
            root.clone().into_os_string(),
        ]
    };
    // Compressing and resolving verify by streaming: nothing is decoded to disk.
    let (code, listed, errors) = run(&store_args("ls"));
    assert_eq!(code, 0, "{errors}");
    assert!(listed.ends_with("\nmaterialized:0\n"), "{listed}");
    let (code, swept, errors) = run(&store_args("sweep"));
    assert_eq!(code, 0, "{errors}");
    assert_eq!(swept, "removed:0\tbytes:0\tkept:0\n");
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
    assert!(cli_pc::command::parse(&args).is_ok());
    let mut duplicate = args.to_vec();
    duplicate.extend(["--recover".into(), "cmos=tip".into()]);
    assert!(cli_pc::command::parse(&duplicate).is_err());
}

#[test]
fn workspace_commands_require_explicit_store_session_and_tag() {
    for valid in [
        vec![
            "image",
            "run",
            "test:a",
            "--workspace",
            "play",
            "--store",
            "store",
        ],
        vec!["image", "workspace", "ls", "--store", "store"],
        vec!["image", "workspace", "discard", "play", "--store", "store"],
        vec![
            "image",
            "publish",
            "--from-workspace",
            "play",
            "--tag",
            "test:b",
            "--store",
            "store",
        ],
    ] {
        assert!(
            cli_pc::command::parse(&valid.into_iter().map(OsString::from).collect::<Vec<_>>())
                .is_ok()
        );
    }
    for invalid in [
        vec!["image", "run", "test:a", "--store", "store"],
        vec!["image", "run", "test:a", "--workspace", "play"],
        vec![
            "image",
            "publish",
            "--from-workspace",
            "play",
            "--store",
            "store",
        ],
        vec!["image", "workspace", "discard", "--store", "store"],
        vec![
            "image",
            "workspace",
            "ls",
            "--store",
            "store",
            "--workspace",
            "play",
        ],
        vec![
            "image",
            "run",
            "test:a",
            "--workspace",
            "play",
            "--store",
            "store",
            "--max-steps",
            "0",
        ],
    ] {
        assert!(cli_pc::command::parse(
            &invalid.into_iter().map(OsString::from).collect::<Vec<_>>()
        )
        .is_err());
    }
}

#[test]
fn cli_workspace_run_publish_list_and_discard_are_opt_in() {
    let Some(bios) = fixtures::fixture("kiselev-bios/bios-xi8088.bin") else {
        return;
    };
    let root = std::env::temp_dir().join(format!("workspace-command-test-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let _cleanup = RemoveDir(root.clone());
    std::fs::write(root.join("bios.bin"), bios).unwrap();
    std::fs::write(root.join("disk.img"), vec![0; 360 * 1024]).unwrap();
    let machine = root.join("machine.toml");
    std::fs::write(
        &machine,
        "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\nimage='disk.img'\nwrite='journal'\njournal='source.upfj'\n",
    )
    .unwrap();
    let args = vec![
        "image".into(),
        "import".into(),
        machine.into_os_string(),
        "--store".into(),
        root.clone().into_os_string(),
        "--tag".into(),
        "test:base".into(),
        "--create".into(),
    ];
    let (code, _, errors) = run(&args);
    assert_eq!(code, 0, "{errors}");
    let execute = |args: &[&str]| {
        let mut args = args.iter().map(OsString::from).collect::<Vec<_>>();
        args.extend(["--store".into(), root.clone().into_os_string()]);
        run(&args)
    };
    let (code, output, errors) = execute(&[
        "image",
        "run",
        "test:base",
        "--workspace",
        "play",
        "--max-steps",
        "1",
    ]);
    assert_eq!(code, 0, "{errors}");
    assert!(output.contains("workspace:play"));
    assert!(output.ends_with("\tSteps\n"), "{output}");
    assert!(output.starts_with("pinned-image:sha256:"));
    assert_ne!(
        execute(&[
            "image",
            "run",
            "test:base",
            "--workspace",
            "play",
            "--new",
            "--max-steps",
            "1"
        ])
        .0,
        0
    );
    let local = image_store::Store::open_existing(&root).unwrap();
    let owner = local.open_workspace("play").unwrap();
    let journal_before = std::fs::read(owner.path().join("media/medium-0.upfj")).unwrap();
    assert!(
        pc_image::workspace::publish_with_hook(&owner, "test:aborted", None, |step| {
            if step == pc_image::workspace::PublicationStep::IntentStored {
                Err(pc_image::Error::Invalid("injected interruption".into()))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    drop(owner);
    assert_eq!(
        execute(&["image", "workspace", "abandon-publication", "play"]).0,
        0
    );
    assert_eq!(
        std::fs::read(root.join("workspaces/play/media/medium-0.upfj")).unwrap(),
        journal_before
    );
    let (code, output, errors) = execute(&["image", "workspace", "ls"]);
    assert_eq!(code, 0, "{errors}");
    assert_eq!(output, "play\n");
    let (code, output, errors) = execute(&[
        "image",
        "publish",
        "--from-workspace",
        "play",
        "--tag",
        "test:saved",
    ]);
    assert_eq!(code, 0, "{errors}");
    assert!(output.starts_with("test:saved\tsha256:"));
    assert!(!root.join("workspaces/play/media/medium-0.upfj").exists());
    let (code, _, _) = execute(&[
        "image",
        "publish",
        "--from-workspace",
        "play",
        "--tag",
        "test:saved",
    ]);
    assert_ne!(code, 0);
    let (code, _, errors) = execute(&["image", "workspace", "discard", "play", "--yes"]);
    assert_eq!(code, 0, "{errors}");
    let (code, output, errors) = execute(&["image", "workspace", "ls"]);
    assert_eq!(code, 0, "{errors}");
    assert!(output.is_empty());
    let (code, output, errors) = execute(&["image", "ls"]);
    assert_eq!(code, 0, "{errors}");
    assert!(output.contains("test:base\t"));
    assert!(output.contains("test:saved\t"));
}

#[test]
fn destructive_workspace_commands_require_existing_names_and_explicit_discard() {
    let root = std::env::temp_dir().join(format!("workspace-review-cli-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let _cleanup = RemoveDir(root.clone());
    let store = image_store::Store::open(&root).unwrap();
    let owner = store.workspace("kept").unwrap();
    owner.write("session.toml", b"schema=1\n").unwrap();
    std::fs::create_dir(owner.path().join("custom")).unwrap();
    std::fs::write(owner.path().join("custom/disk.upfj"), b"synthetic journal").unwrap();
    drop(owner);
    let execute = |args: &[&str]| {
        let mut args = args.iter().map(OsString::from).collect::<Vec<_>>();
        args.extend(["--store".into(), root.clone().into_os_string()]);
        run(&args)
    };
    for args in [
        vec!["image", "workspace", "discard", "typo", "--yes"],
        vec!["image", "workspace", "abandon-publication", "typo"],
        vec![
            "image",
            "publish",
            "--from-workspace",
            "typo",
            "--tag",
            "test:new",
        ],
    ] {
        assert_ne!(execute(&args).0, 0);
        assert!(!root.join("workspaces/typo").exists());
    }
    let (code, output, _) = execute(&["image", "workspace", "discard", "kept"]);
    assert_ne!(code, 0);
    assert!(output.contains("affected:custom/disk.upfj"));
    assert!(root.join("workspaces/kept/custom/disk.upfj").exists());
    assert_eq!(
        execute(&["image", "workspace", "discard", "kept", "--yes"]).0,
        0
    );
    assert!(!root.join("workspaces/kept/session.toml").exists());
    for args in [
        vec![
            "image",
            "run",
            "test:a",
            "--store",
            "store",
            "--workspace",
            "new",
            "--new",
        ],
        vec![
            "image",
            "publish",
            "--store",
            "store",
            "--from-workspace",
            "new",
            "--tag",
            "test:b",
            "--flatten",
        ],
        vec![
            "image",
            "workspace",
            "abandon-publication",
            "new",
            "--store",
            "store",
        ],
    ] {
        assert!(
            cli_pc::command::parse(&args.into_iter().map(OsString::from).collect::<Vec<_>>())
                .is_ok()
        );
    }
}

#[test]
#[ignore = "slow: compare stored FreeDOS runs and journal bytes on both start paths"]
fn stored_freedos_runs_keep_the_old_workspace_and_journal_bytes() {
    use cpu::Cpu;
    use std::{collections::BTreeMap, path::Path};

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(root, &path, files);
                } else {
                    files.insert(
                        path.strip_prefix(root).unwrap().to_owned(),
                        std::fs::read(path).unwrap(),
                    );
                }
            }
        }
        let mut files = BTreeMap::new();
        visit(root, root, &mut files);
        files
    }

    if fixtures::fixture("kiselev-bios/bios-xi8088.bin").is_none()
        || fixtures::fixture("freedos/freedos.img").is_none()
    {
        return;
    }
    let root =
        std::env::temp_dir().join(format!("stored-freedos-comparison-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let _cleanup = RemoveDir(root.clone());
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let profile = std::fs::read_to_string(repo.join("machines/at-freedos.toml")).unwrap();
    let file = root.join("machine.toml");
    std::fs::write(
        &file,
        profile.replace("../vendor/", &format!("{}/", repo.join("vendor").display())),
    )
    .unwrap();
    let store = image_store::Store::open(root.join("store")).unwrap();
    let profile = std::fs::read_to_string(&file).unwrap().replace(
        "write = \"volatile\"",
        "write = \"journal\"\njournal = \"source.upfj\"",
    );
    std::fs::write(&file, profile).unwrap();
    pc_image::import_machine(
        &store,
        &file,
        "test:dos",
        &pc_image::ImportOptions {
            no_compress: true,
            ..Default::default()
        },
    )
    .unwrap();

    let id = pc_image::machine::create_from_tag(&store, "test:dos", Some("DOS"), None).unwrap();
    let cap = 10_000_000;
    for round in 0..2 {
        let (retired, time, clock, image) = {
            let owner = store.workspace("legacy").unwrap();
            let prepared = pc_image::workspace::prepare_start(&owner, "test:dos", false).unwrap();
            let mut machine = assembly::assemble_with_verified_bases(
                &prepared.machine,
                &assembly::ObservationPolicy::default(),
                assembly::AssemblyOptions::default(),
                &prepared.bases,
            )
            .unwrap();
            assert_eq!(
                machine
                    .run_session(&assembly::RunPolicy::natural(Some(cap), Some(1_000_000)))
                    .unwrap(),
                machine::RunOutcome::RetiredCapReached
            );
            pc_harness::finish_persistence(&mut machine, &mut Vec::new()).unwrap();
            (
                machine.machine.cpu().retired(),
                machine.machine.virtual_time(),
                machine.machine.clock().0,
                prepared.image,
            )
        };
        let (code, output, errors) = run(&[
            "image".into(),
            "run".into(),
            "test:dos".into(),
            "--workspace".into(),
            "contract".into(),
            "--max-steps".into(),
            cap.to_string().into(),
            "--store".into(),
            store.root().into(),
        ]);
        assert_eq!(code, 0, "{errors}");
        assert!(errors.is_empty(), "{errors}");
        assert_eq!(
            output,
            format!("pinned-image:{image}\nworkspace:contract\tSteps\n")
        );
        assert_eq!(
            snapshot(&store.workspace_path("legacy").unwrap()),
            snapshot(&store.workspace_path("contract").unwrap())
        );
        {
            use unemu_core_api::Machine;
            let owner = store.workspace("counters").unwrap();
            let prepared = pc_image::launch::prepare_workspace(
                &owner,
                "test:dos",
                false,
                &unemu_pc_core::PcCore,
                "DOS".into(),
            )
            .unwrap();
            let mut machine = core_launch::start_with(&prepared.prepared, |request| {
                unemu_pc_core::PcCore.create_machine(request)
            })
            .unwrap();
            assert_eq!(
                machine.run(unemu_core_api::RunLimit::steps(cap)).unwrap(),
                unemu_core_api::Stop::Steps
            );
            machine.finish().unwrap();
            let assembled = machine.assembled_ref();
            assert_eq!(
                (
                    assembled.machine.cpu().retired(),
                    machine.steps(),
                    machine.now().0
                ),
                (retired, time, clock)
            );
        }
        let (code, output, errors) = run(&[
            "image".into(),
            "machine".into(),
            "run".into(),
            id.clone().into(),
            "--max-steps".into(),
            cap.to_string().into(),
            "--store".into(),
            store.root().into(),
        ]);
        assert_eq!(code, 0, "{errors}");
        assert_eq!(
            errors,
            if round == 0 {
                "migrated:contract\nmigrated:counters\nmigrated:legacy\n"
            } else {
                ""
            }
        );
        assert_eq!(
            output,
            format!("pinned-image:{image}\nmachine:{id}\tSteps\n")
        );
        assert_eq!(
            snapshot(&store.workspace_path("legacy").unwrap().join("media")),
            snapshot(&store.workspace_path(&id).unwrap().join("media"))
        );
        assert_eq!(time, cap);
        assert!(retired > 0);
        eprintln!("FreeDOS counters: retired={retired}, steps={time}, clock={clock}");
    }
}
