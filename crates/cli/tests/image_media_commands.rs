// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic installation-set CLI, closure and summary contracts; machine-images §5.2.
use image_store::{
    media_set::{Config, Validator},
    summary::{ArtifactKind, Facets, SummaryCache},
    Store,
};
use pc_image::media_set::PcMediaFormats;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "media-set-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn run(root: &Path, args: &[&str]) -> (u8, String, String) {
    let mut args: Vec<OsString> = ["image", "media"]
        .into_iter()
        .chain(args.iter().copied())
        .map(Into::into)
        .collect();
    args.extend(["--store".into(), root.join("store").into_os_string()]);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = cli::command::run(&args, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}
fn disk(path: &Path) -> Vec<u8> {
    use cli::create::{CreateCommand, FloppyFormat, ImageKind};
    let args = vec![
        "floppy".into(),
        "--type".into(),
        "360K".into(),
        "--format".into(),
        path.into(),
    ];
    let command: CreateCommand = cli::create::parse(&args).unwrap();
    assert!(matches!(
        command.kind,
        ImageKind::Floppy {
            format: FloppyFormat::Dd360,
            ..
        }
    ));
    cli::create::execute(command, &mut Vec::new()).unwrap();
    fs::read(path).unwrap()
}
#[test]
fn media_set_cli_directory_list_extras_sources_and_tag_precheck() {
    let t = Temp::new();
    let p = &t.0;
    fs::create_dir(p.join("disks")).unwrap();
    let a = disk(&p.join("disks/a.img"));
    let b = disk(&p.join("disks/b.img"));
    fs::write(p.join("manual.txt"), b"Open synthetic manual").unwrap();
    let directory = p.join("disks");
    let extra = p.join("manual.txt:MANUAL.TXT");
    let import = [
        "import",
        directory.to_str().unwrap(),
        "--title",
        "Synthetic set",
        "--tag",
        "test:dir",
        "--label",
        "1=Setup",
        "--label",
        "2=Setup",
        "--extra",
        extra.to_str().unwrap(),
        "--create",
    ]
    .map(str::to_owned);
    let import = import.iter().map(String::as_str).collect::<Vec<_>>();
    let (code, _, err) = run(p, &import);
    assert_eq!(code, 0, "{err}");
    let store = Store::open_existing(p.join("store")).unwrap();
    let image = store
        .resolve("test:dir", &Validator(&PcMediaFormats))
        .unwrap();
    let config = Config::decode(&image.config).unwrap();
    assert_eq!(config.media.len(), 2);
    assert_eq!(config.media[0].label, "Setup");
    assert_eq!(config.extras[0].dos_name.as_deref(), Some("MANUAL.TXT"));
    assert_eq!(config.extras[0].size, 21);
    assert_eq!(fs::read(p.join("disks/a.img")).unwrap(), a);
    assert_eq!(fs::read(p.join("disks/b.img")).unwrap(), b);
    assert_eq!(
        fs::read(p.join("manual.txt")).unwrap(),
        b"Open synthetic manual"
    );
    let (code, out, err) = run(p, &["ls"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("test:dir"));
    let (code, out, err) = run(p, &["show", "test:dir"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("MANUAL.TXT"));
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 64, &[], &[]).unwrap();
    assert_eq!(
        cache.summary(&image.image.digest).unwrap().kind,
        ArtifactKind::MediaSet
    );
    assert_eq!(cache.query("synthetic", &Facets::default()).len(), 1);
    fs::remove_dir_all(p.join("disks")).unwrap();
    let (code, _, err) = run(p, &import);
    assert_ne!(code, 0);
    assert!(err.contains("tag conflict"), "{err}");
}
#[test]
fn media_set_cli_file_order_determinism_and_fail_closed_import() {
    let t = Temp::new();
    let p = &t.0;
    let a = disk(&p.join("a.img"));
    let mut b = fs::read(p.join("a.img")).unwrap();
    b[100] = 3;
    fs::write(p.join("b.img"), &b).unwrap();
    let a_path = p.join("a.img");
    let b_path = p.join("b.img");
    let import = |tag| {
        run(
            p,
            &[
                "import",
                b_path.to_str().unwrap(),
                a_path.to_str().unwrap(),
                "--title",
                "Ordered set",
                "--tag",
                tag,
                "--create",
            ],
        )
    };
    let (code, _, err) = import("test:first");
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = import("test:second");
    assert_eq!(code, 0, "{err}");
    let store = Store::open_existing(p.join("store")).unwrap();
    let first = store
        .resolve("test:first", &Validator(&PcMediaFormats))
        .unwrap();
    assert_eq!(
        first.image.digest,
        store
            .resolve("test:second", &Validator(&PcMediaFormats))
            .unwrap()
            .image
            .digest
    );
    let config = Config::decode(&first.config).unwrap();
    assert_eq!(config.media[0].label, "b.img");
    assert_eq!(config.media[1].label, "a.img");
    assert_eq!(fs::read(b_path).unwrap(), b);
    assert_eq!(fs::read(a_path).unwrap(), a);
    fs::write(p.join("odd.img"), [0; 4096]).unwrap();
    let odd = p.join("odd.img");
    let args = [
        "import",
        odd.to_str().unwrap(),
        "--title",
        "Unknown",
        "--tag",
        "test:unknown",
    ];
    let (code, _, err) = run(p, &args);
    assert_ne!(code, 0);
    assert!(err.contains("unknown media size"), "{err}");
    for (format, success) in [("iso-2048", true), ("1.44M", false), ("unknown", false)] {
        let tag = format!("test:{format}");
        let (code, _, err) = run(
            p,
            &[
                "import",
                odd.to_str().unwrap(),
                "--title",
                "Optical",
                "--tag",
                &tag,
                "--format",
                &format!("1={format}"),
            ],
        );
        assert_eq!(code == 0, success, "{err}");
    }
}

#[test]
fn media_set_closure_refuses_unknown_format_size_mismatch_and_extra_descriptors() {
    use image_store::{media_type as mt, Descriptor, Manifest};
    let t = Temp::new();
    let p = &t.0;
    let disk_path = p.join("disk.img");
    disk(&disk_path);
    let (code, _, err) = run(
        p,
        &[
            "import",
            disk_path.to_str().unwrap(),
            "--title",
            "Closure test",
            "--tag",
            "test:valid",
            "--create",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let store = Store::open_existing(p.join("store")).unwrap();
    let resolved = store
        .resolve("test:valid", &Validator(&PcMediaFormats))
        .unwrap();
    let config = Config::decode(&resolved.config).unwrap();
    for variant in ["unknown", "size", "missing", "extra", "order", "platform"] {
        let mut config = config.clone();
        let mut layers = resolved.manifest.layers.clone();
        match variant {
            "unknown" => {
                config.media[0]
                    .format
                    .insert("unsupported".into(), true.into());
            }
            "size" => {
                config.media[0].format = pc_image::media_set::format("720K").unwrap();
            }
            "missing" => {
                layers.remove(0);
            }
            "extra" => {
                let b = store.import(b"unreferenced".as_slice(), None).unwrap();
                layers.insert(0, Descriptor::new(mt::EXTRA, b.digest, b.size));
            }
            "order" => layers.reverse(),
            "platform" => config.platform = "other-platform".into(),
            _ => unreachable!(),
        }
        let b = store
            .import(config.encode().unwrap().as_slice(), None)
            .unwrap();
        let manifest = Manifest::new_media_set(
            &config.platform,
            Descriptor::new(mt::MEDIA_SET_CONFIG, b.digest, b.size),
            layers,
        )
        .unwrap();
        let descriptor = store.import_manifest(&manifest).unwrap();
        assert!(
            store
                .update_tag(
                    &format!("test:{variant}"),
                    &descriptor,
                    None,
                    &Validator(&PcMediaFormats)
                )
                .is_err(),
            "{variant}"
        );
    }
    assert!(store.resolve("test:valid", &pc_image::PcProvider).is_err());
    let text = std::str::from_utf8(&resolved.config).unwrap();
    let medium = &text[text.find("[[media]]").unwrap()..];
    assert!(Config::decode(format!("{text}\n{medium}").as_bytes()).is_err());
}

#[test]
fn review_directory_preview_dry_run_and_preflight() {
    let t = Temp::new();
    let p = &t.0;
    let directory = p.join("disks");
    fs::create_dir(&directory).unwrap();
    let a = disk(&directory.join("a.img"));
    let mut b = a.clone();
    b[100] = 9;
    fs::write(directory.join("b.ima"), &b).unwrap();
    fs::write(directory.join(".hidden"), b"hidden").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("a.img", directory.join("c.img")).unwrap();
    let args = [
        "import",
        directory.to_str().unwrap(),
        "--title",
        "Distinct disks",
        "--tag",
        "review:set",
        "--create",
        "--dry-run",
    ];
    let (code, preview, error) = run(p, &args);
    assert_eq!(code, 0, "{error}");
    assert!(preview.contains(".hidden"));
    assert!(preview.find("a.img").unwrap() < preview.find("b.ima").unwrap());
    assert!(!p.join("store").exists());
    let (code, out, error) = run(p, &args[..args.len() - 1]);
    assert_eq!(code, 0, "{error}");
    assert!(out.find("media order:").unwrap() < out.find("review:set\tsha256:").unwrap());
    let store = Store::open_existing(p.join("store")).unwrap();
    let image = store
        .resolve("review:set", &Validator(&PcMediaFormats))
        .unwrap();
    let config = Config::decode(&image.config).unwrap();
    assert_eq!(config.media[0].label, "a.img");
    assert_eq!(config.media[1].label, "b.ima");
    assert_ne!(config.media[0].blob, config.media[1].blob);
    #[cfg(unix)]
    assert_eq!(config.media[0].blob, config.media[2].blob);
    assert_eq!(fs::read(directory.join("a.img")).unwrap(), a);
    assert_eq!(fs::read(directory.join("b.ima")).unwrap(), b);
    fs::create_dir(directory.join("nested")).unwrap();
    let mut invalid = args.to_vec();
    invalid[5] = "review:other";
    let (code, _, error) = run(p, &invalid);
    assert_ne!(code, 0);
    assert!(error.contains("nested"), "{error}");
    fs::remove_dir(directory.join("nested")).unwrap();
    fs::write(directory.join("notes.txt"), b"not a disk").unwrap();
    let (code, _, error) = run(p, &invalid);
    assert_ne!(code, 0);
    assert!(
        error.contains("notes.txt") && error.contains("explicit file list"),
        "{error}"
    );
}
#[test]
fn review_bad_labels_and_guest_names_leave_no_blobs() {
    let t = Temp::new();
    let p = &t.0;
    let source = p.join("disk.img");
    disk(&source);
    let extra = p.join("readme.txt");
    fs::write(&extra, b"manual").unwrap();
    for option in [
        ["--label".to_owned(), "1=bad\nlabel".into()],
        ["--extra".into(), format!("{}:readme.txt", extra.display())],
    ] {
        let (code, _, error) = run(
            p,
            &[
                "import",
                source.to_str().unwrap(),
                "--title",
                "Preflight",
                "--tag",
                "review:bad",
                "--create",
                &option[0],
                &option[1],
            ],
        );
        assert_ne!(code, 0);
        assert!(
            error.contains("disk.img") || error.contains("readme.txt"),
            "{error}"
        );
        assert_eq!(
            fs::read_dir(p.join("store/images/blobs/sha256"))
                .unwrap()
                .count(),
            0
        );
    }
}
#[test]
fn review_schema_two_summary_is_discarded_and_rebuilt() {
    let t = Temp::new();
    let p = &t.0;
    let source = p.join("disk.img");
    disk(&source);
    let (code, _, error) = run(
        p,
        &[
            "import",
            source.to_str().unwrap(),
            "--title",
            "Cache rebuild",
            "--tag",
            "review:cache",
            "--create",
        ],
    );
    assert_eq!(code, 0, "{error}");
    let (code, _, error) = run(p, &["ls"]);
    assert_eq!(code, 0, "{error}");
    let path = p.join("store/cache/summaries/index.json");
    let bytes = fs::read_to_string(&path).unwrap();
    assert!(bytes.contains("\"schema\":3"));
    fs::write(&path, bytes.replacen("\"schema\":3", "\"schema\":2", 1)).unwrap();
    let store = Store::open_existing(p.join("store")).unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    assert_eq!(cache.pending().len(), 1);
    cache.rebuild(&store, 64, &[], &[]).unwrap();
    assert!(cache.pending().is_empty());
    assert!(fs::read_to_string(path).unwrap().contains("\"schema\":3"));
    let command = vec![
        "image".into(),
        "ls".into(),
        "--store".into(),
        store.root().as_os_str().to_owned(),
    ];
    let (mut listed, mut errors) = (Vec::new(), Vec::new());
    assert_eq!(cli::command::run(&command, &mut listed, &mut errors), 0);
    assert!(String::from_utf8(listed).unwrap().contains("\tmedia-set\n"));
    let tag = &store.list_tags(None, 1000).unwrap().tags[0];
    fs::remove_file(
        store
            .blob_file_path(&tag.manifest.digest, tag.manifest.size)
            .unwrap(),
    )
    .unwrap();
    let mut listed = Vec::new();
    assert_eq!(cli::command::run(&command, &mut listed, &mut errors), 0);
    assert!(String::from_utf8(listed).unwrap().contains("\tunknown\n"));
}
