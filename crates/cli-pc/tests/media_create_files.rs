// SPDX-License-Identifier: AGPL-3.0-or-later
//! `media create floppy --add`: FAT12 disks built from host files and folders,
//! read back through the media crate's independent reader. See
//! machines/README.md § Preparing disks and the Microsoft FAT Specification,
//! FAT Directory Structure.

use cli_pc::command::run;
use media::fat12::reader::Volume;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "unemupc-create-files-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    fn file(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    /// `media create floppy` with the given arguments; `@name` stands for a
    /// path inside the scratch folder.
    fn floppy(&self, parts: &[&str]) -> (u8, String, String) {
        let args: Vec<OsString> = ["media", "create", "floppy"]
            .iter()
            .map(OsString::from)
            .chain(parts.iter().map(|part| match part.strip_prefix('@') {
                Some(relative) => self.path(relative).into_os_string(),
                None => OsString::from(part),
            }))
            .collect();
        let (mut output, mut errors) = (Vec::new(), Vec::new());
        let code = run(&args, &mut output, &mut errors);
        (
            code,
            String::from_utf8(output).unwrap(),
            String::from_utf8(errors).unwrap(),
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn pattern(length: usize, seed: u8) -> Vec<u8> {
    (0..length)
        .map(|i| (i as u8).wrapping_mul(13).wrapping_add(seed))
        .collect()
}

#[test]
fn add_builds_a_readable_tree_and_prints_names_skips_and_free_space() {
    let scratch = Scratch::new();
    scratch.file("game/GAME.EXE", &pattern(2000, 1));
    scratch.file("game/data/level1.dat", &pattern(3000, 2));
    scratch.file("game/long file name.txt", b"long name\r\n");
    scratch.file("game/.DS_Store", b"junk");
    scratch.file("notes.txt", b"notes\r\n");
    for (kind, sectors) in [
        ("1.44M", 2880),
        ("1.2M", 2400),
        ("720K", 1440),
        ("360K", 720),
    ] {
        let (code, output, errors) = scratch.floppy(&[
            "--type",
            kind,
            "--add",
            "@game",
            "--add",
            "@notes.txt",
            "--label",
            "mygame",
            "--force",
            "@out.img",
        ]);
        assert_eq!(code, 0, "{errors}");
        let image = fs::read(scratch.path("out.img")).unwrap();
        assert_eq!(image.len(), sectors * 512);
        let volume = Volume::open(&image).unwrap();
        volume.check().unwrap();
        assert_eq!(volume.label().as_deref(), Some("MYGAME"));
        assert_eq!(volume.read("GAME\\GAME.EXE").unwrap(), pattern(2000, 1));
        assert_eq!(
            volume.read("GAME\\DATA\\LEVEL1.DAT").unwrap(),
            pattern(3000, 2)
        );
        assert_eq!(volume.read("GAME\\LONG_FIL.TXT").unwrap(), b"long name\r\n");
        assert_eq!(volume.read("NOTES.TXT").unwrap(), b"notes\r\n");
        let expected = format!(
            "created {} ({} bytes): label MYGAME, 4 files, {} bytes free\n",
            scratch.path("out.img").display(),
            sectors * 512,
            volume.free_bytes()
        );
        assert!(output.starts_with(&expected), "{output}");
        assert!(
            output.contains(&format!(
                "renamed {} -> GAME\\LONG_FIL.TXT\n",
                scratch.path("game/long file name.txt").display()
            )),
            "{output}"
        );
        assert!(
            output.contains(&format!(
                "skipped {}: host metadata",
                scratch.path("game/.DS_Store").display()
            )),
            "{output}"
        );
    }
}

#[test]
fn usage_documents_the_file_options() {
    let mut output = Vec::new();
    assert_eq!(run(&[], &mut output, &mut Vec::new()), 0);
    let usage = String::from_utf8(output).unwrap();
    for option in ["--add PATH", "--top NAME", "--split"] {
        assert!(usage.contains(option), "{option}: {usage}");
    }
}

#[test]
fn a_misfit_without_split_writes_nothing_and_names_the_shortfall() {
    let scratch = Scratch::new();
    scratch.file("big/one.bin", &pattern(200_000, 1));
    scratch.file("big/two.bin", &pattern(200_000, 2));
    let (code, output, errors) = scratch.floppy(&["--type", "360K", "--add", "@big", "@out.img"]);
    // 2 x 196 clusters plus the folder's cluster against 354.
    assert_eq!(code, 1, "{output}");
    assert!(errors.contains("39936 bytes short"), "{errors}");
    assert!(
        errors.contains("use --split for a set of disks, or --type 1.2M"),
        "{errors}"
    );
    assert!(!scratch.path("out.img").exists());
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 1);
}

#[test]
fn split_writes_numbered_disks_that_each_keep_the_full_path() {
    let scratch = Scratch::new();
    for level in ["level1", "level2"] {
        for (seed, name) in ["a.dat", "b.dat", "c.dat"].iter().enumerate() {
            scratch.file(
                &format!("azura/{level}/{name}"),
                &pattern(150_000, seed as u8),
            );
        }
    }
    let (code, output, errors) = scratch.floppy(&[
        "--type", "720K", "--add", "@azura", "--label", "AZURA", "--split", "@set.img",
    ]);
    assert_eq!(code, 0, "{errors}");
    assert!(!scratch.path("set.img").exists());
    let mut files = 0;
    for (disk, count) in [(1, 4), (2, 2)] {
        let path = scratch.path(&format!("set-{disk}of2.img"));
        let image = fs::read(&path).unwrap();
        let volume = Volume::open(&image).unwrap();
        volume.check().unwrap();
        assert_eq!(volume.label(), Some(format!("AZURA {disk}")));
        for (dos, entry) in volume.walk().unwrap() {
            assert!(dos == "AZURA" || dos.starts_with("AZURA\\"), "{dos}");
            if !entry.is_dir() {
                files += 1;
            }
        }
        assert!(
            output.contains(&format!(
                "created {} (737280 bytes): label AZURA {disk}, {count} files, {} bytes free\n",
                path.display(),
                volume.free_bytes()
            )),
            "{output}"
        );
    }
    assert_eq!(files, 6);
}

#[test]
fn split_onto_one_disk_keeps_the_plain_name_and_label() {
    let scratch = Scratch::new();
    scratch.file("small.txt", b"small");
    let (code, output, errors) = scratch.floppy(&[
        "--type",
        "1.44M",
        "--add",
        "@small.txt",
        "--label",
        "SMALL",
        "--split",
        "@one.img",
    ]);
    assert_eq!(code, 0, "{errors}");
    let image = fs::read(scratch.path("one.img")).unwrap();
    assert_eq!(
        Volume::open(&image).unwrap().label().as_deref(),
        Some("SMALL")
    );
    assert!(!scratch.path("one-1of1.img").exists());
    assert!(output.contains("label SMALL, 1 file,"), "{output}");
}

#[test]
fn existing_outputs_are_kept_without_force() {
    let scratch = Scratch::new();
    scratch.file("one/a.bin", &pattern(300_000, 1));
    scratch.file("one/b.bin", &pattern(300_000, 2));
    let old = scratch.file("kept.img", b"old base");
    let (code, _, errors) = scratch.floppy(&["--type", "360K", "--add", "@one/a.bin", "@kept.img"]);
    assert_eq!(code, 1);
    assert!(errors.contains("--force"), "{errors}");
    assert_eq!(fs::read(&old).unwrap(), b"old base");

    // Every disk of a set is checked before the first is written.
    let second = scratch.file("set-2of2.img", b"second");
    let split = ["--type", "360K", "--add", "@one", "--split", "@set.img"];
    let (code, _, errors) = scratch.floppy(&split);
    assert_eq!(code, 1);
    assert!(errors.contains("set-2of2.img"), "{errors}");
    assert!(!scratch.path("set-1of2.img").exists());
    assert_eq!(fs::read(&second).unwrap(), b"second");

    let (code, _, errors) = scratch.floppy(&[&split[..], &["--force"]].concat());
    assert_eq!(code, 0, "{errors}");
    for disk in 1..=2 {
        let image = fs::read(scratch.path(&format!("set-{disk}of2.img"))).unwrap();
        Volume::open(&image).unwrap().check().unwrap();
    }
}

#[test]
fn a_file_larger_than_a_disk_is_named_and_never_split() {
    let scratch = Scratch::new();
    scratch.file("huge.bin", &pattern(400_000, 1));
    for split in [false, true] {
        let mut parts = vec!["--type", "360K", "--add", "@huge.bin", "@out.img"];
        if split {
            parts.push("--split");
        }
        let (code, _, errors) = scratch.floppy(&parts);
        assert_eq!(code, 1);
        assert!(errors.contains("huge.bin (400000 bytes)"), "{errors}");
        assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 1);
    }
}

#[test]
fn file_option_mistakes_are_usage_errors_before_any_output() {
    let scratch = Scratch::new();
    scratch.file("a.txt", b"a");
    for parts in [
        &["--type", "1.44M", "--top", "GAME", "@out.img"][..],
        &["--type", "1.44M", "--split", "@out.img"],
        &["--type", "1.44M", "--format", "--split", "@out.img"],
        &[
            "--type", "1.44M", "--add", "@a.txt", "--top", "bad/name", "@out.img",
        ],
        &[
            "--type",
            "1.44M",
            "--add",
            "@a.txt",
            "--top",
            "TOOLONGNAME",
            "@out.img",
        ],
        &[
            "--type", "1.44M", "--add", "@a.txt", "--top", "A", "--top", "B", "@out.img",
        ],
        &[
            "--type", "1.44M", "--add", "@a.txt", "--split", "--split", "@out.img",
        ],
        &["--type", "1.44M", "--add", "", "@out.img"],
        &["--type", "1.44M", "--add", "--split", "@out.img"],
        &["--type", "1.44M", "@out.img", "--add"],
    ] {
        let (code, _, errors) = scratch.floppy(parts);
        assert_eq!(code, 2, "{parts:?}: {errors}");
        assert!(!scratch.path("out.img").exists(), "{parts:?}");
    }
    let args: Vec<OsString> = ["media", "create", "hd", "--size", "1MiB", "--add"]
        .iter()
        .map(OsString::from)
        .chain([
            scratch.path("a.txt").into_os_string(),
            scratch.path("out.img").into_os_string(),
        ])
        .collect();
    assert_eq!(run(&args, &mut Vec::new(), &mut Vec::new()), 2);
    // --add implies --format, and --label goes with it.
    for parts in [
        &[
            "--type", "1.44M", "--add", "@a.txt", "--label", "A", "@l.img",
        ][..],
        &[
            "--type", "1.44M", "--format", "--add", "@a.txt", "--top", "game", "@f.img",
        ],
    ] {
        let (code, _, errors) = scratch.floppy(parts);
        assert_eq!(code, 0, "{parts:?}: {errors}");
    }
    let image = fs::read(scratch.path("f.img")).unwrap();
    assert_eq!(
        Volume::open(&image).unwrap().read("GAME\\A.TXT").unwrap(),
        b"a"
    );
}

#[test]
fn named_sources_that_cannot_be_copied_are_refused() {
    let scratch = Scratch::new();
    scratch.file("game/GAME.EXE", b"game");
    scratch.file(".DS_Store", b"junk");
    let mut refused = vec![("@.DS_Store", "is host metadata")];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("game", scratch.path("linked")).unwrap();
        refused.push(("@linked", "is a symbolic link; name its target"));
    }
    for (source, message) in refused {
        let (code, _, errors) = scratch.floppy(&[
            "--type", "1.44M", "--add", "@game", "--add", source, "@out.img",
        ]);
        assert_eq!(code, 1, "{source}");
        assert!(errors.contains(message), "{errors}");
        assert!(!scratch.path("out.img").exists());
    }
}

#[test]
fn a_source_named_twice_is_refused_naming_both() {
    let scratch = Scratch::new();
    scratch.file("g/one.txt", b"1");
    let (code, _, errors) =
        scratch.floppy(&["--type", "1.44M", "--add", "@g", "--add", "@g", "@out.img"]);
    assert_eq!(code, 1);
    let g = scratch.path("g").display().to_string();
    assert!(
        errors.contains(&format!("{g} and {g} are the same host file or folder")),
        "{errors}"
    );
    assert!(!scratch.path("out.img").exists());
}

#[test]
fn earlier_images_inside_the_source_tree_are_left_out() {
    let scratch = Scratch::new();
    scratch.file("game/GAME.EXE", b"game");
    for _ in 0..2 {
        let (code, _, errors) = scratch.floppy(&[
            "--type",
            "1.44M",
            "--add",
            "@game",
            "--force",
            "@game/game.img",
        ]);
        assert_eq!(code, 0, "{errors}");
    }
    let image = fs::read(scratch.path("game/game.img")).unwrap();
    let paths: Vec<_> = Volume::open(&image)
        .unwrap()
        .walk()
        .unwrap()
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert_eq!(paths, ["GAME", "GAME\\GAME.EXE"]);
    // A split set looks for every earlier member beside its output.
    fs::remove_file(scratch.path("game/game.img")).unwrap();
    for name in ["set-1of2.img", "set-2of3.img", "set-x.img"] {
        scratch.file(&format!("game/{name}"), b"earlier");
    }
    let (code, output, errors) = scratch.floppy(&[
        "--type",
        "1.44M",
        "--add",
        "@game",
        "--split",
        "--force",
        "@game/set.img",
    ]);
    assert_eq!(code, 0, "{errors}");
    for name in ["set-1of2.img", "set-2of3.img"] {
        let line = format!(
            "skipped {}: the image being written",
            scratch.path(&format!("game/{name}")).display()
        );
        assert!(output.contains(&line), "{output}");
    }
    let image = fs::read(scratch.path("game/set.img")).unwrap();
    assert_eq!(
        Volume::open(&image)
            .unwrap()
            .read("GAME\\SET-X.IMG")
            .unwrap(),
        b"earlier",
        "a file that only looks similar is copied"
    );
}

#[test]
fn a_failed_disk_of_a_set_names_the_disks_already_written() {
    let scratch = Scratch::new();
    scratch.file("one/a.bin", &pattern(300_000, 1));
    scratch.file("one/b.bin", &pattern(300_000, 2));
    // A folder in the second disk's place cannot be replaced.
    scratch.file("set-2of2.img/keep", b"keep");
    let (code, _, errors) = scratch.floppy(&[
        "--type", "360K", "--add", "@one", "--split", "--force", "@set.img",
    ]);
    assert_eq!(code, 1);
    let first = scratch.path("set-1of2.img");
    assert!(
        errors.contains(&format!("already written: {}", first.display())),
        "{errors}"
    );
    Volume::open(&fs::read(&first).unwrap())
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(
        fs::read(scratch.path("set-2of2.img/keep")).unwrap(),
        b"keep"
    );
}
