// SPDX-License-Identifier: AGPL-3.0-or-later
//! `image export NAME:TAG --to DIR [--files] [--store DIR]`: one
//! installation set or firmware package as plain files
//! (backlog/docs/design/recipes.md §3.10, decision-37). The layout, version
//! [`LAYOUT_VERSION`], is the one `scripts/export-layout.sh` writes from any
//! OCI Image Layout without this program (backlog/docs/commands.md,
//! Exporting an image):
//!
//! - every layer's decoded bytes under its export file name, the layer's
//!   `org.opencontainers.image.title` without `.zst` on a compressed layer
//!   ([`image_store::export_name`]): media, ROM images, extras, a set's
//!   `metadata.json` and a package's `README.md`;
//! - `config.toml` and `manifest.json`, the config and manifest bytes as
//!   stored;
//! - `EXPORT-VERSION`, the layout's version and a line feed;
//! - `SHA256SUMS`, a `sha256sum -c` list of every other file above, by the
//!   bytes of the names, written last.
//!
//! With `--files`, the folder `files` also holds the tree of every diskette
//! of a set that reads as a FAT12 volume ([`media::fat12::reader`]), the
//! volumes merged into one tree in the set's order, as the volumes of a split
//! set make up one tree: a file's name is its long name, else its 8.3 name
//! as stored in the case its entry's case bits give, and its modification
//! time is the last-write time of its entry
//! taken as UTC, as a folder's is. A path two volumes both hold is one
//! folder, or one file when the bytes are the same; other bytes, or a file
//! on one and a folder on the other, refuse the export. Attributes are not
//! set on the host. A medium that is not a FAT12 volume (an optical image,
//! a disk of another format) is reported and left out of the tree. The tree
//! is not listed in `SHA256SUMS`: it is the content of the images, which
//! are.
//!
//! Nothing is written until every file has been planned and checked: a name
//! that is already in `DIR` is kept when it holds the same bytes and
//! refuses the export when it holds others, is a folder where a file goes or
//! the other way round, or is a symbolic link. Each file is written under a
//! temporary name and renamed. The files name no host path.
//!
//! One case the check cannot see: names are compared by their bytes, case
//! aside, not by Unicode normalization, as no normalization table is in the
//! program. Two long names that differ only in composition (`é` as one
//! code point or as `e` and a combining accent) are two files on most
//! hosts and one on macOS, whose file systems treat them as one name. There
//! the second meets the first while the export writes, and the export stops
//! and removes every file and folder it wrote, so `DIR` is left as it was.

use super::super::{validators, CliPlatform, HostSources};
use image_store::{
    export_name, media_type, BlobSource, Descriptor, Manifest, ResolvedImage, Store, MAX_SMALL_BLOB,
};
use media::fat12::{reader, DosTime};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The version of the export layout, the content of `EXPORT-VERSION`.
pub const LAYOUT_VERSION: &str = "1";
/// The folder `--files` writes the trees of the disks into.
pub const FILES: &str = "files";

#[derive(Debug, PartialEq, Eq)]
pub struct ExportCommand {
    /// `None` without `--store`: the default store.
    pub store: Option<PathBuf>,
    pub tag: String,
    pub to: PathBuf,
    pub files: bool,
}

/// Parse the words after `image export`.
pub fn parse(args: &[OsString]) -> Result<ExportCommand, &'static str> {
    let (mut store, mut tag, mut to, mut files) = (None, None, None, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg.is_empty() {
            return Err("empty export argument");
        }
        if !arg.as_encoded_bytes().starts_with(b"--") {
            if tag.is_some() {
                return Err("image export takes one NAME:TAG");
            }
            tag = Some(arg.to_str().ok_or("invalid export tag")?.to_owned());
            continue;
        }
        if arg == "--files" {
            if files {
                return Err("--files is accepted once");
            }
            files = true;
            continue;
        }
        let value = args
            .next()
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing export option value")?;
        match arg.to_str() {
            Some("--store") if store.is_none() => store = Some(PathBuf::from(value)),
            Some("--to") if to.is_none() => to = Some(PathBuf::from(value)),
            _ => return Err("unknown or duplicate export option"),
        }
    }
    Ok(ExportCommand {
        store,
        tag: tag.ok_or("image export requires NAME:TAG")?,
        to: to.ok_or("image export requires --to DIR")?,
        files,
    })
}

pub fn execute(
    command: ExportCommand,
    output: &mut impl Write,
    host: &HostSources,
    platforms: &[&dyn CliPlatform],
) -> anyhow::Result<()> {
    let store = match command.store {
        Some(store) => store,
        None => host
            .store_root
            .clone()
            .ok_or_else(|| anyhow::anyhow!("default store unavailable; use --store DIR"))?,
    };
    let store = Store::open_existing(&store)?.with_compatibility(host.compatibility.clone());
    let (image, manifest) = resolve(&store, &command.tag, platforms)?;
    let plan = plan(&image, &manifest, command.files)?;
    check(&command.to, &plan, &image)?;
    write(&command.to, &plan, &image)?;
    for (name, reason) in &plan.not_read {
        writeln!(output, "not read\t{name}\t{reason}")?;
    }
    writeln!(
        output,
        "exported {}\t{}\t{} files",
        command.tag,
        image.image.digest,
        plan.plain.len()
    )?;
    if let Some(tree) = &plan.tree {
        let files = tree.iter().filter(|n| n.contents.is_some()).count();
        writeln!(
            output,
            "{FILES}\t{files} files\t{} folders\t{} volumes",
            tree.len() - files,
            plan.volumes
        )?;
    }
    Ok(())
}

/// The image of `tag`, resolved with its platform's validator for its
/// artifact type, and its manifest bytes as stored.
fn resolve(
    store: &Store,
    tag: &str,
    platforms: &[&dyn CliPlatform],
) -> anyhow::Result<(ResolvedImage, Vec<u8>)> {
    let descriptor = store.tag_descriptor(tag)?;
    let bytes = store.read_verified(&descriptor, MAX_SMALL_BLOB)?;
    let manifest = Manifest::decode_for_summary(&bytes)?;
    let kind = manifest.artifact_type();
    if kind != media_type::MEDIA_SET && kind != media_type::FIRMWARE {
        anyhow::bail!(
            "{tag} is not an installation set or a firmware package; image export writes those"
        );
    }
    let platform = manifest.platform()?;
    let validator = validators(platforms)
        .into_iter()
        .find(|v| v.artifact_type() == kind && image_store::platform::same(v.platform(), platform))
        .ok_or_else(|| anyhow::anyhow!("no platform compiled into this program reads {tag}"))?;
    let image = store.resolve_descriptor(&descriptor, validator)?;
    if let Some(missing) = image.missing_artwork.first() {
        anyhow::bail!(
            "{tag}: the artwork {} is not in the store; pull the image again to export it",
            export_name::of(missing)
                .map(str::to_owned)
                .unwrap_or_else(|| missing.digest.to_string())
        );
    }
    Ok((image, bytes))
}

/// What one plain file of the layout holds.
enum Source<'a> {
    Bytes(Vec<u8>),
    /// A layer's decoded bytes, streamed from the store.
    Layer(&'a Descriptor),
}

/// What a file is to hold, for the check of a name that is there.
enum Want<'a> {
    Bytes(&'a [u8]),
    Layer(&'a Descriptor),
}

impl<'a> Source<'a> {
    fn want(&'a self) -> Want<'a> {
        match self {
            Source::Bytes(bytes) => Want::Bytes(bytes),
            Source::Layer(layer) => Want::Layer(layer),
        }
    }
}

struct Plan<'a> {
    /// The plain files by name, in the order they are written.
    plain: Vec<(String, Source<'a>)>,
    /// The merged tree of `--files`.
    tree: Option<Vec<reader::Node>>,
    volumes: usize,
    /// Media `--files` did not read, by name, with the reason.
    not_read: Vec<(String, String)>,
}

fn plan<'a>(image: &'a ResolvedImage, manifest: &[u8], files: bool) -> anyhow::Result<Plan<'a>> {
    let layers = &image.manifest.layers;
    export_name::check(layers).map_err(|error| {
        anyhow::anyhow!(
            "{error}; an image written before every layer had a file name cannot be exported"
        )
    })?;
    let mut plain: Vec<(String, Source<'a>)> = layers
        .iter()
        .map(|layer| {
            let name = export_name::decoded(layer).unwrap_or_default().to_owned();
            (name, Source::Layer(layer))
        })
        .collect();
    plain.push((
        export_name::CONFIG.into(),
        Source::Bytes(image.config.clone()),
    ));
    plain.push((
        export_name::MANIFEST.into(),
        Source::Bytes(manifest.to_vec()),
    ));
    plain.push((
        export_name::VERSION.into(),
        Source::Bytes(format!("{LAYOUT_VERSION}\n").into_bytes()),
    ));
    let mut sums: Vec<(String, String)> = Vec::new();
    for (name, source) in &plain {
        let digest = match source {
            Source::Bytes(bytes) => image_store::Digest::of(bytes),
            Source::Layer(layer) => layer.decoded_digest()?,
        };
        sums.push((name.clone(), digest.hex().to_owned()));
    }
    sums.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let list: String = sums
        .iter()
        .map(|(name, hex)| format!("{hex}  {name}\n"))
        .collect();
    plain.push((export_name::SUMS.into(), Source::Bytes(list.into_bytes())));
    let mut plan = Plan {
        plain,
        tree: None,
        volumes: 0,
        not_read: Vec::new(),
    };
    if files {
        if let Some((name, _)) = plan
            .plain
            .iter()
            .find(|(name, _)| export_name::key(name) == export_name::key(FILES))
        {
            anyhow::bail!(
                "--files writes the folder {FILES}, and the layer {name} takes that name"
            );
        }
        tree(image, &mut plan)?;
    }
    Ok(plan)
}

/// The merged tree of every diskette of the set that reads as a FAT12
/// volume, in the set's order.
fn tree(image: &ResolvedImage, plan: &mut Plan<'_>) -> anyhow::Result<()> {
    let mut volumes = Vec::new();
    for layer in &image.manifest.layers {
        let kind = layer.decoded_media_type();
        if kind != media_type::FLOPPY && kind != media_type::OPTICAL {
            continue;
        }
        let name = export_name::decoded(layer).unwrap_or_default().to_owned();
        if kind != media_type::FLOPPY {
            plan.not_read.push((name, "not a diskette image".into()));
            continue;
        }
        let size = layer.decoded_size()?;
        if size > image_store::codec::MAX_FLOPPY {
            plan.not_read
                .push((name, "larger than any diskette".into()));
            continue;
        }
        let mut bytes = Vec::with_capacity(size as usize);
        image
            .content_reader(&layer.decoded_digest()?)?
            .read_to_end(&mut bytes)?;
        let volume = match reader::Volume::open(&bytes) {
            Ok(volume) => volume,
            Err(error) => {
                plan.not_read
                    .push((name, format!("not a FAT12 volume: {error}")));
                continue;
            }
        };
        let nodes = volume
            .tree()
            .map_err(|error| anyhow::anyhow!("--files: {name}: {error}"))?;
        volumes.push((name, nodes));
    }
    plan.volumes = volumes.len();
    plan.tree = Some(merge(volumes)?);
    Ok(())
}

/// The trees of the volumes, each with its medium's name, merged into one
/// in their order: a path two volumes hold (ignoring case, as the hosts
/// that ignore it compare names) is one folder, spelled and timed as the
/// first one has it, or one file when the bytes are the same; anything
/// else is refused, as is a name no host takes.
fn merge(volumes: Vec<(String, Vec<reader::Node>)>) -> anyhow::Result<Vec<reader::Node>> {
    let mut merged: Vec<reader::Node> = Vec::new();
    // The key of each path to its place in `merged` and its medium.
    let mut seen: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for (name, nodes) in volumes {
        for mut node in nodes {
            for part in node.path.split('/') {
                if let Some(problem) = export_name::problem(part) {
                    anyhow::bail!(
                        "--files: {name}: {}: the name {part:?} {problem}",
                        node.path
                    );
                }
            }
            let key = export_name::key(&node.path);
            let Some((at, first)) = seen.get(&key) else {
                // A tree lists a folder before its members, so the folder
                // is placed already, in the spelling it was placed with.
                if let Some((parent, leaf)) = node.path.rsplit_once('/') {
                    if let Some((at, _)) = seen.get(&export_name::key(parent)) {
                        node.path = format!("{}/{leaf}", merged[*at].path);
                    }
                }
                seen.insert(key, (merged.len(), name.clone()));
                merged.push(node);
                continue;
            };
            match (&merged[*at].contents, &node.contents) {
                (None, None) => {}
                (Some(a), Some(b)) if a == b => {}
                (Some(_), Some(_)) => anyhow::bail!(
                    "--files: {first} and {name} both hold {} with other bytes; export without --files",
                    node.path
                ),
                _ => anyhow::bail!(
                    "--files: {} is a file on one of {first} and {name} and a folder on the other; export without --files",
                    node.path
                ),
            }
        }
    }
    Ok(merged)
}

/// What a name in `DIR` holds before the export.
#[derive(PartialEq, Eq)]
enum Found {
    Nothing,
    Same,
}

/// Refuse, before anything is written, a name in `dir` that holds other
/// bytes, a folder where a file goes or a file where a folder goes, or a
/// symbolic link.
fn check(dir: &Path, plan: &Plan<'_>, image: &ResolvedImage) -> anyhow::Result<()> {
    folder(dir, "the export folder")?;
    for (name, source) in &plan.plain {
        found_file(&dir.join(name), name, source.want(), image)?;
    }
    if let Some(tree) = &plan.tree {
        let root = dir.join(FILES);
        folder(&root, FILES)?;
        for node in tree {
            let path = host_path(&root, &node.path);
            let shown = format!("{FILES}/{}", node.path);
            match &node.contents {
                None => folder(&path, &shown)?,
                Some(bytes) => {
                    found_file(&path, &shown, Want::Bytes(bytes), image)?;
                }
            }
        }
    }
    Ok(())
}

fn host_path(root: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(root.to_path_buf(), |at, part| at.join(part))
}

/// A folder that is there, or nothing.
fn folder(path: &Path, shown: &str) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::anyhow!("{shown}: {error}")),
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(meta) if meta.file_type().is_symlink() => {
            anyhow::bail!("{shown} is a symbolic link; the export writes no file through one")
        }
        Ok(_) => anyhow::bail!("{shown} is a file where the export writes a folder"),
    }
}

fn found_file(
    path: &Path,
    shown: &str,
    want: Want<'_>,
    image: &ResolvedImage,
) -> anyhow::Result<Found> {
    let meta = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Found::Nothing),
        Err(error) => return Err(anyhow::anyhow!("{shown}: {error}")),
        Ok(meta) => meta,
    };
    if meta.file_type().is_symlink() {
        anyhow::bail!("{shown} is a symbolic link; the export writes no file through one");
    }
    if !meta.is_file() {
        anyhow::bail!("{shown} is a folder where the export writes a file");
    }
    let differs = || {
        anyhow::anyhow!("{shown} is already there with other bytes; the export overwrites nothing")
    };
    let same = match want {
        Want::Bytes(bytes) => meta.len() == bytes.len() as u64 && fs::read(path)? == bytes,
        Want::Layer(layer) => {
            meta.len() == layer.decoded_size()?
                && same_stream(
                    fs::File::open(path)?,
                    image.content_reader(&layer.decoded_digest()?)?,
                )?
        }
    };
    if !same {
        return Err(differs());
    }
    Ok(Found::Same)
}

/// Whether two streams deliver the same bytes.
fn same_stream(mut a: impl Read, mut b: impl Read) -> io::Result<bool> {
    let (mut left, mut right) = (vec![0; 1 << 16], vec![0; 1 << 16]);
    loop {
        let count = read_full(&mut a, &mut left)?;
        if read_full(&mut b, &mut right[..count.max(1)])? != count {
            return Ok(false);
        }
        if count == 0 {
            return Ok(true);
        }
        if left[..count] != right[..count] {
            return Ok(false);
        }
    }
}

/// Fill `buffer` as far as the stream goes.
fn read_full(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// Write the plan into `dir`; on a failure, remove what this export wrote,
/// innermost first, and give the failure.
fn write(dir: &Path, plan: &Plan<'_>, image: &ResolvedImage) -> anyhow::Result<()> {
    let mut written = Vec::new();
    let result = write_all(dir, plan, image, &mut written);
    if result.is_err() {
        for (path, folder) in written.iter().rev() {
            let _ = if *folder {
                fs::remove_dir(path)
            } else {
                fs::remove_file(path)
            };
        }
    }
    result
}

/// What [`write`] wrote, in order: each path, and whether it is a folder.
type Written = Vec<(PathBuf, bool)>;

fn write_all(
    dir: &Path,
    plan: &Plan<'_>,
    image: &ResolvedImage,
    written: &mut Written,
) -> anyhow::Result<()> {
    let mut missing = Vec::new();
    let mut at = Some(dir);
    while let Some(path) = at.filter(|path| !path.as_os_str().is_empty() && !path.exists()) {
        missing.push(path.to_path_buf());
        at = path.parent();
    }
    for path in missing.into_iter().rev() {
        fs::create_dir(&path)?;
        written.push((path, true));
    }
    for (name, source) in &plan.plain {
        let path = dir.join(name);
        if found_file(&path, name, source.want(), image)? == Found::Same {
            continue;
        }
        match source {
            Source::Bytes(bytes) => publish(&path, &mut &bytes[..])?,
            Source::Layer(layer) => {
                publish(&path, &mut image.content_reader(&layer.decoded_digest()?)?)?
            }
        }
        written.push((path, false));
    }
    let Some(tree) = &plan.tree else {
        return Ok(());
    };
    let root = dir.join(FILES);
    if !root.is_dir() {
        fs::create_dir(&root)?;
        written.push((root.clone(), true));
    }
    for node in tree {
        let path = host_path(&root, &node.path);
        match &node.contents {
            None if path.is_dir() => {}
            None => {
                fs::create_dir(&path)?;
                written.push((path, true));
            }
            Some(bytes) => {
                let shown = format!("{FILES}/{}", node.path);
                if found_file(&path, &shown, Want::Bytes(bytes), image)? == Found::Nothing {
                    publish(&path, &mut &bytes[..])?;
                    written.push((path, false));
                }
            }
        }
    }
    // Times last, a folder's after its members were written; innermost
    // first, as the tree lists a folder before its members.
    for node in tree.iter().rev() {
        if let Some(time) = node.time {
            set_time(&host_path(&root, &node.path), time, node.contents.is_none())?;
        }
    }
    Ok(())
}

/// Write `path`, which is not there, from `source`: into a temporary file
/// beside it, then renamed.
fn publish(path: &Path, source: &mut dyn Read) -> anyhow::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temporary = path.with_file_name(format!(
        ".export-{}-{}.partial",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        io::copy(source, &mut file)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

/// Set the modification time of a written file or folder: the entry's
/// last-write time read as UTC.
fn set_time(path: &Path, time: DosTime, folder: bool) -> anyhow::Result<()> {
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(time.unix_seconds());
    let file = if folder {
        fs::File::open(path)?
    } else {
        fs::OpenOptions::new().write(true).open(path)?
    };
    file.set_modified(time)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<OsString> {
        text.split_whitespace().map(OsString::from).collect()
    }

    #[test]
    fn the_command_takes_a_tag_a_folder_and_two_options() {
        assert_eq!(
            parse(&words("a/b:c --to out --files --store s")),
            Ok(ExportCommand {
                store: Some("s".into()),
                tag: "a/b:c".into(),
                to: "out".into(),
                files: true,
            })
        );
        assert_eq!(
            parse(&words("--to out a:b")).map(|c| (c.store, c.files)),
            Ok((None, false))
        );
        for (line, reason) in [
            ("--to out", "image export requires NAME:TAG"),
            ("a:b", "image export requires --to DIR"),
            ("a:b c:d --to out", "image export takes one NAME:TAG"),
            (
                "a:b --to out --to again",
                "unknown or duplicate export option",
            ),
            ("a:b --to out --files --files", "--files is accepted once"),
            ("a:b --to", "missing export option value"),
            (
                "a:b --to out --bogus x",
                "unknown or duplicate export option",
            ),
        ] {
            assert_eq!(parse(&words(line)), Err(reason), "{line}");
        }
    }

    fn node(path: &str, contents: Option<&[u8]>) -> reader::Node {
        reader::Node {
            path: path.into(),
            short_name: path.rsplit('/').next().unwrap().to_ascii_uppercase(),
            attributes: if contents.is_some() { 0x20 } else { 0x10 },
            time: None,
            contents: contents.map(<[u8]>::to_vec),
        }
    }

    #[test]
    fn volumes_merge_into_one_tree_in_the_first_spelling() {
        let merged = merge(vec![
            (
                "A.IMG".into(),
                vec![node("Game", None), node("Game/One.dat", Some(b"1"))],
            ),
            (
                "B.IMG".into(),
                vec![
                    node("GAME", None),
                    node("GAME/One.dat", Some(b"1")),
                    node("GAME/TWO.DAT", Some(b"2")),
                    node("GAME/SUB", None),
                    node("GAME/SUB/X", Some(b"x")),
                ],
            ),
        ])
        .unwrap();
        let paths: Vec<&str> = merged.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Game",
                "Game/One.dat",
                "Game/TWO.DAT",
                "Game/SUB",
                "Game/SUB/X"
            ]
        );
        let clash = |second: reader::Node| {
            merge(vec![
                ("A.IMG".into(), vec![node("X", Some(b"1"))]),
                ("B.IMG".into(), vec![second]),
            ])
            .unwrap_err()
            .to_string()
        };
        assert_eq!(
            clash(node("x", Some(b"2"))),
            "--files: A.IMG and B.IMG both hold x with other bytes; export without --files"
        );
        assert_eq!(
            clash(node("X", None)),
            "--files: X is a file on one of A.IMG and B.IMG and a folder on the other; export without --files"
        );
        assert!(
            merge(vec![("A.IMG".into(), vec![node("CON.TXT", Some(b""))])])
                .unwrap_err()
                .to_string()
                .contains("device name")
        );
    }

    #[test]
    fn two_streams_compare_by_their_bytes() {
        let long = vec![7u8; 200_000];
        assert!(same_stream(&long[..], &long[..]).unwrap());
        assert!(!same_stream(&long[..], &long[..199_999]).unwrap());
        assert!(!same_stream(&long[..199_999], &long[..]).unwrap());
        let mut other = long.clone();
        other[150_000] = 8;
        assert!(!same_stream(&long[..], &other[..]).unwrap());
        assert!(same_stream(&b""[..], &b""[..]).unwrap());
    }
}
