// SPDX-License-Identifier: AGPL-3.0-or-later
//! Installation-set CLI import and inspection; machine-images §5.2.
//! Validation and publication are shared with the desktop library through
//! `pc_image::media_import`; sources are copied by the store's streaming,
//! full-SHA-256 ingestion path. A set is user-supplied unless `--open` states
//! its licence and names the extra that holds the licence's text
//! (`--license-text`): the vendor's terms as `--license LicenseRef-NAME`, or
//! an SPDX licence expression together with the extra that holds the set's
//! corresponding source (`--source`); nothing is guessed. Set-level
//! catalogue flags fill the set's one title record (machine-images §2.1), with
//! the year and software-platform rules the shared import applies to the
//! library's dialog too, and the set's own labels, so a recipe can state
//! everything `pc_image::template::build_media` states for an installation
//! set.

use image_registry::guard;
use image_store::{
    license,
    media_set::{valid_text, Config, Validator},
    media_type,
    metadata::{Metadata, Title},
    Store,
};
use pc_image::{
    firmware::catalogue,
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
    /// `--title` and the set-level catalogue flags; absent fields stay unknown.
    /// Boxed: a full title record would dwarf every other image command.
    title: Box<Title>,
    /// `--set-label`: the set's labels in the order given.
    set_labels: Vec<String>,
    files: Vec<PathBuf>,
    labels: BTreeMap<usize, String>,
    formats: BTreeMap<usize, String>,
    extras: Vec<(PathBuf, Option<String>)>,
    /// `--open`: the user states the set may be redistributed.
    open: bool,
    license: Option<String>,
    /// `--license-text`: the file name of the extra with the licence's text.
    license_text: Option<String>,
    /// `--source`: the file name of the extra with the set's source archive.
    source: Option<String>,
    /// `--min-app-version`: raise the set's minimum release.
    min_app_version: Option<image_store::compat::AppVersion>,
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
        title: Box::default(),
        set_labels: Vec::new(),
        files: Vec::new(),
        labels: BTreeMap::new(),
        formats: BTreeMap::new(),
        extras: Vec::new(),
        open: false,
        license: None,
        license_text: None,
        source: None,
        min_app_version: None,
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
        if arg == "--open" && action == "import" && !command.open {
            command.open = true;
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
            Some("--title") if action == "import" && command.title.title.is_empty() => {
                command.title.title = value.to_str().ok_or("invalid title")?.into()
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
            Some(option @ ("--publisher" | "--developer" | "--genre")) if action == "import" => {
                let values = match option {
                    "--publisher" => &mut command.title.publishers,
                    "--developer" => &mut command.title.developers,
                    _ => &mut command.title.genres,
                };
                values.push(catalogue_text(value)?);
            }
            Some("--year") if action == "import" && command.title.release_year.is_none() => {
                command.title.release_year = Some(
                    value
                        .to_str()
                        .and_then(|s| s.trim().parse().ok())
                        .ok_or("--year expects a whole number from 1 to 9999")?,
                )
            }
            Some("--platform") if action == "import" && command.title.platform.is_none() => {
                command.title.platform = Some(catalogue_text(value)?)
            }
            Some("--description") if action == "import" && command.title.description.is_none() => {
                command.title.description = Some(catalogue_text(value)?)
            }
            Some("--version") if action == "import" && command.title.version.is_none() => {
                command.title.version = Some(catalogue_text(value)?)
            }
            Some("--edition") if action == "import" && command.title.edition.is_none() => {
                command.title.edition = Some(catalogue_text(value)?)
            }
            Some("--set-label") if action == "import" => {
                let label = catalogue_text(value)?;
                // The dialog's rule for its Labels field and for disk labels.
                if !valid_text(&label) {
                    return Err("invalid set label: at most 128 characters, no control characters");
                }
                // Labels keep their order and are never deduplicated later,
                // so a repeat would only give the same labels another digest.
                if command.set_labels.contains(&label) {
                    return Err("duplicate set label");
                }
                command.set_labels.push(label);
            }
            Some("--license") if action == "import" && command.license.is_none() => {
                command.license = Some(value.to_str().ok_or("invalid licence")?.into())
            }
            Some("--license-text") if action == "import" && command.license_text.is_none() => {
                command.license_text = Some(value.to_str().ok_or("invalid terms text name")?.into())
            }
            Some("--source") if action == "import" && command.source.is_none() => {
                command.source = Some(value.to_str().ok_or("invalid source archive name")?.into())
            }
            Some("--min-app-version")
                if action == "import" && command.min_app_version.is_none() =>
            {
                command.min_app_version = Some(super::min_app_version(value)?)
            }
            _ => return Err("unknown or duplicate media option"),
        }
    }
    if (command.license.is_some() || command.license_text.is_some()) && !command.open {
        return Err("--license and --license-text state the terms of an open set; add --open");
    }
    if command.source.is_some() && !command.open {
        return Err("--source names the source archive of an open set; add --open");
    }
    if command.open && (command.license.is_none() || command.license_text.is_none()) {
        return Err(
            "--open needs --license LicenseRef-NAME and --license-text EXTRA: a set's terms are stated, never guessed (an SPDX expression also needs --source EXTRA)",
        );
    }
    if command.store.as_os_str().is_empty() {
        return Err("image commands require --store DIR");
    }
    if action == "import"
        && (command.files.is_empty()
            || command.title.title.trim().is_empty()
            || command.tag.is_empty())
    {
        return Err("media import requires files, --title and --tag");
    }
    if action == "show" && command.tag.is_empty() {
        return Err("media show requires NAME:TAG");
    }
    Ok(command)
}

/// A set-level metadata value as the library's import dialog keeps it:
/// trimmed, and never blank.
fn catalogue_text(value: &OsString) -> Result<String, &'static str> {
    let text = value.to_str().ok_or("invalid set metadata value")?.trim();
    if text.is_empty() {
        return Err("blank set metadata value");
    }
    Ok(text.into())
}

/// The catalogue fields and labels a set states, one line per value in
/// stated order; nothing for a field left unknown.
fn write_catalogue(
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
    for label in labels {
        writeln!(output, "set-label\t{label:?}")?;
    }
    Ok(())
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
        "show" => show(&store, &command.tag, output)?,
        _ => unreachable!(),
    }
    Ok(())
}

/// One set in full: media, extras, its catalogue fields, the terms text and
/// source archive its layers link, every layer's class, licence and terms
/// link, and where its marks allow it to be pushed.
fn show(store: &Store, tag: &str, output: &mut impl Write) -> anyhow::Result<()> {
    let image = store.resolve(tag, &Validator(&PcMediaFormats))?;
    let config = Config::decode(&image.config)?;
    writeln!(
        output,
        "{}\t{}\t{}",
        tag, image.image.digest, config.platform
    )?;
    super::write_min_app_version(output, &image.manifest)?;
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
    let marks = super::registry::guard_marks(store, &image.manifest, catalogue())?;
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
        title: (*command.title).clone(),
        labels: command.set_labels.clone(),
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
    let open = match (&command.license, &command.license_text) {
        (Some(license), Some(terms)) if command.open => Some(media_import::OpenStatement {
            license: license.clone(),
            terms: terms.clone(),
            source: command.source.clone(),
        }),
        _ => None,
    };
    let mut plan = media_import::plan_with(&request, open.as_ref())?;
    if let Some(stated) = command.min_app_version {
        plan.raise_min_app_version(stated)?;
    }
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
    write_catalogue(output, &command.title, &command.set_labels)?;
    if let Some((license, terms)) = plan.open() {
        writeln!(output, "open\t{license}\tterms\t{:?}", terms.filename)?;
    }
    if let Some(source) = plan.source() {
        writeln!(output, "source\t{:?}", source.filename)?;
    }
    if let Some(stated) = plan.min_app_version() {
        writeln!(output, "min-app-version\t{stated}")?;
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

    fn parse_import(flags: &[&str]) -> Result<MediaCommand, &'static str> {
        let mut args: Vec<OsString> = [
            "import", "disk.img", "--title", "Set", "--tag", "test:set", "--store", "store",
        ]
        .map(OsString::from)
        .into();
        args.extend(flags.iter().map(OsString::from));
        parse(&args)
    }

    #[test]
    fn set_labels_keep_the_order_given_and_version_and_edition_fill_the_title() {
        let command = parse_import(&[
            "--set-label",
            "operating-system",
            "--version",
            " 1.4 ",
            "--set-label",
            "freedos",
            "--edition",
            "Floppy Edition",
            "--set-label",
            " open-source ",
        ])
        .unwrap();
        assert_eq!(
            command.set_labels,
            ["operating-system", "freedos", "open-source"]
        );
        assert_eq!(command.title.version.as_deref(), Some("1.4"));
        assert_eq!(command.title.edition.as_deref(), Some("Floppy Edition"));
    }

    #[test]
    fn min_app_version_takes_one_release_version() {
        let command = parse_import(&["--min-app-version", "0.0.2"]).unwrap();
        assert_eq!(
            command.min_app_version,
            Some(image_store::compat::AppVersion::new(0, 0, 2))
        );
        assert!(parse_import(&[]).unwrap().min_app_version.is_none());
        for bad in ["1.0", "0.0.2-rc.1", "x"] {
            assert!(parse_import(&["--min-app-version", bad]).is_err(), "{bad}");
        }
        assert!(
            parse_import(&["--min-app-version", "0.0.2", "--min-app-version", "0.0.2"]).is_err()
        );
    }

    #[test]
    fn without_the_new_flags_the_command_is_unchanged() {
        let command = parse_import(&[]).unwrap();
        assert!(command.set_labels.is_empty());
        assert_eq!(
            *command.title,
            Title {
                title: "Set".into(),
                ..Title::default()
            }
        );
    }

    #[test]
    fn a_blank_repeated_or_invalid_set_label_version_or_edition_is_refused() {
        let long = "x".repeat(129);
        for (flags, reason) in [
            (vec!["--set-label", "  "], "blank set metadata value"),
            (vec!["--version", "\t"], "blank set metadata value"),
            (vec!["--edition", " "], "blank set metadata value"),
            (vec!["--set-label", ""], "missing media option value"),
            (vec!["--version", "--edition"], "missing media option value"),
            (
                vec!["--set-label", "a", "--set-label", "b", "--set-label", " a"],
                "duplicate set label",
            ),
            (
                vec!["--version", "1.4", "--version", "1.4"],
                "unknown or duplicate media option",
            ),
            (
                vec!["--edition", "A", "--edition", "B"],
                "unknown or duplicate media option",
            ),
            (
                vec!["--set-label", &long],
                "invalid set label: at most 128 characters, no control characters",
            ),
            (
                vec!["--set-label", "a\u{7f}b"],
                "invalid set label: at most 128 characters, no control characters",
            ),
        ] {
            assert_eq!(parse_import(&flags), Err(reason), "{flags:?}");
        }
        let at_limit = "x".repeat(128);
        assert_eq!(
            parse_import(&["--set-label", &at_limit])
                .unwrap()
                .set_labels,
            [at_limit]
        );
    }

    #[test]
    fn the_set_flags_belong_to_import_only() {
        for action in ["ls", "show"] {
            for flag in ["--set-label", "--version", "--edition"] {
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
