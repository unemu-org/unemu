// SPDX-License-Identifier: AGPL-3.0-or-later
//! Synthetic installation-set CLI, closure and summary contracts; machine-images §5.2.
use image_store::{
    media_set::{Config, Validator},
    summary::{ArtifactKind, Facets, SummaryCache},
    BlobSource, Store,
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
    let code = cli_pc::command::run(&args, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}
fn disk(path: &Path) -> Vec<u8> {
    use cli_pc::create::{CreateCommand, FloppyFormat, ImageKind};
    let args = vec![
        "floppy".into(),
        "--type".into(),
        "360K".into(),
        "--format".into(),
        path.into(),
    ];
    let command: CreateCommand = cli_pc::create::parse(&args).unwrap();
    assert!(matches!(
        command.kind,
        ImageKind::Floppy {
            format: FloppyFormat::Dd360,
            ..
        }
    ));
    cli_pc::create::execute(command, &mut Vec::new()).unwrap();
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
        "test-dir:r1",
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
        .resolve("test-dir:r1", &Validator(&PcMediaFormats))
        .unwrap();
    let config = Config::decode(&image.config).unwrap();
    assert_eq!(config.media.len(), 2);
    let payload = image
        .manifest
        .layers
        .iter()
        .find(|d| d.names(&config.media[0].blob))
        .unwrap();
    assert!(payload.is_compressed());
    // The push guard reads this mark on the stored descriptor.
    assert_eq!(payload.annotations["unemu.redistribution"], "user-supplied");
    assert_eq!(store.read_verified(payload, a.len() as u64).unwrap(), a);
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
    assert!(out.contains("test-dir:r1"));
    let (code, out, err) = run(p, &["show", "test-dir:r1"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("MANUAL.TXT"));
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 64, &[], &[]).unwrap();
    assert_eq!(
        cache.summary(&image.image.digest).unwrap().kind,
        ArtifactKind::MediaSet
    );
    assert_eq!(cache.query("synthetic", &Facets::default()).len(), 1);
    let summary = cache.summary(&image.image.digest).unwrap();
    assert!(summary.stored_size < summary.decoded_size);
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
    // Two revisions of one repository: the title id is the repository's, so
    // the same files give the same set.
    let (code, _, err) = import("test-order:r1");
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = import("test-order:r2");
    assert_eq!(code, 0, "{err}");
    let store = Store::open_existing(p.join("store")).unwrap();
    let first = store
        .resolve("test-order:r1", &Validator(&PcMediaFormats))
        .unwrap();
    assert_eq!(
        first.image.digest,
        store
            .resolve("test-order:r2", &Validator(&PcMediaFormats))
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
        "test-unknown:r1",
    ];
    let (code, _, err) = run(p, &args);
    assert_ne!(code, 0);
    assert!(err.contains("unknown media size"), "{err}");
    for (format, success) in [("iso-2048", true), ("1.44M", false), ("unknown", false)] {
        let tag = format!("test-{}:r1", format.to_lowercase());
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
            "test-valid:r1",
            "--create",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let store = Store::open_existing(p.join("store")).unwrap();
    let resolved = store
        .resolve("test-valid:r1", &Validator(&PcMediaFormats))
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
                    &format!("test-{variant}:r1"),
                    &descriptor,
                    None,
                    &Validator(&PcMediaFormats)
                )
                .is_err(),
            "{variant}"
        );
    }
    assert!(store
        .resolve("test-valid:r1", &pc_image::PcProvider)
        .is_err());
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
        "review-set:r1",
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
    assert!(out.find("media order:").unwrap() < out.find("review-set:r1\tsha256:").unwrap());
    let store = Store::open_existing(p.join("store")).unwrap();
    let image = store
        .resolve("review-set:r1", &Validator(&PcMediaFormats))
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
    invalid[5] = "review-other:r1";
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
                "review-bad:r1",
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
            "review-cache:r1",
            "--create",
        ],
    );
    assert_eq!(code, 0, "{error}");
    let (code, _, error) = run(p, &["ls"]);
    assert_eq!(code, 0, "{error}");
    let path = p.join("store/cache/summaries/index.json");
    let bytes = fs::read_to_string(&path).unwrap();
    assert!(bytes.contains("\"schema\":8"));
    fs::write(&path, bytes.replacen("\"schema\":8", "\"schema\":2", 1)).unwrap();
    let store = Store::open_existing(p.join("store")).unwrap();
    let mut cache = SummaryCache::open(&store).unwrap();
    assert_eq!(cache.pending().len(), 1);
    cache.rebuild(&store, 64, &[], &[]).unwrap();
    assert!(cache.pending().is_empty());
    assert!(fs::read_to_string(path).unwrap().contains("\"schema\":8"));
    let command = vec![
        "image".into(),
        "ls".into(),
        "--store".into(),
        store.root().as_os_str().to_owned(),
    ];
    let (mut listed, mut errors) = (Vec::new(), Vec::new());
    assert_eq!(cli_pc::command::run(&command, &mut listed, &mut errors), 0);
    assert!(String::from_utf8(listed)
        .unwrap()
        .contains("\tmedia-set\tstored:"));
    let tag = &store.list_tags(None, 1000).unwrap().tags[0];
    fs::remove_file(
        store
            .blob_file_path(&tag.manifest.digest, tag.manifest.size)
            .unwrap(),
    )
    .unwrap();
    let mut listed = Vec::new();
    assert_eq!(cli_pc::command::run(&command, &mut listed, &mut errors), 0);
    assert!(String::from_utf8(listed)
        .unwrap()
        .contains("\tunknown\tstored:"));
}

#[test]
fn compression_command_preserves_media_set_config_for_optical_and_extra_payloads() {
    let temp = Temp::new();
    let path = &temp.0;
    let optical = path.join("disc.iso");
    let extra = path.join("manual.txt");
    fs::write(&optical, vec![0x39; 8192]).unwrap();
    fs::write(&extra, vec![b'x'; 8192]).unwrap();
    let (code, _, errors) = run(
        path,
        &[
            "import",
            optical.to_str().unwrap(),
            "--format",
            "1=iso-2048",
            "--extra",
            extra.to_str().unwrap(),
            "--title",
            "Synthetic optical set",
            "--tag",
            "test-raw:r1",
            "--create",
            "--no-compress",
        ],
    );
    assert_eq!(code, 0, "{errors}");
    let store = Store::open_existing(path.join("store")).unwrap();
    let raw = store
        .resolve("test-raw:r1", &Validator(&PcMediaFormats))
        .unwrap();
    assert!(raw.manifest.layers.iter().all(|d| !d.is_compressed()));
    let command = [
        "image".into(),
        "compress".into(),
        "test-raw:r1".into(),
        "--tag".into(),
        "test-compressed:r1".into(),
        "--store".into(),
        store.root().as_os_str().to_owned(),
    ];
    let mut errors = Vec::new();
    assert_eq!(
        cli_pc::command::run(&command, &mut Vec::new(), &mut errors),
        0,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    fs::remove_file(optical).unwrap();
    fs::remove_file(extra).unwrap();
    let image = store
        .resolve("test-compressed:r1", &Validator(&PcMediaFormats))
        .unwrap();
    assert_eq!(raw.config, image.config);
    assert_eq!(raw.manifest.config.digest, image.manifest.config.digest);
    for d in &image.manifest.layers {
        if matches!(
            d.decoded_media_type(),
            image_store::media_type::OPTICAL | image_store::media_type::EXTRA
        ) {
            assert!(d.is_compressed());
            assert_eq!(store.read_verified(d, 8192).unwrap().len(), 8192);
        } else {
            assert!(!d.is_compressed());
        }
    }
}

/// A synthetic 360K floppy of one repeated byte.
fn raw_disk(path: &Path, byte: u8) -> PathBuf {
    fs::write(path, vec![byte; 368_640]).unwrap();
    path.to_owned()
}
fn blob_count(root: &Path) -> usize {
    fs::read_dir(root.join("store/images/blobs/sha256")).map_or(0, |d| d.count())
}

#[test]
fn an_open_statement_goes_together_and_names_a_licence_reference_and_an_extra() {
    let t = Temp::new();
    let p = &t.0;
    let disk = raw_disk(&p.join("disk.img"), 7);
    let terms = p.join("VENDOR.TXT");
    fs::write(&terms, b"Synthetic vendor terms").unwrap();
    let import = |flags: &[&str]| {
        let mut args = vec![
            "import",
            disk.to_str().unwrap(),
            "--title",
            "Synthetic shareware",
            "--tag",
            "test-open:r1",
            "--create",
            "--extra",
            terms.to_str().unwrap(),
        ];
        args.extend(flags);
        run(p, &args)
    };
    for (flags, reason) in [
        (
            vec!["--open"],
            "--open needs --license LicenseRef-NAME and --license-text EXTRA",
        ),
        (
            vec!["--open", "--license", "LicenseRef-synthetic"],
            "--open needs",
        ),
        (
            vec!["--open", "--license-text", "VENDOR.TXT"],
            "--open needs",
        ),
        (vec!["--license", "LicenseRef-synthetic"], "add --open"),
        (vec!["--license-text", "VENDOR.TXT"], "add --open"),
        (
            vec![
                "--open",
                "--license",
                "LicenseRef-a",
                "--license",
                "LicenseRef-b",
                "--license-text",
                "VENDOR.TXT",
            ],
            "unknown or duplicate media option",
        ),
    ] {
        let (code, _, err) = import(&flags);
        assert_eq!(code, 2, "{flags:?}: {err}");
        assert!(err.contains(reason), "{flags:?}: {err}");
        // The usage printed with the refusal names the statement's flags.
        assert!(
            err.contains("[--open --license LicenseRef-NAME --license-text EXTRA]"),
            "{err}"
        );
    }
    assert!(
        !p.join("store").exists(),
        "a refused statement creates nothing"
    );
    for (license, text, reason) in [
        (
            "not a licence",
            "VENDOR.TXT",
            "unknown SPDX licence identifier",
        ),
        ("MIT", "VENDOR.TXT", "the vendor's own terms"),
        (
            "LicenseRef-synthetic",
            "MANUAL.TXT",
            "not the file name of an extra",
        ),
    ] {
        let (code, _, err) = import(&["--open", "--license", license, "--license-text", text]);
        assert_eq!(code, 1, "{license} {text}: {err}");
        assert!(err.contains(reason), "{license} {text}: {err}");
        assert_eq!(blob_count(p), 0, "a refused statement writes no blobs");
    }
}

#[test]
fn a_set_imported_without_metadata_flags_keeps_its_manifest_bytes() {
    // The same inputs and tag as the shared import's pinned set, so the
    // command line's default title is the shared default: measured before
    // set-level metadata flags existed, and moved once by the rename of the
    // platform id to `ibm-pc`, once by the layers' export file names and once
    // by the title id, which is now the repository's last component, `test`,
    // instead of `title`.
    let t = Temp::new();
    let p = &t.0;
    let disk = raw_disk(&p.join("disk.img"), 0x5a);
    let manual = p.join("MANUAL.TXT");
    fs::write(&manual, b"Synthetic manual").unwrap();
    let (code, out, err) = run(
        p,
        &[
            "import",
            disk.to_str().unwrap(),
            "--title",
            "Pinned set",
            "--tag",
            "test:set-r1",
            "--extra",
            &format!("{}:MANUAL.TXT", manual.display()),
            "--no-compress",
            "--create",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        format!(
            "media order:\n1\t\"disk.img\"\t{disk:?}\t360K\n\
             extra\t\"MANUAL.TXT\"\tSome(\"MANUAL.TXT\")\n\
             test:set-r1\tsha256:fd0d9ac8ee837a9db484568e36c6d27c22e78649062bf34729386ac10ade437d\n"
        )
    );
}

#[test]
fn set_level_metadata_flags_land_in_the_metadata_blob_in_order_and_show_prints_them() {
    use image_store::{license, metadata::Metadata};
    let t = Temp::new();
    let p = &t.0;
    let disk = raw_disk(&p.join("disk.img"), 4);
    let terms = p.join("VENDOR.TXT");
    fs::write(&terms, b"Synthetic vendor terms").unwrap();
    let import = |extra: &[&str]| {
        let mut args = vec![
            "import",
            disk.to_str().unwrap(),
            "--title",
            "Synthetic catalogued set",
            "--tag",
            "test-catalogued:r1",
            "--extra",
            terms.to_str().unwrap(),
            "--publisher",
            "Second Publisher",
            "--developer",
            "Synthetic Studio",
            "--year",
            " 1991 ",
            "--publisher",
            "First Publisher",
            "--genre",
            "platformer",
            "--developer",
            "Another Studio",
            "--platform",
            "DOS",
            "--genre",
            "puzzle",
            "--description",
            "A synthetic set.\nSecond line.",
            "--open",
            "--license",
            "LicenseRef-synthetic-shareware",
            "--license-text",
            "VENDOR.TXT",
            "--create",
        ];
        args.extend(extra);
        run(p, &args)
    };
    let catalogue = "publisher\t\"Second Publisher\"\n\
                     publisher\t\"First Publisher\"\n\
                     developer\t\"Synthetic Studio\"\n\
                     developer\t\"Another Studio\"\n\
                     year\t1991\n\
                     genre\t\"platformer\"\n\
                     genre\t\"puzzle\"\n\
                     platform\t\"DOS\"\n\
                     description\t\"A synthetic set.\\nSecond line.\"\n";
    let (code, preview, err) = import(&["--dry-run"]);
    assert_eq!(code, 0, "{err}");
    assert!(preview.contains(catalogue), "{preview}");
    assert_eq!(blob_count(p), 0, "a dry run writes no blobs");
    let (code, out, err) = import(&[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(catalogue), "{out}");

    let store = Store::open_existing(p.join("store")).unwrap();
    let image = store
        .resolve("test-catalogued:r1", &Validator(&PcMediaFormats))
        .unwrap();
    let metadata = Metadata::decode(&image.metadata, &image.manifest.layers).unwrap();
    let [title] = &metadata.titles[..] else {
        panic!("one title: {metadata:?}");
    };
    assert_eq!(title.title, "Synthetic catalogued set");
    assert_eq!(title.publishers, ["Second Publisher", "First Publisher"]);
    assert_eq!(title.developers, ["Synthetic Studio", "Another Studio"]);
    assert_eq!(title.release_year, Some(1991));
    assert_eq!(title.genres, ["platformer", "puzzle"]);
    assert_eq!(title.platform.as_deref(), Some("DOS"));
    assert_eq!(
        title.description.as_deref(),
        Some("A synthetic set.\nSecond line.")
    );
    // Registry browsers read the same record from the manifest's standard
    // annotations, lists joined in the order given; there is no creation time.
    let oci: Vec<(&str, &str)> = image
        .manifest
        .annotations
        .iter()
        .filter(|(key, _)| key.starts_with("org.opencontainers.image."))
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        oci,
        [
            (
                "org.opencontainers.image.authors",
                "Synthetic Studio, Another Studio"
            ),
            (
                "org.opencontainers.image.description",
                "A synthetic set.\nSecond line."
            ),
            (
                "org.opencontainers.image.licenses",
                "LicenseRef-synthetic-shareware AND CC0-1.0"
            ),
            ("org.opencontainers.image.title", "Synthetic catalogued set"),
            (
                "org.opencontainers.image.vendor",
                "Second Publisher, First Publisher"
            ),
        ]
    );
    // The library's facets read the same record.
    let mut cache = SummaryCache::open(&store).unwrap();
    cache.rebuild(&store, 64, &[], &[]).unwrap();
    let facets = Facets {
        publishers: vec!["First Publisher".into()],
        developers: vec!["Another Studio".into()],
        genres: vec!["puzzle".into()],
        platforms: vec!["DOS".into()],
        years: vec![1991],
        ..Facets::default()
    };
    assert_eq!(cache.query("", &facets).len(), 1);

    let (code, out, err) = run(p, &["show", "test-catalogued:r1"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(catalogue), "{out}");
    // The metadata stays this program's generated content: CC0-1.0 in an open set.
    let metadata_layer = out
        .lines()
        .find(|l| l.starts_with("layer\tmetadata\t"))
        .unwrap();
    assert!(metadata_layer.ends_with("\topen\tCC0-1.0\t-"), "{out}");
    let stored = image
        .manifest
        .layers
        .iter()
        .find(|d| d.decoded_media_type() == image_store::media_type::METADATA)
        .unwrap();
    assert_eq!(license::open_license(stored).unwrap().as_str(), "CC0-1.0");
    assert!(out.ends_with("push\tany configured registry, including unemu/\n"));

    // A set without the flags shows no catalogue lines.
    let (code, _, err) = run(
        p,
        &[
            "import",
            disk.to_str().unwrap(),
            "--title",
            "Synthetic plain set",
            "--tag",
            "test-plain:r1",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = run(p, &["show", "test-plain:r1"]);
    assert_eq!(code, 0, "{err}");
    for key in [
        "publisher",
        "developer",
        "year",
        "genre",
        "platform",
        "description",
    ] {
        assert!(!out.contains(&format!("\n{key}\t")), "{out}");
    }
    // Nor does its manifest carry any standard annotation but the title.
    let plain = store
        .resolve("test-plain:r1", &Validator(&PcMediaFormats))
        .unwrap();
    let oci: Vec<&str> = plain
        .manifest
        .annotations
        .keys()
        .map(String::as_str)
        .filter(|key| key.starts_with("org.opencontainers.image."))
        .collect();
    assert_eq!(oci, ["org.opencontainers.image.title"]);
}

#[test]
fn a_bad_year_an_unknown_platform_or_a_blank_value_is_refused_before_any_blob() {
    let t = Temp::new();
    let p = &t.0;
    let disk = raw_disk(&p.join("disk.img"), 5);
    let import = |flags: &[&str]| {
        let mut args = vec![
            "import",
            disk.to_str().unwrap(),
            "--title",
            "Synthetic refused set",
            "--tag",
            "test-refused:r1",
            "--create",
        ];
        args.extend(flags);
        run(p, &args)
    };
    for (flags, code, reason) in [
        (
            vec!["--year", "19x1"],
            2,
            "--year expects a whole number from 1 to 9999",
        ),
        (
            vec!["--year", "1991", "--year", "1992"],
            2,
            "unknown or duplicate media option",
        ),
        (
            vec!["--platform", "DOS", "--platform", "DOS"],
            2,
            "unknown or duplicate media option",
        ),
        (
            vec!["--description", "One", "--description", "Two"],
            2,
            "unknown or duplicate media option",
        ),
        (vec!["--publisher", "  "], 2, "blank set metadata value"),
        (vec!["--genre", "\t"], 2, "blank set metadata value"),
        (
            vec!["--year", "0"],
            1,
            "release year 0: expected a whole number from 1 to 9999",
        ),
        (
            vec!["--year", "10000"],
            1,
            "release year 10000: expected a whole number from 1 to 9999",
        ),
        (
            vec!["--year", "-1991"],
            1,
            "release year -1991: expected a whole number from 1 to 9999",
        ),
        (
            vec!["--platform", "dos"],
            1,
            "software platform \"dos\": expected one of \"DOS\", \"Windows 3.x\"",
        ),
        (
            vec!["--platform", "Amiga"],
            1,
            "software platform \"Amiga\": expected one of",
        ),
    ] {
        let (status, _, err) = import(&flags);
        assert_eq!(status, code, "{flags:?}: {err}");
        assert!(err.contains(reason), "{flags:?}: {err}");
        assert_eq!(
            blob_count(p),
            0,
            "{flags:?}: a refused import writes no blobs"
        );
    }
    let store = Store::open_existing(p.join("store")).unwrap();
    assert!(store.tag_descriptor("test-refused:r1").is_err());
    // The usage names the set-level flags.
    let (_, _, err) = import(&["--year", "19x1"]);
    assert!(
        err.contains("[--publisher P]... [--developer D]... [--year YYYY] [--genre G]... [--platform PLATFORM] [--description TEXT]"),
        "{err}"
    );
}

#[test]
fn set_labels_version_and_edition_land_in_the_metadata_in_order_and_show_prints_them() {
    use image_store::metadata::Metadata;
    let t = Temp::new();
    let p = &t.0;
    let disk = raw_disk(&p.join("disk.img"), 6);
    let import = |tag: &str, flags: &[&str]| {
        let mut args = vec![
            "import",
            disk.to_str().unwrap(),
            "--title",
            "Synthetic release",
            "--tag",
            tag,
            "--create",
        ];
        args.extend(flags);
        run(p, &args)
    };
    let digest = |output: &str| {
        output
            .lines()
            .last()
            .unwrap()
            .split('\t')
            .nth(1)
            .unwrap()
            .to_owned()
    };
    let flags = [
        "--set-label",
        "zeta",
        "--version",
        " 2.1 ",
        "--set-label",
        "alpha",
        "--edition",
        "Synthetic Edition",
        "--set-label",
        "middle",
    ];
    let lines = "version\t\"2.1\"\n\
                 edition\t\"Synthetic Edition\"\n\
                 set-label\t\"zeta\"\n\
                 set-label\t\"alpha\"\n\
                 set-label\t\"middle\"\n";
    let (code, preview, err) = import(
        "test-release:r1",
        &[&flags[..], &["--dry-run"][..]].concat(),
    );
    assert_eq!(code, 0, "{err}");
    assert!(preview.contains(lines), "{preview}");
    assert_eq!(blob_count(p), 0, "a dry run writes no blobs");
    let (code, out, err) = import("test-release:r1", &flags);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(lines), "{out}");

    // The labels are the set's, in the order given; version and edition are
    // the title record's, trimmed like every set-level value.
    let store = Store::open_existing(p.join("store")).unwrap();
    let image = store
        .resolve("test-release:r1", &Validator(&PcMediaFormats))
        .unwrap();
    let metadata = Metadata::decode(&image.metadata, &image.manifest.layers).unwrap();
    assert_eq!(metadata.labels, ["zeta", "alpha", "middle"]);
    let [title] = &metadata.titles[..] else {
        panic!("one title: {metadata:?}");
    };
    assert_eq!(title.version.as_deref(), Some("2.1"));
    assert_eq!(title.edition.as_deref(), Some("Synthetic Edition"));
    let (code, shown, err) = run(p, &["show", "test-release:r1"]);
    assert_eq!(code, 0, "{err}");
    assert!(shown.contains(lines), "{shown}");

    // The order is part of the set: the same labels in another order are
    // another set digest.
    let mut reordered = flags;
    reordered.swap(1, 5);
    let (code, again, err) = import("test-reordered:r1", &reordered);
    assert_eq!(code, 0, "{err}");
    assert_ne!(digest(&again), digest(&out));

    // A set without the flags shows none of these lines.
    let (code, _, err) = import("test-plain:r1", &[]);
    assert_eq!(code, 0, "{err}");
    let (code, shown, err) = run(p, &["show", "test-plain:r1"]);
    assert_eq!(code, 0, "{err}");
    for key in ["version", "edition", "set-label"] {
        assert!(!shown.contains(&format!("\n{key}\t")), "{shown}");
    }
}

#[test]
fn needs_requirements_and_the_work_land_in_metadata_schema_two_and_show_prints_them() {
    use image_store::{metadata::Metadata, needs};
    let t = Temp::new();
    let p = &t.0;
    let disk = raw_disk(&p.join("disk.img"), 9);
    let import = |tag: &str, flags: &[&str]| {
        let mut args = vec![
            "import",
            disk.to_str().unwrap(),
            "--title",
            "Synthetic work",
            "--tag",
            tag,
            "--create",
        ];
        args.extend(flags);
        run(p, &args)
    };
    let flags = [
        "--need",
        "video=vga",
        "--need",
        "sound=sb,adlib,pc-speaker,none",
        "--need",
        "cpu=286+",
        "--need",
        "memory=640K",
        "--requirements",
        "IBM AT or compatible; 640K; VGA",
        "--work",
        "synthetic-work",
        "--synopsis",
        "A synthetic work.",
        "--work-id",
        "wikidata=Q1",
    ];
    let lines = "requirements\t\"IBM AT or compatible; 640K; VGA\"\n\
                 need\tcpu\t286+\n\
                 need\tmemory\t640K\n\
                 need\tsound\tsb,adlib,pc-speaker,none\n\
                 need\tvideo\tvga\n\
                 work\t\"synthetic-work\"\n\
                 synopsis\t\"A synthetic work.\"\n\
                 work-id\twikidata\t\"Q1\"\n";
    let (code, preview, err) = import(
        "synthetic-work:r1",
        &[&flags[..], &["--dry-run"][..]].concat(),
    );
    assert_eq!(code, 0, "{err}");
    assert!(preview.contains(lines), "{preview}");
    assert_eq!(blob_count(p), 0, "a dry run writes no blobs");
    let (code, out, err) = import("synthetic-work:r1", &flags);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(lines), "{out}");

    let store = Store::open_existing(p.join("store")).unwrap();
    let image = store
        .resolve("synthetic-work:r1", &Validator(&PcMediaFormats))
        .unwrap();
    let metadata = Metadata::decode(&image.metadata, &image.manifest.layers).unwrap();
    assert_eq!(metadata.schema(), 2);
    let [title] = &metadata.titles[..] else {
        panic!("one title: {metadata:?}");
    };
    assert_eq!(title.needs["sound"], ["sb", "adlib", "pc-speaker", "none"]);
    assert_eq!(title.work_id.as_deref(), Some("synthetic-work"));
    assert_eq!(title.work_ids["wikidata"], "Q1");
    assert_eq!(
        image.manifest.annotations[needs::ANNOTATION],
        "cpu=286+;memory=640K;sound=sb,adlib,pc-speaker,none;video=vga"
    );
    let (code, shown, err) = run(p, &["show", "synthetic-work:r1"]);
    assert_eq!(code, 0, "{err}");
    assert!(shown.contains(lines), "{shown}");

    // A set without the flags keeps schema 1 and shows none of these lines.
    let (code, _, err) = import("test-plain:r1", &[]);
    assert_eq!(code, 0, "{err}");
    let plain = store
        .resolve("test-plain:r1", &Validator(&PcMediaFormats))
        .unwrap();
    assert!(plain.metadata.starts_with(br#"{"schema":1,"#));
    assert!(!plain.manifest.annotations.contains_key(needs::ANNOTATION));
    let (code, shown, err) = run(p, &["show", "test-plain:r1"]);
    assert_eq!(code, 0, "{err}");
    for key in ["requirements", "need", "work", "synopsis", "work-id"] {
        assert!(!shown.contains(&format!("\n{key}\t")), "{shown}");
    }
}

#[test]
fn an_unknown_need_or_work_id_is_refused_before_anything_is_written() {
    let t = Temp::new();
    let p = &t.0;
    let disk = raw_disk(&p.join("disk.img"), 10);
    for (flags, reason) in [
        (
            vec!["--need", "video=8514"],
            "need video: \"8514\" is not one of mda, hercules, cga, ega, mcga, vga, svga, none",
        ),
        (
            vec!["--need", "chipset=aga"],
            "need kind \"chipset\" is not one of cpu, fpu, video, sound, memory, ems, xms, input",
        ),
        (vec!["--need", "memory=640"], "need memory: \"640\""),
        (
            vec!["--need", "memory=640K,1M"],
            "need memory: a quantity is one minimum, not 2 values",
        ),
        (
            vec!["--work-id", "steam=1"],
            "the work's id in catalogue \"steam\": the catalogue is not one of wikidata, mobygames, igdb",
        ),
        (
            vec!["--work", "synthetic-work"],
            "work \"synthetic-work\": the releases of a work are tagged in the repository named after it, \
             but the tag test-refused:r1 names the repository \"test-refused\", whose last component is \"test-refused\"",
        ),
    ] {
        let mut args = vec![
            "import",
            disk.to_str().unwrap(),
            "--title",
            "Synthetic refused work",
            "--tag",
            "test-refused:r1",
            "--create",
        ];
        args.extend(&flags);
        let (code, _, err) = run(p, &args);
        assert_eq!(code, 1, "{flags:?}: {err}");
        assert!(err.contains(reason), "{flags:?}: {err}");
        // The field and its rule, without the prefix of a damaged artifact.
        assert!(!err.contains("invalid image"), "{flags:?}: {err}");
        assert_eq!(
            blob_count(p),
            0,
            "{flags:?}: a refused import writes no blobs"
        );
    }
    // The usage names the schema-2 flags.
    let (_, _, err) = run(p, &["import", "--need", "cpu"]);
    assert!(
        err.contains("[--need KIND=VALUE[,VALUE]...]... [--requirements TEXT] [--work ID] [--synopsis TEXT] [--work-id wikidata|mobygames|igdb=ID]..."),
        "{err}"
    );
}

#[test]
fn a_blank_duplicate_or_misplaced_set_label_version_or_edition_is_refused() {
    let t = Temp::new();
    let p = &t.0;
    let disk = raw_disk(&p.join("disk.img"), 8);
    let long = "x".repeat(129);
    for (flags, reason) in [
        (vec!["--set-label", " "], "blank set metadata value"),
        (vec!["--version", "\t"], "blank set metadata value"),
        (vec!["--edition", "  "], "blank set metadata value"),
        (
            vec!["--set-label", "same", "--set-label", " same "],
            "duplicate set label",
        ),
        (
            vec!["--version", "1", "--version", "2"],
            "unknown or duplicate media option",
        ),
        (
            vec!["--edition", "A", "--edition", "B"],
            "unknown or duplicate media option",
        ),
        (
            vec!["--set-label", long.as_str()],
            "invalid set label: at most 128 characters, no control characters",
        ),
        (
            vec!["--set-label", "two\nlines"],
            "invalid set label: at most 128 characters, no control characters",
        ),
        (vec!["--version"], "missing media option value"),
    ] {
        let mut args = vec![
            "import",
            disk.to_str().unwrap(),
            "--title",
            "Synthetic refused release",
            "--tag",
            "test-refused:r1",
            "--create",
        ];
        args.extend(&flags);
        let (code, _, err) = run(p, &args);
        assert_eq!(code, 2, "{flags:?}: {err}");
        assert!(err.contains(reason), "{flags:?}: {err}");
        assert_eq!(
            blob_count(p),
            0,
            "{flags:?}: a refused import writes no blobs"
        );
    }
    // The usage names the new set-level flags.
    let (_, _, err) = run(p, &["import", "--version", " "]);
    assert!(
        err.contains("[--version V] [--edition E] [--set-label L]..."),
        "{err}"
    );
}

/// `image push LOCAL [TARGET] --dry-run` against the default registry with no
/// configured registries: the guard's verdict for the official namespace.
fn dry_run(root: &Path, tag: &str) -> (u8, String, String) {
    let preferences = root.join("empty.toml");
    fs::write(&preferences, "").unwrap();
    let args: Vec<OsString> = vec![
        "image".into(),
        "push".into(),
        tag.into(),
        "shareware-1:r1".into(),
        "--store".into(),
        root.join("store").into_os_string(),
        "--registry".into(),
        "registry.unemu.dev".into(),
        "--config".into(),
        preferences.into_os_string(),
        "--dry-run".into(),
    ];
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run(&args, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn an_open_set_shows_its_licence_and_terms_and_only_a_linked_closure_enters_unemu() {
    use image_store::{license, Digest, Manifest};
    let t = Temp::new();
    let p = &t.0;
    let one = raw_disk(&p.join("one.img"), 1);
    let two = raw_disk(&p.join("two.img"), 2);
    let manual = p.join("MANUAL.TXT");
    fs::write(&manual, b"Synthetic manual").unwrap();
    let terms = p.join("VENDOR.TXT");
    fs::write(&terms, b"Synthetic vendor terms").unwrap();
    let (code, out, err) = run(
        p,
        &[
            "import",
            one.to_str().unwrap(),
            two.to_str().unwrap(),
            "--title",
            "Synthetic shareware",
            "--tag",
            "test-open:r1",
            "--extra",
            manual.to_str().unwrap(),
            "--extra",
            &format!("{}:VENDOR.TXT", terms.display()),
            "--open",
            "--license",
            "LicenseRef-synthetic-shareware",
            "--license-text",
            "VENDOR.TXT",
            "--create",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("open\tLicenseRef-synthetic-shareware\tterms\t\"VENDOR.TXT\"\n"),
        "{out}"
    );
    let terms_digest = Digest::of(b"Synthetic vendor terms").to_string();
    let (code, out, err) = run(p, &["show", "test-open:r1"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains(&format!("terms\t\"VENDOR.TXT\"\t{terms_digest}\n")),
        "{out}"
    );
    let layers: Vec<&str> = out.lines().filter(|l| l.starts_with("layer\t")).collect();
    assert_eq!(layers.len(), 6, "{out}");
    for line in &layers {
        let generated =
            line.starts_with("layer\tconfig\t") || line.starts_with("layer\tmetadata\t");
        let class = if generated {
            "\topen\tCC0-1.0\t-".to_owned()
        } else {
            format!("\topen\tLicenseRef-synthetic-shareware\t{terms_digest}")
        };
        assert!(line.ends_with(&class), "{line}");
    }
    assert!(
        out.ends_with("push\tany configured registry, including unemu/\n"),
        "{out}"
    );
    let (code, out, err) = dry_run(p, "test-open:r1");
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.starts_with("target:registry.unemu.dev/unemu/shareware-1:r1\n"),
        "{out}"
    );
    assert!(out.ends_with("guard:allowed\n"), "{out}");

    // Hand-edited copies of the set: one disk loses its link, or every layer
    // links a terms text that is not in the closure.
    let store = Store::open_existing(p.join("store")).unwrap();
    let image = store
        .resolve("test-open:r1", &Validator(&PcMediaFormats))
        .unwrap();
    for (tag, edit) in [
        ("test-unlinked:r1", "missing"),
        ("test-elsewhere:r1", "elsewhere"),
    ] {
        let mut layers = image.manifest.layers.clone();
        match edit {
            "missing" => {
                layers[0].annotations.remove(license::TERMS);
            }
            _ => {
                for d in layers
                    .iter_mut()
                    .filter(|d| d.annotations.contains_key(license::TERMS))
                {
                    d.annotations
                        .insert(license::TERMS.into(), Digest::of(b"elsewhere").to_string());
                }
            }
        }
        let mut manifest =
            Manifest::new_media_set("ibm-pc", image.manifest.config.clone(), layers).unwrap();
        manifest.annotations.clone_from(&image.manifest.annotations);
        let descriptor = store.import_manifest(&manifest).unwrap();
        store
            .update_tag(tag, &descriptor, None, &Validator(&PcMediaFormats))
            .unwrap();
        let (code, out, err) = dry_run(p, tag);
        assert_eq!(code, 1, "{tag}: {out}{err}");
        assert!(out.ends_with("guard:refused\n"), "{tag}: {out}");
        assert!(
            err.contains("terms text") && err.contains("--license-text"),
            "{tag}: {err}"
        );
        let (code, out, err) = run(p, &["show", tag]);
        assert_eq!(code, 0, "{err}");
        assert!(
            out.ends_with("push\tprivate registries outside unemu/ only: open media without the set's terms text\n"),
            "{tag}: {out}"
        );
    }

    // A set imported without the statement stays user-supplied and private.
    let three = raw_disk(&p.join("three.img"), 3);
    let (code, _, err) = run(
        p,
        &[
            "import",
            three.to_str().unwrap(),
            "--title",
            "Synthetic private set",
            "--tag",
            "test-private:r1",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, _) = run(p, &["show", "test-private:r1"]);
    assert_eq!(code, 0);
    assert!(!out.contains("\nterms\t"), "{out}");
    assert!(out.contains("layer\tmedium\t"), "{out}");
    assert!(
        out.lines()
            .filter(|l| l.starts_with("layer\t"))
            .all(|l| l.ends_with("\tuser-supplied\t-\t-")),
        "{out}"
    );
    assert!(
        out.ends_with("push\tprivate registries outside unemu/ only: user-supplied layers\n"),
        "{out}"
    );
    let (code, out, err) = dry_run(p, "test-private:r1");
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("official namespace"), "{err}");

    // Rights follow the bytes: once another local set records one of the
    // open set's disks user-supplied, the open set stays private too.
    let (code, _, err) = run(
        p,
        &[
            "import",
            one.to_str().unwrap(),
            "--title",
            "Synthetic copy",
            "--tag",
            "test-copy:r1",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = dry_run(p, "test-open:r1");
    assert_eq!(code, 1, "{out}{err}");
    assert!(
        out.contains("\tuser-supplied\tlocal tag test-copy:r1\n"),
        "{out}"
    );
    assert!(err.contains("official namespace"), "{err}");
}

#[test]
fn an_spdx_licensed_set_needs_its_source_and_only_a_linked_closure_enters_unemu() {
    use image_store::{license, Digest, Manifest};
    let t = Temp::new();
    let p = &t.0;
    let one = raw_disk(&p.join("one.img"), 1);
    let two = raw_disk(&p.join("two.img"), 2);
    let copying = p.join("COPYING");
    fs::write(&copying, b"Synthetic licence text").unwrap();
    let archive = p.join("SRC.TAR");
    fs::write(&archive, b"Synthetic source archive").unwrap();
    let import = |tag: &str, statement: &[&str]| {
        let mut args = vec![
            "import",
            one.to_str().unwrap(),
            two.to_str().unwrap(),
            "--title",
            "Synthetic free software",
            "--tag",
            tag,
            "--extra",
            copying.to_str().unwrap(),
            "--extra",
            archive.to_str().unwrap(),
            "--create",
        ];
        args.extend(statement);
        run(p, &args)
    };
    // --source belongs to an open statement, and an SPDX licence needs it.
    let (code, _, err) = import("test-gpl:r1", &["--source", "SRC.TAR"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--source names the source archive of an open set; add --open"));
    assert!(
        err.contains("[--open --license EXPRESSION --license-text EXTRA --source EXTRA]"),
        "{err}"
    );
    let gpl = [
        "--open",
        "--license",
        "GPL-2.0-or-later AND MIT",
        "--license-text",
        "COPYING",
    ];
    let (code, _, err) = import("test-gpl:r1", &gpl);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("the vendor's own terms") && err.contains("corresponding source"),
        "{err}"
    );
    let mut with_source = gpl.to_vec();
    with_source.extend(["--source", "SOURCES.ZIP"]);
    let (code, _, err) = import("test-gpl:r1", &with_source);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("source archive \"SOURCES.ZIP\": not the file name of an extra"));
    with_source[6] = "SRC.TAR";
    let (code, out, err) = import("test-gpl:r1", &with_source);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("open\tGPL-2.0-or-later AND MIT\tterms\t\"COPYING\"\nsource\t\"SRC.TAR\"\n"),
        "{out}"
    );
    // show names the licence text and the source archive every layer links.
    let (terms, source) = (
        Digest::of(b"Synthetic licence text").to_string(),
        Digest::of(b"Synthetic source archive").to_string(),
    );
    let (code, out, err) = run(p, &["show", "test-gpl:r1"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains(&format!(
            "terms\t\"COPYING\"\t{terms}\nsource\t\"SRC.TAR\"\t{source}\n"
        )),
        "{out}"
    );
    for line in out
        .lines()
        .filter(|l| l.starts_with("layer\tmedium\t") || l.starts_with("layer\textra\t"))
    {
        assert!(
            line.ends_with(&format!("\topen\tGPL-2.0-or-later AND MIT\t{terms}")),
            "{line}"
        );
    }
    assert!(
        out.ends_with("push\tany configured registry, including unemu/\n"),
        "{out}"
    );
    let (code, out, err) = dry_run(p, "test-gpl:r1");
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.ends_with("guard:allowed\n"), "{out}");

    // A hand-edited copy whose disk no longer links the archive is refused
    // for unemu/ and shown as private-only.
    let store = Store::open_existing(p.join("store")).unwrap();
    let image = store
        .resolve("test-gpl:r1", &Validator(&PcMediaFormats))
        .unwrap();
    let mut layers = image.manifest.layers.clone();
    layers[0].annotations.remove(license::SOURCE);
    let mut manifest =
        Manifest::new_media_set("ibm-pc", image.manifest.config.clone(), layers).unwrap();
    manifest.annotations.clone_from(&image.manifest.annotations);
    let descriptor = store.import_manifest(&manifest).unwrap();
    store
        .update_tag(
            "test-unsourced:r1",
            &descriptor,
            None,
            &Validator(&PcMediaFormats),
        )
        .unwrap();
    let (code, out, err) = dry_run(p, "test-unsourced:r1");
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.ends_with("guard:refused\n"), "{out}");
    assert!(
        err.contains("these open copyleft media have no source archive")
            && err.contains("--source EXTRA"),
        "{err}"
    );
    let (code, out, err) = run(p, &["show", "test-unsourced:r1"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.ends_with(
            "push\tprivate registries outside unemu/ only: open copyleft media without the set's source archive\n"
        ),
        "{out}"
    );
}

/// A new set takes a revision tag, and its release alias follows it; an
/// alias alone, or a tag outside the scheme, is refused before any medium is
/// copied, and the refusal names the revision to use.
#[test]
fn a_new_set_takes_a_revision_tag_and_its_release_alias_follows() {
    let t = Temp::new();
    let p = &t.0;
    disk(&p.join("a.img"));
    let image = p.join("a.img");
    let import = |tag: &str, create: bool| {
        let mut args = vec![
            "import",
            image.to_str().unwrap(),
            "--title",
            "Revision set",
            "--tag",
            tag,
        ];
        if create {
            args.push("--create");
        }
        run(p, &args)
    };
    let (code, _, err) = import("lab-set:1.0", true);
    assert_eq!(code, 1);
    assert!(
        err.contains("tag the image lab-set:1.0-r1, and the alias lab-set:1.0 follows"),
        "{err}"
    );
    let (code, _, err) = import("lab-set:Beta", false);
    assert_eq!(code, 1);
    assert!(
        err.contains("neither a revision tag nor a release alias"),
        "{err}"
    );
    let (code, out, err) = import("lab-set:1.0-r1", false);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("lab-set:1.0-r1\tsha256:"), "{out}");
    let (code, listed, err) = run(p, &["ls"]);
    assert_eq!(code, 0, "{err}");
    let tags: Vec<&str> = listed
        .lines()
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    assert_eq!(tags, ["lab-set:1.0", "lab-set:1.0-r1"], "{listed}");
}
