// SPDX-License-Identifier: AGPL-3.0-or-later
//! Installation-set CLI import and inspection; machine-images §5.2.
//! Validation and publication are shared with the desktop library through
//! `pc_image::media_import`; sources are copied by the store's streaming,
//! full-SHA-256 ingestion path.

use image_store::{
    media_set::{Config, Validator},
    metadata::Title,
    Store,
};
use pc_image::{
    media_import,
    media_set::{self as pc, PcMediaFormats},
};
use std::{collections::BTreeMap, ffi::OsString, io::Write, path::PathBuf};

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
        // The command line never moves an existing tag, even to identical bytes.
        store.check_new_tag(&command.tag)?;
        Some(store)
    };
    let mut files = command.files.clone();
    if files.len() == 1 && files[0].is_dir() {
        let listing = media_import::list_folder(&files[0]);
        for name in &listing.hidden {
            writeln!(output, "skipped hidden file: {:?}", name)?;
        }
        files = listing.files?;
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
    let request = media_import::Request {
        tag: command.tag.clone(),
        title: Title {
            title: command.title.clone(),
            ..Title::default()
        },
        labels: Vec::new(),
        media: files
            .iter()
            .enumerate()
            .map(|(index, path)| media_import::Source {
                path: path.clone(),
                label: command.labels.get(&(index + 1)).cloned(),
                format: command.formats.get(&(index + 1)).cloned(),
            })
            .collect(),
        extras: command
            .extras
            .iter()
            .map(|(path, dos_name)| media_import::ExtraSource {
                path: path.clone(),
                dos_name: dos_name.clone(),
                kind: None,
            })
            .collect(),
        compress: !command.no_compress,
    };
    let plan = media_import::plan(&request)?;
    writeln!(output, "media order:")?;
    for (index, (path, medium)) in plan.media().enumerate() {
        writeln!(
            output,
            "{}\t{:?}\t{:?}\t{}",
            index + 1,
            medium.label,
            path,
            pc::format_name(&medium.format)?
        )?;
    }
    for extra in &plan.config().extras {
        writeln!(output, "extra\t{:?}\t{:?}", extra.filename, extra.dos_name)?;
    }
    if command.dry_run {
        writeln!(output, "dry run; no blobs or tag written")?;
        return Ok(());
    }
    let store = store.ok_or_else(|| anyhow::anyhow!("store unavailable"))?;
    let outcome = media_import::publish(&store, &plan, &|_, _| {}, &|| false)?;
    writeln!(output, "{}\t{}", command.tag, outcome.descriptor().digest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_folder_still_reports_the_hidden_files_skipped_before_it() {
        let root = std::env::temp_dir().join(format!("cli-media-refused-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let folder = root.join("disks");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(".hidden"), b"hidden").unwrap();
        std::fs::write(folder.join("a.img"), vec![0; 368_640]).unwrap();
        std::fs::write(folder.join("notes.txt"), b"not a disk").unwrap();
        let args: Vec<OsString> = [
            "import".into(),
            folder.clone().into_os_string(),
            "--title".into(),
            "Refused".into(),
            "--tag".into(),
            "test:refused".into(),
            "--store".into(),
            root.join("store").into_os_string(),
            "--dry-run".into(),
        ]
        .into();
        let mut output = Vec::new();
        let error = execute(parse(&args).unwrap(), &mut output).unwrap_err();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "skipped hidden file: \".hidden\"\n"
        );
        assert_eq!(
            format!("{error:#}"),
            format!(
                "source {:?}: not a media candidate; use an explicit file list",
                folder.join("notes.txt")
            )
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
