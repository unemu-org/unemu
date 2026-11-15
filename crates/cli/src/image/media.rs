// SPDX-License-Identifier: AGPL-3.0-or-later
//! Installation-set CLI import and inspection; machine-images §5.2.
//! Sources are copied by the store's streaming, full-SHA-256 ingestion path.

use anyhow::Context;
use image_store::{
    media_set::{Config, Extra, ExtraKind, MediaFormats, Medium, Validator},
    media_type as mt,
    metadata::{Metadata, Title},
    Descriptor, Manifest, Store,
};
use pc_image::media_set::{self as pc, PcMediaFormats};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::Write,
    path::PathBuf,
};

#[derive(Debug, PartialEq, Eq)]
pub struct MediaCommand {
    action: String,
    store: PathBuf,
    create: bool,
    dry_run: bool,
    no_compress: bool,
    tag: String,
    title: String,
    files: Vec<PathBuf>,
    labels: BTreeMap<usize, String>,
    formats: BTreeMap<usize, String>,
    extras: Vec<(PathBuf, Option<String>)>,
}

pub fn parse(args: &[OsString]) -> Result<MediaCommand, &'static str> {
    let action = args
        .first()
        .and_then(|s| s.to_str())
        .ok_or("expected image media import, ls or show")?;
    if !matches!(action, "import" | "ls" | "show") {
        return Err("expected image media import, ls or show");
    }
    let mut command = MediaCommand {
        action: action.into(),
        store: PathBuf::new(),
        create: false,
        dry_run: false,
        no_compress: false,
        tag: String::new(),
        title: String::new(),
        files: Vec::new(),
        labels: BTreeMap::new(),
        formats: BTreeMap::new(),
        extras: Vec::new(),
    };
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        if arg == "--no-compress" && action == "import" && !command.no_compress {
            command.no_compress = true;
            continue;
        }
        if arg == "--dry-run" && action == "import" && !command.dry_run {
            command.dry_run = true;
            continue;
        }
        if arg == "--create" && action == "import" && !command.create {
            command.create = true;
            continue;
        }
        if !arg.as_encoded_bytes().starts_with(b"--") {
            if action == "import" {
                command.files.push(arg.into());
            } else if action == "show" && command.tag.is_empty() {
                command.tag = arg.to_str().ok_or("invalid tag")?.into();
            } else {
                return Err("unexpected positional argument");
            }
            continue;
        }
        let value = args
            .next()
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing media option value")?;
        match arg.to_str() {
            Some("--store") if command.store.as_os_str().is_empty() => command.store = value.into(),
            Some("--title") if action == "import" && command.title.is_empty() => {
                command.title = value.to_str().ok_or("invalid title")?.into()
            }
            Some("--tag") if action == "import" && command.tag.is_empty() => {
                command.tag = value.to_str().ok_or("invalid tag")?.into()
            }
            Some(option @ ("--label" | "--format")) if action == "import" => {
                let (number, text) = value
                    .to_str()
                    .and_then(|s| s.split_once('='))
                    .ok_or("expected N=TEXT (one-based index)")?;
                let number: usize = number.parse().map_err(|_| "invalid medium index")?;
                if number == 0 || text.is_empty() {
                    return Err("invalid medium index or value");
                }
                let map = if option == "--label" {
                    &mut command.labels
                } else {
                    &mut command.formats
                };
                if map.insert(number, text.into()).is_some() {
                    return Err("duplicate medium option");
                }
            }
            Some("--extra") if action == "import" => {
                let text = value.to_str().ok_or("invalid extra path")?;
                let (path, dos) = if let Some((path, name)) =
                    text.rsplit_once(':').filter(|(path, _)| {
                        !(path.len() == 1 && path.as_bytes()[0].is_ascii_alphabetic())
                    }) {
                    (path, Some(name.to_owned()))
                } else {
                    (text, None)
                };
                if path.is_empty() {
                    return Err("invalid extra path or DOS 8.3 name");
                }
                command.extras.push((path.into(), dos));
            }
            _ => return Err("unknown or duplicate media option"),
        }
    }
    if command.store.as_os_str().is_empty() {
        return Err("image commands require --store DIR");
    }
    if action == "import"
        && (command.files.is_empty() || command.title.trim().is_empty() || command.tag.is_empty())
    {
        return Err("media import requires files, --title and --tag");
    }
    if action == "show" && command.tag.is_empty() {
        return Err("media show requires NAME:TAG");
    }
    Ok(command)
}

fn descriptor(kind: &str, blob: image_store::Blob) -> Descriptor {
    user_supplied(Descriptor::new(kind, blob.digest, blob.size))
}
fn user_supplied(mut d: Descriptor) -> Descriptor {
    d.annotations
        .insert("unemu.redistribution".into(), "user-supplied".into());
    d
}

pub fn execute(command: MediaCommand, output: &mut impl Write) -> anyhow::Result<()> {
    if command.action == "import" {
        return import(command, output);
    }
    let store = Store::open_existing(&command.store)?;
    match command.action.as_str() {
        "ls" => {
            let mut summaries = image_store::summary::SummaryCache::open(&store)?;
            while !summaries.pending().is_empty() {
                summaries.rebuild(&store, 64, &[], &[])?;
            }
            for tag in summaries.tags() {
                if let Some(row) = summaries.summary(&tag.manifest.digest) {
                    if row.kind == image_store::summary::ArtifactKind::MediaSet {
                        writeln!(
                            output,
                            "{}\t{}\t{}",
                            tag.name, tag.manifest.digest, row.name
                        )?;
                    }
                }
            }
        }
        "show" => {
            let image = store.resolve(&command.tag, &Validator(&PcMediaFormats))?;
            let config = Config::decode(&image.config)?;
            writeln!(
                output,
                "{}\t{}\t{}",
                command.tag, image.image.digest, config.platform
            )?;
            for (index, m) in config.media.iter().enumerate() {
                writeln!(
                    output,
                    "{}\t{}\t{}\t{}",
                    index + 1,
                    format_args!("{:?}", m.label),
                    pc::format_name(&m.format)?,
                    m.blob
                )?;
            }
            for e in config.extras {
                writeln!(
                    output,
                    "extra\t{}\t{}\t{}\t{}",
                    format_args!("{:?}", e.filename),
                    e.size,
                    e.blob,
                    e.dos_name.as_deref().unwrap_or("")
                )?;
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn import(command: MediaCommand, output: &mut impl Write) -> anyhow::Result<()> {
    Store::validate_tag_name(&command.tag)?;
    let store = if command.dry_run && !command.store.exists() {
        None
    } else {
        let store = if command.create {
            Store::open(&command.store)?
        } else {
            Store::open_existing(&command.store)?
        };
        store.check_new_tag(&command.tag)?;
        Some(store)
    };
    let mut files = command.files.clone();
    if files.len() == 1 && files[0].is_dir() {
        let directory = &files[0];
        let mut entries = std::fs::read_dir(directory)
            .with_context(|| format!("directory {:?}", directory))?
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| format!("directory {:?}", directory))?;
        entries.sort_by_key(|e| e.file_name());
        let mut ordered = Vec::new();
        for entry in entries {
            let path = entry.path();
            if entry.file_name().as_encoded_bytes().starts_with(b".") {
                writeln!(output, "skipped hidden file: {:?}", entry.file_name())?;
                continue;
            }
            let stat = std::fs::metadata(&path).with_context(|| format!("source {:?}", path))?;
            anyhow::ensure!(stat.is_file(), "source {:?}: expected a regular file", path);
            let extension = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            anyhow::ensure!(
                matches!(extension.as_str(), "img" | "ima" | "iso")
                    || pc::detect(stat.len()).is_some(),
                "source {:?}: not a media candidate; use an explicit file list",
                path
            );
            ordered.push(path);
        }
        files = ordered;
    }
    anyhow::ensure!(
        !files.is_empty(),
        "source list {:?}: no media files",
        command.files
    );
    anyhow::ensure!(
        command
            .labels
            .keys()
            .chain(command.formats.keys())
            .all(|n| *n <= files.len()),
        "source list {:?}: medium option index out of range",
        files
    );
    anyhow::ensure!(
        image_store::media_set::valid_text(&command.title),
        "invalid title {:?}",
        command.title
    );
    let empty = image_store::Digest::of(b"");
    let mut config = Config {
        schema: 1,
        platform: "ibm-pc-at".into(),
        media: Vec::new(),
        extras: Vec::new(),
    };
    let mut kinds = Vec::new();
    let mut sizes = Vec::new();
    for (index, path) in files.iter().enumerate() {
        let stat = std::fs::metadata(path).with_context(|| format!("source {:?}", path))?;
        anyhow::ensure!(stat.is_file(), "source {:?}: expected a regular file", path);
        let name = command
            .formats
            .get(&(index + 1))
            .map(String::as_str)
            .or_else(|| pc::detect(stat.len()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "source {:?}: unknown media size; use --format N=FORMAT",
                    path
                )
            })?;
        let format = pc::format(name).with_context(|| format!("source {:?}", path))?;
        kinds.push(
            PcMediaFormats
                .validate_format(&format, stat.len())
                .with_context(|| format!("source {:?}", path))?,
        );
        sizes.push(stat.len());
        let label = command
            .labels
            .get(&(index + 1))
            .cloned()
            .unwrap_or_else(|| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        anyhow::ensure!(
            image_store::media_set::valid_text(&label),
            "source {:?}: invalid label {:?}",
            path,
            label
        );
        config.media.push(Medium {
            id: format!("disk-{}", index + 1),
            label,
            format,
            blob: empty.clone(),
        });
    }
    for (path, dos_name) in &command.extras {
        let stat = std::fs::metadata(path).with_context(|| format!("extra {:?}", path))?;
        anyhow::ensure!(stat.is_file(), "extra {:?}: expected a regular file", path);
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("extra {:?}: filename must be UTF-8", path))?
            .to_owned();
        anyhow::ensure!(
            image_store::media_set::valid_text(&filename),
            "extra {:?}: invalid filename",
            path
        );
        if let Some(name) = dos_name {
            PcMediaFormats
                .validate_name(name)
                .with_context(|| format!("extra {:?}", path))?;
        }
        let extension = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let kind = match extension.as_str() {
            "txt" | "pdf" | "md" | "doc" => ExtraKind::Document,
            "zip" | "7z" | "arj" | "rar" => ExtraKind::Archive,
            "exe" | "com" | "bat" => ExtraKind::Executable,
            _ => ExtraKind::Other,
        };
        config.extras.push(Extra {
            blob: empty.clone(),
            filename,
            size: stat.len(),
            kind,
            dos_name: dos_name.clone(),
        });
    }
    config
        .encode()
        .with_context(|| format!("source list {:?}, extras {:?}", files, command.extras))?;
    let mut metadata = Metadata::new(&config.platform);
    metadata.titles.push(Title {
        id: "title".into(),
        title: command.title.clone(),
        ..Title::default()
    });
    metadata.encode(&[])?;
    writeln!(output, "media order:")?;
    for (index, path) in files.iter().enumerate() {
        writeln!(
            output,
            "{}\t{:?}\t{:?}\t{}",
            index + 1,
            config.media[index].label,
            path,
            pc::format_name(&config.media[index].format)?
        )?;
    }
    for extra in &config.extras {
        writeln!(output, "extra\t{:?}\t{:?}", extra.filename, extra.dos_name)?;
    }
    if command.dry_run {
        writeln!(output, "dry run; no blobs or tag written")?;
        return Ok(());
    }
    let store = store.ok_or_else(|| anyhow::anyhow!("store unavailable"))?;
    let codecs = if command.no_compress {
        &[]
    } else {
        image_store::codec::MEDIA_PAYLOAD_TYPES
    };
    let mut layers = Vec::new();
    for (index, path) in files.iter().enumerate() {
        let file = std::fs::File::open(path).with_context(|| format!("source {:?}", path))?;
        let blob = store
            .import_payload(file, kinds[index], sizes[index], None, codecs)
            .with_context(|| format!("source {:?}", path))?;
        anyhow::ensure!(
            blob.decoded_size()? == sizes[index],
            "source {:?}: length changed during import",
            path
        );
        config.media[index].blob = blob.decoded_digest()?;
        layers.push(user_supplied(blob));
    }
    for (index, (path, _)) in command.extras.iter().enumerate() {
        let file = std::fs::File::open(path).with_context(|| format!("extra {:?}", path))?;
        let blob = store
            .import_payload(file, mt::EXTRA, config.extras[index].size, None, codecs)
            .with_context(|| format!("extra {:?}", path))?;
        anyhow::ensure!(
            blob.decoded_size()? == config.extras[index].size,
            "extra {:?}: length changed during import",
            path
        );
        config.extras[index].blob = blob.decoded_digest()?;
        layers.push(user_supplied(blob));
    }
    let mut seen = BTreeSet::new();
    let mut unique = Vec::<Descriptor>::new();
    for d in layers {
        if seen.insert(d.digest.clone()) {
            unique.push(d);
        } else {
            anyhow::ensure!(
                unique.iter().any(|p| p.digest == d.digest
                    && p.media_type == d.media_type
                    && p.size == d.size),
                "source list {:?}: identical bytes used with conflicting payload types",
                files
            );
        }
    }
    let bytes = metadata.encode(&unique)?;
    unique.push(descriptor(
        mt::METADATA,
        store.import(bytes.as_slice(), None)?,
    ));
    let bytes = config.encode()?;
    let cfg = descriptor(mt::MEDIA_SET_CONFIG, store.import(bytes.as_slice(), None)?);
    let mut manifest = Manifest::new_media_set(&config.platform, cfg, unique)?;
    manifest
        .annotations
        .insert("org.opencontainers.image.title".into(), command.title);
    let manifest = store.import_manifest(&manifest)?;
    store.update_tag(&command.tag, &manifest, None, &Validator(&PcMediaFormats))?;
    writeln!(output, "{}\t{}", command.tag, manifest.digest)?;
    Ok(())
}
