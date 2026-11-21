// SPDX-License-Identifier: AGPL-3.0-or-later
//! `image firmware import` and `edit`: firmware packages from ROM files and
//! their metadata revisions; machine-images §5.3.
//! Folder hashing shares the persistent ROM index; sources remain untouched.
//! Licences are explicit statements: `--open` needs `--license`, and the
//! catalogue's own classification of a ROM always wins.

use cli_core::{image::min_app_version, HostSources};
use image_store::{
    firmware::{Provenance, ProvenanceKind},
    media_type, Digest, Store,
};
use pc_image::{
    firmware::catalogue,
    firmware_import::{self, Attachment, ReadmeChoice, Statement},
};
use std::{ffi::OsString, io::Write, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub struct FirmwareCommand {
    action: String,
    store: PathBuf,
    files: Vec<PathBuf>,
    tag: String,
    create: bool,
    dry_run: bool,
    rescan: bool,
    selected: Option<String>,
    extras: Vec<(PathBuf, Option<String>)>,
    /// `--unlisted ID --role ROLE`: one ROM without a catalogue entry.
    unlisted: Option<(String, String)>,
    /// `edit --to`: the new revision's tag.
    target: Option<String>,
    statement: Box<Statement>,
}

/// Provenance flags gathered before they form one [`Provenance`].
#[derive(Default)]
struct ProvenanceFlags {
    kind: Option<(ProvenanceKind, String)>,
    revision: Option<String>,
    url: Option<String>,
    date: Option<String>,
}

fn text(value: &OsString) -> Result<String, &'static str> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or("firmware option values must be UTF-8")
}
fn once(slot: &mut Option<String>, value: String) -> Result<(), &'static str> {
    if slot.replace(value).is_some() {
        return Err("duplicate firmware option");
    }
    Ok(())
}
fn push(list: &mut Option<Vec<String>>, value: String) {
    list.get_or_insert_with(Vec::new).push(value);
}
fn flag(slot: &mut bool) -> Result<(), &'static str> {
    if std::mem::replace(slot, true) {
        return Err("duplicate firmware option");
    }
    Ok(())
}

pub fn parse(args: &[OsString]) -> Result<FirmwareCommand, &'static str> {
    let action = args
        .first()
        .and_then(|s| s.to_str())
        .filter(|s| matches!(*s, "import" | "edit"))
        .ok_or("expected image firmware import, edit, ls or show")?;
    let mut command = FirmwareCommand {
        action: action.into(),
        store: PathBuf::new(),
        files: Vec::new(),
        tag: String::new(),
        create: false,
        dry_run: false,
        rescan: false,
        selected: None,
        extras: Vec::new(),
        unlisted: None,
        target: None,
        statement: Box::default(),
    };
    let describes = matches!(action, "import" | "edit");
    let mut provenance = ProvenanceFlags::default();
    let mut role = None;
    let mut unlisted = None;
    let mut extra_licenses: Vec<Option<String>> = Vec::new();
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        if arg.is_empty() {
            return Err("empty firmware argument");
        }
        if !arg.as_encoded_bytes().starts_with(b"--") {
            if action == "import" {
                command.files.push(arg.into());
            } else if action == "edit" && command.tag.is_empty() {
                command.tag = arg.to_str().ok_or("invalid firmware tag")?.into();
            } else {
                return Err("unexpected firmware argument");
            }
            continue;
        }
        let statement = &mut *command.statement;
        let switch = match arg.to_str() {
            Some("--create") if action == "import" => Some(&mut command.create),
            Some("--dry-run") if action == "import" => Some(&mut command.dry_run),
            Some("--rescan") if action == "import" => Some(&mut command.rescan),
            Some("--open") if describes => Some(&mut statement.open),
            Some("--user-supplied") if describes => Some(&mut statement.user_supplied),
            _ => None,
        };
        if let Some(slot) = switch {
            flag(slot)?;
            continue;
        }
        // An edit keeps a README written by hand unless asked to generate it.
        if action == "edit" && arg.to_str() == Some("--regenerate-readme") {
            if statement.readme != ReadmeChoice::Keep {
                return Err("duplicate firmware option");
            }
            statement.readme = ReadmeChoice::Generate;
            continue;
        }
        let value = args
            .next()
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing firmware option value")?;
        match arg.to_str() {
            Some("--store") if command.store.as_os_str().is_empty() => command.store = value.into(),
            Some("--for") if action == "import" && command.selected.is_none() => {
                command.selected = Some(value.to_str().ok_or("invalid catalogue id")?.into())
            }
            Some("--unlisted") if action == "import" => once(&mut unlisted, text(value)?)?,
            Some("--role") if action == "import" => once(&mut role, text(value)?)?,
            Some("--to") if action == "edit" => once(&mut command.target, text(value)?)?,
            Some("--extra") if action == "import" => {
                let text = value.to_str().ok_or("invalid extra path")?;
                let (path, name) = text
                    .rsplit_once(':')
                    .filter(|(p, _)| !(p.len() == 1 && p.as_bytes()[0].is_ascii_alphabetic()))
                    .map_or((text, None), |(p, n)| (p, Some(n.to_owned())));
                if path.is_empty() {
                    return Err("invalid extra path");
                }
                command.extras.push((path.into(), name));
            }
            Some("--extra-license") if action == "import" => {
                let index = command
                    .extras
                    .len()
                    .checked_sub(1)
                    .ok_or("--extra-license follows the --extra it describes")?;
                extra_licenses.resize(index + 1, None);
                once(&mut extra_licenses[index], text(value)?)?;
            }
            Some("--title") if describes => once(&mut statement.title, text(value)?)?,
            Some("--description") if describes => once(&mut statement.description, text(value)?)?,
            Some("--notes") if describes => once(&mut statement.notes, text(value)?)?,
            Some("--vendor") if describes => once(&mut statement.vendor, text(value)?)?,
            Some("--version") if describes => once(&mut statement.version, text(value)?)?,
            Some("--date") if describes => once(&mut statement.date, text(value)?)?,
            Some("--license") if describes => once(&mut statement.license, text(value)?)?,
            Some("--source-license") if describes => {
                once(&mut statement.source_license, text(value)?)?
            }
            Some("--source") if describes && statement.source.is_none() => {
                statement.source = Some(value.into())
            }
            Some("--label") if describes => push(&mut statement.labels, text(value)?),
            Some("--board") if describes => push(&mut statement.boards, text(value)?),
            Some("--class") if describes => push(&mut statement.machine_classes, text(value)?),
            Some("--requires") if describes => push(&mut statement.requires, text(value)?),
            Some(flag @ ("--dumped-from" | "--downloaded-from" | "--built-from")) if describes => {
                let kind = match flag {
                    "--dumped-from" => ProvenanceKind::Dumped,
                    "--downloaded-from" => ProvenanceKind::Downloaded,
                    _ => ProvenanceKind::Built,
                };
                if provenance.kind.replace((kind, text(value)?)).is_some() {
                    return Err("give one of --dumped-from, --downloaded-from or --built-from");
                }
            }
            Some("--min-app-version") if describes && statement.min_app_version.is_none() => {
                statement.min_app_version = Some(min_app_version(value)?)
            }
            Some("--revision") if describes => once(&mut provenance.revision, text(value)?)?,
            Some("--url") if describes => once(&mut provenance.url, text(value)?)?,
            Some("--provenance-date") if describes => once(&mut provenance.date, text(value)?)?,
            _ => return Err("unknown or duplicate firmware option"),
        }
    }
    match provenance.kind {
        Some((kind, from)) => {
            command.statement.provenance = Some(Provenance {
                kind: Some(kind),
                from,
                revision: provenance.revision,
                url: provenance.url,
                date: provenance.date,
            })
        }
        None if provenance.revision.is_some()
            || provenance.url.is_some()
            || provenance.date.is_some() =>
        {
            return Err(
                "--revision, --url and --provenance-date need --dumped-from, --downloaded-from or --built-from",
            )
        }
        None => (),
    }
    command.statement.extra_licenses = extra_licenses;
    let statement = &command.statement;
    if statement.license.is_some() && !statement.open {
        return Err("--license states the licence of open ROMs; add --open");
    }
    if statement.open && statement.user_supplied {
        return Err("--open and --user-supplied exclude each other");
    }
    if statement.source_license.is_some() && (!statement.open || statement.source.is_none()) {
        return Err("--source-license states the licence of the --source archive; add --open");
    }
    match (unlisted, role) {
        (Some(id), Some(role)) => {
            if command.selected.is_some() || command.files.len() != 1 {
                return Err("--unlisted takes exactly one ROM file and no --for");
            }
            command.unlisted = Some((id, role));
        }
        (None, None) => (),
        _ => return Err("--unlisted ID and --role ROLE go together"),
    }
    if action == "import" && command.files.is_empty() {
        return Err("firmware import requires a folder or files");
    }
    if action == "edit" && command.tag.is_empty() {
        return Err("firmware show and edit require NAME:TAG");
    }
    let single = command.selected.is_some() || command.unlisted.is_some();
    if !command.extras.is_empty() && !single {
        return Err("--extra requires --for CATALOGUE-ID or --unlisted ID");
    }
    if action == "import" && *command.statement != Statement::default() && !single {
        return Err("metadata and licence options require --for CATALOGUE-ID or --unlisted ID");
    }
    if action == "edit" && *command.statement == Statement::default() {
        return Err("firmware edit needs at least one metadata or licence option");
    }
    if command
        .selected
        .as_ref()
        .is_some_and(|id| !catalogue().iter().any(|e| e.id == id))
    {
        return Err("unknown firmware catalogue id");
    }
    Ok(command)
}
/// Without `--store`, the store is the default the executable supplies; library
/// callers that supply none get an error instead of a host location.
pub fn execute(
    mut command: FirmwareCommand,
    output: &mut impl Write,
    sources: &HostSources,
) -> anyhow::Result<()> {
    if command.store.as_os_str().is_empty() {
        command.store = sources
            .store_root
            .clone()
            .ok_or_else(|| anyhow::anyhow!("default store unavailable; use --store DIR"))?;
    }
    if command.action == "import" {
        let control = machine_file::firmware_discovery::DiscoveryControl::default();
        let cache = if command.dry_run {
            None
        } else {
            sources.index.as_deref()
        };
        return machine_file::firmware_discovery::with_discovery_session(
            cache,
            command.rescan,
            &control,
            || import(command, output, catalogue()),
        );
    }
    let store = Store::open_existing(&command.store)?;
    edit(&store, command, output, catalogue())
}

fn edit(
    store: &Store,
    command: FirmwareCommand,
    output: &mut impl Write,
    catalogue: &[assembly::FirmwareEntry],
) -> anyhow::Result<()> {
    let target = match command.target {
        Some(target) => target,
        None => firmware_import::next_revision(&command.tag).ok_or_else(|| {
            anyhow::anyhow!(
                "{} is not a revision tag such as NAME:r1; name the new tag with --to",
                command.tag
            )
        })?,
    };
    let revision =
        firmware_import::edit(store, &command.tag, &target, &command.statement, catalogue)?;
    for note in &revision.notes {
        writeln!(output, "note\t{target}\t{note}")?;
    }
    writeln!(
        output,
        "edited\t{}\t{target}\t{}",
        command.tag, revision.descriptor.digest
    )?;
    Ok(())
}

fn import(
    command: FirmwareCommand,
    output: &mut impl Write,
    catalogue: &[assembly::FirmwareEntry],
) -> anyhow::Result<()> {
    let candidates = if let Some((id, role)) = &command.unlisted {
        vec![firmware_import::unlisted(
            &command.files[0],
            id,
            role,
            catalogue,
        )?]
    } else {
        let scan = firmware_import::scan(&command.files, catalogue)?;
        if scan.size_mismatches + scan.hash_mismatches > 0 {
            writeln!(
                output,
                "unmatched: {} files ({} size mismatches, {} SHA-256 mismatches)",
                scan.size_mismatches + scan.hash_mismatches,
                scan.size_mismatches,
                scan.hash_mismatches
            )?;
        }
        for issue in scan.issues {
            writeln!(output, "{:?}: {}", issue.source, issue.reason)?;
        }
        let candidates = scan
            .candidates
            .into_iter()
            .filter(|c| {
                command
                    .selected
                    .as_ref()
                    .is_none_or(|id| id == &c.catalogue_id)
            })
            .collect::<Vec<_>>();
        if command.selected.is_some() && candidates.is_empty() {
            anyhow::bail!("selected catalogue entry has no complete unambiguous package");
        }
        candidates
    };
    let mut eligible = Vec::new();
    for candidate in candidates {
        if command.selected.is_none()
            && command.unlisted.is_none()
            && candidate.is_ambiguous(catalogue)
        {
            writeln!(
                output,
                "{}: ambiguous catalogue identities; select --for CATALOGUE-ID",
                candidate.catalogue_id
            )?;
        } else {
            eligible.push(candidate);
        }
    }
    if eligible.is_empty() {
        if command.dry_run {
            writeln!(output, "dry run; no blobs or tag written")?;
        }
        return Ok(());
    }
    let store = if command.dry_run && !command.store.join("store.toml").exists() {
        None
    } else if command.create && !command.dry_run {
        Some(Store::open(&command.store)?)
    } else {
        Some(Store::open_existing(&command.store)?)
    };
    let extras = command
        .extras
        .into_iter()
        .map(|(path, dos_name)| Attachment { path, dos_name })
        .collect::<Vec<_>>();
    for extra in &extras {
        anyhow::ensure!(
            std::fs::metadata(&extra.path)?.is_file(),
            "extra must be a regular file"
        );
        if let Some(name) = &extra.dos_name {
            use image_store::media_set::MediaFormats;
            pc_image::media_set::PcMediaFormats.validate_name(name)?;
        }
    }
    let statement = &command.statement;
    let mut states = Vec::new();
    for candidate in &eligible {
        let preview =
            firmware_import::preview_manifest_with(candidate, &extras, statement, catalogue)?;
        let tag = match &store {
            Some(store) => firmware_import::import_tag(store, &candidate.catalogue_id)?,
            None => candidate.tag(),
        };
        for note in &preview.notes {
            writeln!(output, "note\t{tag}\t{note}")?;
        }
        let manifest = preview.manifest;
        let digest = Digest::of(&manifest.encode()?);
        states.push((
            match &store {
                Some(store) => firmware_import::existing(store, &tag, &manifest, catalogue)?
                    .map(|existing| (existing.digest != digest).then_some(existing.digest)),
                None => None,
            },
            tag,
        ));
    }
    for (c, (existing, tag)) in eligible.into_iter().zip(states) {
        match existing {
            Some(None) => {
                writeln!(output, "already imported\t{tag}")?;
                continue;
            }
            Some(Some(older)) => {
                writeln!(
                    output,
                    "already imported\t{tag}\t{older}\tthe same ROMs with other metadata or licences; image firmware edit {tag} changes them",
                )?;
                continue;
            }
            None => (),
        }
        writeln!(output, "complete\t{tag}\t{}", c.config.roms[0].sha256)?;
        if !command.dry_run {
            let d = firmware_import::publish_with(
                store
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("store unavailable"))?,
                &c,
                &extras,
                statement,
                catalogue,
            )?;
            anyhow::ensure!(
                d.media_type == media_type::MANIFEST,
                "invalid package descriptor"
            );
            writeln!(output, "imported\t{tag}\t{}", d.digest)?;
        }
    }
    if command.dry_run {
        writeln!(output, "dry run; no blobs or tag written")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_store::firmware::Validator;
    use pc_image::firmware::PcFirmwareRules;
    use std::path::Path;
    fn entry(id: &'static str, byte: u8) -> assembly::FirmwareEntry {
        let hash = Box::leak(
            image_store::Digest::of(&vec![byte; 32768])
                .hex()
                .to_owned()
                .into_boxed_str(),
        );
        assembly::FirmwareEntry {
            id,
            sha256: hash,
            size: 32768,
            distribution: assembly::FirmwareDistribution::UserSupplied,
            ..assembly::KISELEV_XI8088
        }
    }
    fn command(root: &Path, files: &Path) -> FirmwareCommand {
        FirmwareCommand {
            action: "import".into(),
            store: root.into(),
            files: vec![files.into()],
            tag: String::new(),
            create: true,
            dry_run: false,
            rescan: false,
            selected: None,
            extras: Vec::new(),
            unlisted: None,
            target: None,
            statement: Box::default(),
        }
    }
    #[test]
    fn min_app_version_is_an_import_statement_of_one_package() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        let command = parse(&args(&[
            "import",
            "rom.bin",
            "--unlisted",
            "rom",
            "--role",
            "option-rom",
            "--min-app-version",
            "0.0.2",
        ]))
        .unwrap();
        assert_eq!(
            command.statement.min_app_version,
            Some(image_store::compat::AppVersion::new(0, 0, 2))
        );
        for bad in [
            &["import", "roms", "--min-app-version", "0.0.2"][..],
            &["import", "roms", "--for", "x", "--min-app-version", "2"],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
        // An edit takes it too: the way to raise an imported package's value.
        let edit = parse(&args(&["edit", "a:1", "--min-app-version", "0.0.2"])).unwrap();
        assert!(edit.statement.min_app_version.is_some());
    }

    #[test]
    fn folder_reimport_keeps_existing_packages_and_publishes_new_ones() {
        let temp = machine_file::test_support::TempDir::new("firmware-folder-reimport");
        let root = temp.path();
        let files = root.join("roms");
        std::fs::create_dir(&files).unwrap();
        let a = files.join("a");
        std::fs::write(&a, vec![0x11; 32768]).unwrap();
        std::fs::write(files.join("b"), vec![0x22; 32768]).unwrap();
        let cat = [
            entry("synthetic-first", 0x11),
            entry("synthetic-second", 0x22),
        ];
        let store = Store::open(root.join("store")).unwrap();
        let scan = firmware_import::scan(&[a], &cat).unwrap();
        firmware_import::publish(&store, &scan.candidates[0], &[], &cat).unwrap();
        let mut out = Vec::new();
        import(command(store.root(), &files), &mut out, &cat).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("already imported\tprivate/firmware/ibm-pc/synthetic-first:r1"));
        assert!(out.contains("imported\tprivate/firmware/ibm-pc/synthetic-second:r1"));
        assert_eq!(store.list_tags(None, 10).unwrap().tags.len(), 2);
    }
    #[test]
    fn a_folder_imported_with_a_statement_is_already_imported_when_scanned_again() {
        let temp = machine_file::test_support::TempDir::new("firmware-described-reimport");
        let root = temp.path();
        let files = root.join("roms");
        std::fs::create_dir(&files).unwrap();
        std::fs::write(files.join("a"), vec![0x11; 32768]).unwrap();
        let cat = [entry("synthetic-first", 0x11)];
        let store = Store::open(root.join("store")).unwrap();
        let scan = firmware_import::scan(std::slice::from_ref(&files), &cat).unwrap();
        // As the library window imports a set whose details the user edited.
        let described = Statement {
            title: Some("Described first BIOS".into()),
            ..Statement::default()
        };
        let first =
            firmware_import::publish_with(&store, &scan.candidates[0], &[], &described, &cat)
                .unwrap();
        let mut out = Vec::new();
        import(command(store.root(), &files), &mut out, &cat).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.contains(&format!(
                "already imported\tprivate/firmware/ibm-pc/synthetic-first:r1\t{}\t",
                first.digest
            )) && out.contains("image firmware edit private/firmware/ibm-pc/synthetic-first:r1"),
            "{out}"
        );
        assert_eq!(
            store
                .tag_descriptor("private/firmware/ibm-pc/synthetic-first:r1")
                .unwrap()
                .digest,
            first.digest
        );
    }
    #[test]
    fn edit_keeps_a_hand_written_readme_unless_regenerate_readme_is_given() {
        let temp = machine_file::test_support::TempDir::new("firmware-edit-readme");
        let root = temp.path();
        let rom = root.join("rom.bin");
        std::fs::write(&rom, [0x66; 8192]).unwrap();
        let store = Store::open(root.join("store")).unwrap();
        let candidate = firmware_import::unlisted(&rom, "readme-rom", "option-rom", &[]).unwrap();
        let written = Statement {
            readme: ReadmeChoice::Text("# Mine\n\nWritten by hand.\n".into()),
            ..Statement::default()
        };
        firmware_import::publish_with(&store, &candidate, &[], &written, &[]).unwrap();
        let readme_of = |tag: &str| {
            let image = store
                .resolve(tag, &Validator(&PcFirmwareRules(&[])))
                .unwrap();
            let layer = image
                .manifest
                .layers
                .iter()
                .find(|d| d.decoded_media_type() == media_type::MARKDOWN)
                .unwrap()
                .clone();
            let bytes =
                image_store::BlobSource::read_verified(&store, &layer, image_store::MAX_SMALL_BLOB)
                    .unwrap();
            String::from_utf8(bytes).unwrap()
        };
        let run = |args: &[&str]| {
            let args: Vec<OsString> = args.iter().map(OsString::from).collect();
            let mut out = Vec::new();
            edit(&store, parse(&args).unwrap(), &mut out, &[]).unwrap();
            String::from_utf8(out).unwrap()
        };
        let out = run(&[
            "edit",
            "private/firmware/ibm-pc/readme-rom:r1",
            "--notes",
            "A quirk.",
        ]);
        assert!(out.contains("edited\tprivate/firmware/ibm-pc/readme-rom:r1\tprivate/firmware/ibm-pc/readme-rom:r2\t"));
        assert_eq!(
            readme_of("private/firmware/ibm-pc/readme-rom:r2"),
            "# Mine\n\nWritten by hand.\n"
        );
        run(&[
            "edit",
            "private/firmware/ibm-pc/readme-rom:r2",
            "--regenerate-readme",
        ]);
        let generated = readme_of("private/firmware/ibm-pc/readme-rom:r3");
        assert!(
            generated.starts_with("# readme-rom\n") && generated.contains("A quirk."),
            "{generated}"
        );
        assert_eq!(
            parse(
                &["edit", "x:1", "--regenerate-readme", "--regenerate-readme"].map(OsString::from)
            )
            .unwrap_err(),
            "duplicate firmware option"
        );
        assert_eq!(
            parse(&["import", "rom.bin", "--regenerate-readme"].map(OsString::from)).unwrap_err(),
            "missing firmware option value"
        );
    }
    #[test]
    fn ambiguity_is_skipped_before_any_existing_tag_precheck() {
        let temp = machine_file::test_support::TempDir::new("firmware-ambiguity-precheck");
        let root = temp.path();
        let file = root.join("rom");
        std::fs::write(&file, vec![0x11; 32768]).unwrap();
        let cat = [
            entry("synthetic-first", 0x11),
            entry("synthetic-alias", 0x11),
        ];
        let store = Store::open(root.join("store")).unwrap();
        let scan = firmware_import::scan(std::slice::from_ref(&file), &cat[..1]).unwrap();
        firmware_import::publish(&store, &scan.candidates[0], &[], &cat).unwrap();
        let mut out = Vec::new();
        import(command(store.root(), &file), &mut out, &cat).unwrap();
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("ambiguous catalogue identities"));
        assert_eq!(store.list_tags(None, 10).unwrap().tags.len(), 1);
    }
    #[test]
    fn listing_prints_only_the_original_facet_fields() {
        let temp = machine_file::test_support::TempDir::new("firmware-ls-fields");
        let root = temp.path();
        let file = root.join("rom");
        std::fs::write(&file, vec![0x11; 32768]).unwrap();
        let cat = [entry("synthetic-first", 0x11)];
        let store = Store::open(root.join("store")).unwrap();
        let scan = firmware_import::scan(std::slice::from_ref(&file), &cat).unwrap();
        let package = firmware_import::publish(&store, &scan.candidates[0], &[], &cat).unwrap();
        // The neutral listing, through the PC platform's validators.
        let listing = cli_core::image::firmware::FirmwareCommand::List {
            store: Some(store.root().to_owned()),
        };
        let mut out = Vec::new();
        crate::with_platform(|pc| {
            cli_core::image::firmware::execute(listing, &mut out, &HostSources::default(), &[pc])
        })
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(
                "private/firmware/ibm-pc/synthetic-first:r1\t{}\t\"synthetic-first\"\tSome(FirmwareSummary {{ \
                 roles: [\"system-bios\"], boards: [\"ibm-5170\"], machine_classes: [\"at\"], \
                 catalogue_ids: [\"synthetic-first\"], hashes: [\"{}\"] }})\t\
                 min-app-version:0.0.3\n",
                package.digest, cat[0].sha256
            )
        );
    }
}
