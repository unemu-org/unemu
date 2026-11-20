// SPDX-License-Identifier: AGPL-3.0-or-later
//! Volumes built from a user's own copy of real games, as a recipe's `fat`
//! step will build them (backlog/docs/design/recipes.md §6.9 and the recipe
//! corpus beside it), come out byte for byte the same twice: Ken's Labyrinth
//! on one 1.44M disk in the archive's order with the archive's times, and
//! Azuramoon split over 1.44M disks. The files are found by size and SHA-256
//! in the user-ROM roots (`vendor/user-roms`, then `UNEMUPC_USER_ROMS_DIR`);
//! the repository holds only the corpus's names, sizes and hashes. Without
//! the files the tests print one SKIPPED line, or fail under
//! `UNEMUPC_REQUIRE_USER_ROMS=1`. Nothing about the user's files but counts
//! is printed.

use media::fat12::{
    plan_volumes, volume_label, BuildError, DosTime, Entry, FloppyFormat, Order, Volume,
    DEFAULT_SERIAL,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const KENSLAB: &str =
    include_str!("../../../backlog/docs/design/recipe-corpus/kenslab-2.01/kenslab-2.01.media.toml");
const AZURAMOON: &str =
    include_str!("../../../backlog/docs/design/recipe-corpus/azuramon/azuramoon-2026.media.toml");

/// One listed file: path, size, SHA-256 and the time a recipe states.
struct Wanted {
    path: String,
    size: u64,
    sha256: String,
    time: Option<DosTime>,
}

fn wanted(list: &toml::Value) -> Vec<Wanted> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| Wanted {
            path: item["path"].as_str().unwrap().to_owned(),
            size: item["size"].as_integer().unwrap() as u64,
            sha256: item["sha256"].as_str().unwrap().to_owned(),
            time: item
                .get("time")
                .map(|time| time.as_datetime().unwrap().to_string().parse().unwrap()),
        })
        .collect()
}

/// Every regular file below `folder` whose size is wanted, symbolic links
/// followed and each folder visited once.
fn candidates(
    folder: &Path,
    sizes: &HashSet<u64>,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
) {
    let Ok(canonical) = folder.canonicalize() else {
        return;
    };
    if !seen.insert(canonical) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            candidates(&path, sizes, seen, out);
        } else if metadata.is_file() && sizes.contains(&metadata.len()) {
            out.push(path);
        }
    }
}

/// The contents of every wanted file, by SHA-256, or the skip line.
fn find(title: &str, files: &[Wanted]) -> Option<HashMap<String, Vec<u8>>> {
    let sizes: HashSet<u64> = files.iter().map(|file| file.size).collect();
    let hashes: HashSet<&str> = files.iter().map(|file| file.sha256.as_str()).collect();
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for root in machine_file::firmware_discovery::default_search_roots() {
        candidates(&root, &sizes, &mut seen, &mut paths);
    }
    let mut found = HashMap::new();
    for path in paths {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let digest = assembly::test_support::fixture_sha256(&bytes);
        if hashes.contains(digest.as_str()) {
            found.entry(digest).or_insert(bytes);
        }
    }
    let present = files
        .iter()
        .filter(|file| found.contains_key(&file.sha256))
        .count();
    if present == files.len() {
        return Some(found);
    }
    let message = format!(
        "SKIPPED: user-supplied files of {title} not found ({present} of {} files; set UNEMUPC_USER_ROMS_DIR or see vendor/user-roms/README.md)",
        files.len()
    );
    assert!(
        std::env::var_os("UNEMUPC_REQUIRE_USER_ROMS").as_deref() != Some(std::ffi::OsStr::new("1")),
        "{message}"
    );
    println!("{message}");
    None
}

fn entries(files: &[Wanted], contents: &HashMap<String, Vec<u8>>) -> Vec<Entry> {
    files
        .iter()
        .map(|file| Entry {
            path: file.path.clone(),
            contents: Some(contents[&file.sha256].clone()),
            time: file.time,
        })
        .collect()
}

/// The keys of a recipe's `fat` step (recipes.md §6.9), as the step reads
/// them: `format`, `label`, `serial`, `order`, `time` and `volumes`.
fn volume(step: &toml::Value) -> Volume {
    assert!(step.get("fat").is_some(), "not a fat step");
    let format = FloppyFormat::from_name(step["format"].as_str().unwrap()).unwrap();
    Volume {
        label: step
            .get("label")
            .map(|label| volume_label(label.as_str().unwrap()).unwrap()),
        serial: step
            .get("serial")
            .map_or(DEFAULT_SERIAL, |serial| serial.as_integer().unwrap() as u32),
        order: match step.get("order").map(|order| order.as_str().unwrap()) {
            None | Some("name") => Order::Name,
            Some("listed") => Order::Listed,
            Some(other) => panic!("unknown order {other}"),
        },
        time: step.get("time").map_or(DosTime::EPOCH, |time| {
            time.as_datetime().unwrap().to_string().parse().unwrap()
        }),
        volumes: step
            .get("volumes")
            .map(|volumes| volumes.as_integer().unwrap() as usize),
        ..Volume::new(format)
    }
}

/// The recipe's `fat` step.
fn fat_step(recipe: &toml::Value) -> &toml::Value {
    recipe["step"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step.get("fat").is_some())
        .unwrap()
}

#[test]
#[ignore = "user-supplied files: Ken's Labyrinth 2.01"]
fn user_rom_kenslab_disk_builds_twice_to_the_same_bytes() {
    let recipe: toml::Value = toml::from_str(KENSLAB).unwrap();
    let files = wanted(&recipe["step"][0]["members"]);
    assert_eq!(files.len(), 19);
    let Some(contents) = find("Ken's Labyrinth 2.01", &files) else {
        return;
    };
    // The keys of the recipe's `fat` step.
    let volume = volume(fat_step(&recipe));
    assert_eq!(
        (volume.format, volume.order, volume.volumes),
        (FloppyFormat::Hd144, Order::Listed, None)
    );
    let build = || {
        plan_volumes(entries(&files, &contents), &volume)
            .unwrap()
            .images()
            .unwrap()
    };
    let first = build();
    assert_eq!(first.len(), 1);
    assert_eq!(first, build());
    let reader = media::fat12::reader::Volume::open(&first[0]).unwrap();
    reader.check().unwrap();
    let root = reader.list("").unwrap();
    assert_eq!(
        root.iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        "listed order"
    );
    for (entry, file) in root.iter().zip(&files) {
        assert_eq!(reader.read(&entry.name).unwrap(), contents[&file.sha256]);
    }
    // MEASURED from the author's LABFULL.ZIP (its SHA-256 is in the recipe).
    // The bytes are those a recipe of version 1.0 freezes.
    assert_eq!(
        assembly::test_support::fixture_sha256(&first[0]),
        "16992a33eef5a71080dc6f0df526a2d723a4acb2f1dfca6b7d7032a7dbb80062"
    );

    // The same files split over 720K volumes: two, each built the same twice.
    let split = Volume {
        format: FloppyFormat::Dd720,
        volumes: Some(2),
        ..volume
    };
    let build = || {
        plan_volumes(entries(&files, &contents), &split)
            .unwrap()
            .images()
            .unwrap()
    };
    let first = build();
    assert_eq!(first.len(), 2);
    assert_eq!(first, build());
    let mut copied = Vec::new();
    for image in &first {
        let reader = media::fat12::reader::Volume::open(image).unwrap();
        reader.check().unwrap();
        for entry in reader.list("").unwrap() {
            copied.push(entry.name.clone());
            assert_eq!(
                reader.read(&entry.name).unwrap().len() as u64,
                files
                    .iter()
                    .find(|file| file.path == entry.name)
                    .unwrap()
                    .size
            );
        }
    }
    assert_eq!(
        copied,
        files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        "listed order across the volumes"
    );
}

#[test]
#[ignore = "user-supplied files: Azuramoon"]
fn user_rom_azuramoon_split_builds_twice_to_the_same_bytes() {
    let recipe: toml::Value = toml::from_str(AZURAMOON).unwrap();
    let files = wanted(&recipe["input"]["game"]["files"]);
    assert_eq!(files.len(), 561);
    let Some(contents) = find("Azuramoon", &files) else {
        return;
    };
    let step = volume(fat_step(&recipe));
    // The recipe's count of volumes is an assumption of its author; the
    // split states the number it fills.
    let stated = step.volumes;
    let volume = |volumes| Volume {
        volumes: Some(volumes),
        ..step.clone()
    };
    // Two is too few; the error names the number the split fills.
    let needed = match plan_volumes(entries(&files, &contents), &volume(2)) {
        Err(BuildError::WrongVolumes { needed, .. }) => needed,
        other => panic!("expected another number of volumes: {other:?}"),
    };
    assert!(needed > 2);
    let build = || {
        plan_volumes(entries(&files, &contents), &volume(needed))
            .unwrap()
            .images()
            .unwrap()
    };
    let first = build();
    assert_eq!(first.len(), needed);
    assert_eq!(first, build());
    let mut copied = 0;
    for image in &first {
        let reader = media::fat12::reader::Volume::open(image).unwrap();
        reader.check().unwrap();
        copied += reader
            .walk()
            .unwrap()
            .iter()
            .filter(|(_, entry)| !entry.is_dir())
            .count();
    }
    assert_eq!(copied, files.len(), "every file on exactly one volume");
    println!(
        "azuramoon: {needed} volumes (the recipe states {stated:?}), {}",
        first
            .iter()
            .map(|image| assembly::test_support::fixture_sha256(image))
            .collect::<Vec<_>>()
            .join(" ")
    );
}
