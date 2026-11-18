// SPDX-License-Identifier: AGPL-3.0-or-later
//! `image media ls` and `show`: the installation sets of a store, and one
//! set in full; machine-images §5.2. A set is read with its platform's
//! validator, its formats are named by its platform, and the push line is
//! the guard's own classification with the platform's restrictions
//! ([`super::super::CliPlatform`]). The platforms' import commands write the
//! same catalogue lines ([`write_catalogue`]) when they plan a set.

use super::super::CliPlatform;
use image_registry::guard;
use image_store::{
    license,
    media_set::Config,
    media_type,
    metadata::{Metadata, Title},
    Store,
};
use std::{ffi::OsString, io::Write, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum MediaCommand {
    List { store: PathBuf },
    Show { store: PathBuf, tag: String },
}

/// Parse the words after `image media`: `Ok(None)` for `import`, a
/// platform's.
pub fn parse(args: &[OsString]) -> Result<Option<MediaCommand>, &'static str> {
    let action = args
        .first()
        .and_then(|s| s.to_str())
        .ok_or("expected image media import, ls or show")?;
    match action {
        "import" => return Ok(None),
        "ls" | "show" => (),
        _ => return Err("expected image media import, ls or show"),
    }
    // An empty operand names no set, so the next one still can.
    let (mut store, mut tag) = (None, String::new());
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        if !arg.as_encoded_bytes().starts_with(b"--") {
            if action == "show" && tag.is_empty() {
                tag = arg.to_str().ok_or("invalid tag")?.into();
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
            Some("--store") if store.is_none() => store = Some(PathBuf::from(value)),
            _ => return Err("unknown or duplicate media option"),
        }
    }
    let store = store.ok_or("image commands require --store DIR")?;
    Ok(Some(match action {
        "show" if tag.is_empty() => return Err("media show requires NAME:TAG"),
        "show" => MediaCommand::Show { store, tag },
        _ => MediaCommand::List { store },
    }))
}

/// A set-level metadata value as the library's import dialog keeps it:
/// trimmed, and never blank.
pub fn catalogue_text(value: &OsString) -> Result<String, &'static str> {
    let text = value.to_str().ok_or("invalid set metadata value")?.trim();
    if text.is_empty() {
        return Err("blank set metadata value");
    }
    Ok(text.into())
}

/// The catalogue fields and labels a set states, one line per value in
/// stated order (needs and work ids by kind); nothing for a field left
/// unknown.
pub fn write_catalogue(
    output: &mut impl Write,
    title: &Title,
    labels: &[String],
) -> std::io::Result<()> {
    for publisher in &title.publishers {
        writeln!(output, "publisher\t{publisher:?}")?;
    }
    for developer in &title.developers {
        writeln!(output, "developer\t{developer:?}")?;
    }
    if let Some(year) = title.release_year {
        writeln!(output, "year\t{year}")?;
    }
    for genre in &title.genres {
        writeln!(output, "genre\t{genre:?}")?;
    }
    if let Some(platform) = &title.platform {
        writeln!(output, "platform\t{platform:?}")?;
    }
    if let Some(description) = &title.description {
        writeln!(output, "description\t{description:?}")?;
    }
    if let Some(version) = &title.version {
        writeln!(output, "version\t{version:?}")?;
    }
    if let Some(edition) = &title.edition {
        writeln!(output, "edition\t{edition:?}")?;
    }
    if let Some(requirements) = &title.requirements {
        writeln!(output, "requirements\t{requirements:?}")?;
    }
    for (kind, values) in &title.needs {
        writeln!(output, "need\t{kind}\t{}", values.join(","))?;
    }
    if let Some(work) = &title.work_id {
        writeln!(output, "work\t{work:?}")?;
    }
    if let Some(synopsis) = &title.synopsis {
        writeln!(output, "synopsis\t{synopsis:?}")?;
    }
    for (kind, id) in &title.work_ids {
        writeln!(output, "work-id\t{kind}\t{id:?}")?;
    }
    for label in labels {
        writeln!(output, "set-label\t{label:?}")?;
    }
    Ok(())
}

pub fn execute(
    command: MediaCommand,
    output: &mut impl Write,
    platforms: &[&dyn CliPlatform],
) -> anyhow::Result<()> {
    match command {
        MediaCommand::List { store } => {
            let store = Store::open_existing(&store)?;
            let mut summaries = image_store::summary::SummaryCache::open(&store)?;
            while !summaries.pending().is_empty() {
                summaries.rebuild(&store, 64, &[], &[])?;
            }
            for tag in summaries.tags() {
                if let Some(row) = summaries.summary(&tag.manifest.digest) {
                    if row.kind == image_store::summary::ArtifactKind::MediaSet {
                        writeln!(
                            output,
                            "{}\t{}\t{}{}",
                            tag.name,
                            tag.manifest.digest,
                            row.name,
                            super::requirement_columns(&store, &tag.manifest)
                        )?;
                    }
                }
            }
        }
        MediaCommand::Show { store, tag } => {
            let store = Store::open_existing(&store)?;
            show(&store, &tag, output, platforms)?
        }
    }
    Ok(())
}

/// One set in full: media, extras, its catalogue fields, the terms text and
/// source archive its layers link, every layer's class, licence and terms
/// link, and where its marks allow it to be pushed.
fn show(
    store: &Store,
    tag: &str,
    output: &mut impl Write,
    platforms: &[&dyn CliPlatform],
) -> anyhow::Result<()> {
    let (platform, validator) = media_sets(platforms)?;
    let image = store.resolve(tag, validator)?;
    let config = Config::decode(&image.config)?;
    // Machine-readable: the platform id, by the id it is written with today.
    writeln!(
        output,
        "{}\t{}\t{}",
        tag,
        image.image.digest,
        image_store::platform::canonical(&config.platform)
    )?;
    super::write_min_app_version(output, &image.manifest)?;
    for (index, m) in config.media.iter().enumerate() {
        writeln!(
            output,
            "{}\t{}\t{}\t{}",
            index + 1,
            format_args!("{:?}", m.label),
            platform.medium_format(&m.format)?,
            m.blob
        )?;
    }
    for e in &config.extras {
        writeln!(
            output,
            "extra\t{}\t{}\t{}\t{}",
            format_args!("{:?}", e.filename),
            e.size,
            e.blob,
            e.dos_name.as_deref().unwrap_or("")
        )?;
    }
    let metadata = Metadata::decode_for_display(&image.metadata, &image.manifest.layers)?;
    if let Some(title) = metadata.titles.first() {
        write_catalogue(output, title, &metadata.labels)?;
    }
    let link = |d: &image_store::Descriptor| d.annotations.get(license::TERMS).cloned();
    for (what, key) in [("terms", license::TERMS), ("source", license::SOURCE)] {
        let mut linked: Vec<&String> = Vec::new();
        for text in image
            .manifest
            .layers
            .iter()
            .filter_map(|d| d.annotations.get(key))
        {
            if !linked.contains(&text) {
                linked.push(text);
            }
        }
        for text in linked {
            let extra = config.extras.iter().find(|e| &e.blob.to_string() == text);
            writeln!(
                output,
                "{what}\t{}\t{text}",
                extra.map_or_else(|| "-".into(), |e| format!("{:?}", e.filename))
            )?;
        }
    }
    let class = |d: &image_store::Descriptor| {
        let terms = link(d).unwrap_or_else(|| "-".into());
        match license::open_license(d) {
            Some(l) => format!("open\t{l}\t{terms}"),
            None => format!("user-supplied\t-\t{terms}"),
        }
    };
    writeln!(
        output,
        "layer\tconfig\t{}\t{}",
        image.manifest.config.digest,
        class(&image.manifest.config)
    )?;
    for d in &image.manifest.layers {
        let kind = match d.decoded_media_type() {
            media_type::FLOPPY | media_type::OPTICAL => "medium",
            media_type::METADATA => "metadata",
            media_type::EXTRA => "extra",
            _ => "other",
        };
        writeln!(
            output,
            "layer\t{kind}\t{}\t{}",
            d.decoded_digest()?,
            class(d)
        )?;
    }
    // The guard's own classification: local marks and the catalogue included.
    let marks = super::guard_marks(store, &image.manifest, platforms)?;
    let closure = guard::closure(&image.manifest, &marks);
    let restricted = closure
        .iter()
        .any(|c| c.effective == guard::Redistribution::UserSupplied);
    writeln!(
        output,
        "push\t{}",
        if restricted {
            "private registries outside unemu/ only: user-supplied layers"
        } else if !guard::copyleft_without_source(&closure).is_empty() {
            "private registries outside unemu/ only: open copyleft media without the set's source archive"
        } else if !guard::terms_without_text(&closure).is_empty() {
            "private registries outside unemu/ only: open media without the set's terms text"
        } else {
            "any configured registry, including unemu/"
        }
    )?;
    Ok(())
}

/// The platform that reads installation sets, and its validator for them.
fn media_sets<'p>(
    platforms: &[&'p dyn CliPlatform],
) -> anyhow::Result<(&'p dyn CliPlatform, &'p dyn image_store::ConfigValidator)> {
    let platform = super::super::platform_of(platforms)?;
    let validator = platform
        .validators()
        .into_iter()
        .find(|v| v.artifact_type() == media_type::MEDIA_SET)
        .ok_or_else(|| anyhow::anyhow!("{} reads no installation sets", platform.platform()))?;
    Ok((platform, validator))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_set_flags_belong_to_import_only() {
        for action in ["ls", "show"] {
            for flag in [
                "--set-label",
                "--version",
                "--edition",
                "--need",
                "--requirements",
                "--work",
                "--synopsis",
                "--work-id",
            ] {
                let mut args: Vec<OsString> = vec![action.into()];
                if action == "show" {
                    args.push("test:set".into());
                }
                args.extend([flag, "value", "--store", "store"].map(OsString::from));
                assert_eq!(
                    parse(&args),
                    Err("unknown or duplicate media option"),
                    "{action} {flag}"
                );
            }
        }
    }
}
