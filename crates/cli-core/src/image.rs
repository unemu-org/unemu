// SPDX-License-Identifier: AGPL-3.0-or-later
//! The image commands that need no emulator; machine-images §§2–3, §5.1 and
//! §8: `image ls`, `sweep`, `compress`, `image workspace ls` and `discard`,
//! and through the submodules `image media ls` and `show`, `image firmware
//! ls` and `show`, `image export`, and the registry commands. The other `image` words are a
//! platform's; [`parse`] leaves them to the platforms. The helpers the
//! platforms' image commands share with these (the `--min-app-version`
//! value, the listing columns of a minimum release, the refusal of a
//! machine's workspace) live here too.

pub mod export;
pub mod firmware;
pub mod media;
pub mod registry;

use super::{CliPlatform, HostSources, Io};
use image_registry::guard;
use image_store::{Manifest, Store};
use std::{ffi::OsString, io::Write, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum ImageCommand {
    Compress {
        store: PathBuf,
        source: String,
        tag: String,
    },
    Workspaces {
        store: PathBuf,
    },
    Discard {
        store: PathBuf,
        workspace: String,
        yes: bool,
    },
    Media(media::MediaCommand),
    Export(export::ExportCommand),
    Firmware(firmware::FirmwareCommand),
    Registry(registry::RegistryCommand),
    List {
        store: PathBuf,
    },
    Sweep {
        store: PathBuf,
    },
}

/// Parse the words after `image`: `Ok(None)` when they are a platform's.
pub fn parse(args: &[OsString]) -> Result<Option<ImageCommand>, &'static str> {
    if args
        .first()
        .is_some_and(|s| matches!(s.to_str(), Some("run" | "workspace" | "publish")))
    {
        let platform = matches!(args[0].to_str(), Some("run" | "publish"))
            || (args[0] == "workspace" && args.get(1).is_some_and(|a| a == "abandon-publication"));
        if platform {
            return Ok(None);
        }
        let words = parse_workspace(args)?;
        return Ok(match (words.action, words.sub) {
            ("workspace", Some("ls")) => Some(ImageCommand::Workspaces { store: words.store }),
            ("workspace", Some("discard")) => Some(ImageCommand::Discard {
                store: words.store,
                workspace: words.workspace.ok_or("workspace required")?,
                yes: words.yes,
            }),
            _ => return Err("invalid image command"),
        });
    }
    if args.first().is_some_and(|a| a == "firmware") {
        return firmware::parse(&args[1..]).map(|c| c.map(ImageCommand::Firmware));
    }
    if args.first().is_some_and(|a| a == "media") {
        return media::parse(&args[1..]).map(|c| c.map(ImageCommand::Media));
    }
    if args.first().is_some_and(|a| a == "export") {
        return export::parse(&args[1..]).map(|c| Some(ImageCommand::Export(c)));
    }
    if args
        .first()
        .is_some_and(|a| a == "machine" || a == "template")
    {
        return Ok(None);
    }
    if let Some(action @ ("push" | "pull" | "remote" | "login" | "registry")) =
        args.first().and_then(|a| a.to_str())
    {
        return registry::parse(action, &args[1..]).map(|c| Some(ImageCommand::Registry(c)));
    }
    let action = args
        .first()
        .and_then(|a| a.to_str())
        .ok_or("expected image import, compress, ls or sweep")?;
    let (source, start) = match action {
        "import" => return Ok(None),
        "compress" => (
            Some(
                args.get(1)
                    .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
                    .ok_or("image import requires a machine file")?,
            ),
            2,
        ),
        "ls" | "sweep" => (None, 1),
        _ => return Err("expected image import, compress, ls or sweep"),
    };
    let (mut store, mut tag) = (None, None);
    let mut args = args[start..].iter();
    while let Some(option) = args.next() {
        if option == "--create" {
            return Err("--create requires image import and is accepted once");
        }
        let value = args
            .next()
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing image option value")?;
        match option.to_str() {
            Some("--store") if store.is_none() => store = Some(PathBuf::from(value)),
            Some("--tag") if action == "compress" && tag.is_none() => {
                tag = Some(value.to_str().ok_or("invalid image tag")?.to_owned())
            }
            _ => return Err("unknown or duplicate image option"),
        }
    }
    let store = store.ok_or("image commands require --store DIR")?;
    Ok(Some(match source {
        Some(source) => ImageCommand::Compress {
            store,
            source: source.to_string_lossy().into_owned(),
            tag: tag.ok_or("image compress requires --tag NAME:TAG")?,
        },
        None if action == "sweep" => ImageCommand::Sweep { store },
        None => ImageCommand::List { store },
    }))
}

pub fn execute(
    command: ImageCommand,
    io: &mut Io<'_>,
    host: &HostSources,
    platforms: &[&dyn CliPlatform],
) -> anyhow::Result<u8> {
    let output = &mut io.output;
    match command {
        ImageCommand::Compress { store, source, tag } => {
            let store = Store::open_existing(store)?;
            let result = super::platform_of(platforms)?.compress(&store, &source, &tag)?;
            writeln!(output, "{tag}\t{}", result.digest)?;
        }
        ImageCommand::Workspaces { store } => {
            let store = Store::open_existing(store)?;
            for name in store.list_workspaces()? {
                writeln!(output, "{name}")?;
            }
        }
        ImageCommand::Discard {
            store,
            workspace,
            yes,
        } => {
            let store = Store::open_existing(store)?;
            // A machine's changes include its settings and pending inserts,
            // which only the machine command drops with its journals.
            refuse_machine(&store, &workspace, "image machine reset ID --yes")?;
            let owner = store.open_workspace(&workspace)?;
            for path in owner.contents()? {
                writeln!(output, "affected:{}", path.display())?;
            }
            if !yes {
                anyhow::bail!("discard requires --yes after reviewing affected media");
            }
            owner.discard()?;
        }
        ImageCommand::Media(command) => media::execute(command, output, platforms)?,
        ImageCommand::Export(command) => export::execute(command, output, host, platforms)?,
        ImageCommand::Firmware(command) => firmware::execute(command, output, host, platforms)?,
        ImageCommand::Registry(command) => registry::execute(command, output, platforms)?,
        ImageCommand::List { store } => {
            let store = image_store::Store::open_existing(store)?;
            let mut after = None;
            loop {
                let page = store.list_tags(after.as_deref(), 1000)?;
                for tag in page.tags {
                    use image_store::BlobSource;
                    let manifest = store
                        .read_verified(&tag.manifest, image_store::MAX_SMALL_BLOB)
                        .and_then(|bytes| image_store::Manifest::decode_for_summary(&bytes));
                    let requirement = requirement_columns(&store, &tag.manifest);
                    let (kind, stored, decoded) = match manifest {
                        Ok(m) => {
                            let stored = m
                                .layers
                                .iter()
                                .chain(std::iter::once(&m.config))
                                .try_fold(tag.manifest.size, |n, d| {
                                    n.checked_add(d.size)
                                        .ok_or_else(|| anyhow::anyhow!("stored size overflow"))
                                })?;
                            let decoded = m
                                .layers
                                .iter()
                                .chain(std::iter::once(&m.config))
                                .try_fold(tag.manifest.size, |n, d| -> anyhow::Result<u64> {
                                    n.checked_add(d.decoded_size()?)
                                        .ok_or_else(|| anyhow::anyhow!("decoded size overflow"))
                                })?;
                            (
                                if m.artifact_type() == image_store::media_type::MEDIA_SET {
                                    "media-set"
                                } else if m.artifact_type() == image_store::media_type::FIRMWARE {
                                    "firmware"
                                } else {
                                    "machine"
                                },
                                stored,
                                decoded,
                            )
                        }
                        Err(_) => ("unknown", 0, 0),
                    };
                    writeln!(
                        output,
                        "{}\t{}\t{}\tstored:{}\tdecoded:{}{requirement}",
                        tag.name, tag.manifest.digest, kind, stored, decoded
                    )?;
                }
                after = page.next_after;
                if after.is_none() {
                    break;
                }
            }
            writeln!(output, "materialized:{}", store.materialized_bytes()?)?;
        }
        ImageCommand::Sweep { store } => {
            let store = Store::open_existing(store)?;
            let mut pinned = Vec::new();
            for platform in platforms {
                pinned.extend(platform.pinned(&store)?);
            }
            let sweep = store.sweep_materialized(&pinned)?;
            writeln!(
                output,
                "removed:{}\tbytes:{}\tkept:{}",
                sweep.removed_files, sweep.removed_bytes, sweep.kept_bytes
            )?;
        }
    }
    Ok(0)
}

/// The value of `--min-app-version`: a release `MAJOR.MINOR.PATCH`, from
/// the first release up to this one, refused before anything is read or
/// copied.
pub fn min_app_version(value: &OsString) -> Result<image_store::compat::AppVersion, &'static str> {
    use image_store::compat::{level, AppVersion, BASELINE};
    let version: AppVersion = value
        .to_str()
        .and_then(|s| s.parse().ok())
        .ok_or("--min-app-version expects a release MAJOR.MINOR.PATCH")?;
    if version < BASELINE {
        return Err("--min-app-version is lower than 0.0.1, the first release");
    }
    if version > level() {
        // This build's level can be above its package version, so the message
        // names it.
        static NEWER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        return Err(NEWER.get_or_init(|| {
            format!(
                "--min-app-version is newer than {}, the newest release this unemupc reads",
                level()
            )
        }));
    }
    Ok(version)
}

/// The listing columns of an image's minimum release (machine-images §5.6),
/// read from its manifest alone: `min-app-version:X.Y.Z` when it declares
/// one, then `Needs unemupc X.Y.Z or newer` when this build does not read it.
/// Empty for an image without the annotation.
pub fn requirement_columns(store: &image_store::Store, image: &image_store::Descriptor) -> String {
    use image_store::{compat::Requirement, BlobSource};
    let Ok(bytes) = store.read_verified(image, image_store::MAX_SMALL_BLOB) else {
        return String::new();
    };
    let requirement = Requirement::read(&bytes);
    let declared = match &requirement {
        Requirement::None => return String::new(),
        Requirement::Release(release) => release.to_string(),
        Requirement::Invalid(text) => format!("{text:?}"),
    };
    let mut columns = format!("\tmin-app-version:{declared}");
    if !requirement.met_by(image_store::compat::level()) {
        let required = match requirement {
            Requirement::Release(release) => Some(release),
            _ => None,
        };
        columns.push('\t');
        columns.push_str(&image_store::compat::needs(required));
    }
    columns
}

/// The `min-app-version` line of a `show` command, when the image declares one.
pub fn write_min_app_version(
    output: &mut impl Write,
    manifest: &image_store::Manifest,
) -> std::io::Result<()> {
    match image_store::compat::Requirement::of(manifest) {
        image_store::compat::Requirement::Release(release) => {
            writeln!(output, "min-app-version\t{release}")
        }
        _ => Ok(()),
    }
}

/// Refuse a workspace command on a machine's workspace, naming the machine
/// command that does it with the machine's own rules.
pub fn refuse_machine(
    store: &image_store::Store,
    workspace: &str,
    instead: &str,
) -> anyhow::Result<()> {
    if store.read_machines()?.get(workspace).is_some() {
        anyhow::bail!(
            "{workspace} is a machine; use {}",
            instead.replace("ID", workspace)
        );
    }
    Ok(())
}

/// Every local mark of `store`, plus a restriction for each blob of
/// `manifest` a platform knows must not be redistributed: the push guard's
/// own classification, which `show` reports beside a set or package.
pub fn guard_marks(
    store: &Store,
    manifest: &Manifest,
    platforms: &[&dyn CliPlatform],
) -> image_registry::Result<guard::LocalMarks> {
    let mut marks = guard::LocalMarks::scan(store)?;
    for digest in restricted(store, manifest, platforms)? {
        marks.restrict_catalogue(digest);
    }
    Ok(marks)
}

/// The blobs of `manifest` that some platform knows must not be
/// redistributed ([`CliPlatform::restricted`]), in platform order: what
/// `show` and `push` add to the store's own marks.
pub fn restricted(
    store: &Store,
    manifest: &Manifest,
    platforms: &[&dyn CliPlatform],
) -> image_store::Result<Vec<image_store::Digest>> {
    let mut digests = Vec::new();
    for platform in platforms {
        digests.extend(platform.restricted(store, manifest)?);
    }
    Ok(digests)
}

/// The words of `image run`, `image workspace` and `image publish`, which
/// share their options: the neutral workspace commands and a platform's run
/// and publication commands read them the same way.
#[derive(Debug)]
pub struct WorkspaceWords<'a> {
    pub action: &'a str,
    pub sub: Option<&'a str>,
    pub tag: Option<String>,
    pub workspace: Option<String>,
    pub store: PathBuf,
    pub max_steps: Option<u64>,
    pub fresh: bool,
    pub flatten: bool,
    pub yes: bool,
}

/// Read the words of a workspace command; `args` starts with `run`,
/// `workspace` or `publish`.
pub fn parse_workspace(args: &[OsString]) -> Result<WorkspaceWords<'_>, &'static str> {
    let action = args[0].to_str().ok_or("invalid image action")?;
    let mut index = 1;
    let mut tag = None;
    let mut workspace = None;
    let mut sub = None;
    if action == "run" {
        tag = Some(
            args.get(index)
                .and_then(|s| s.to_str())
                .filter(|s| !s.starts_with("--"))
                .ok_or("image run requires NAME:TAG")?
                .to_owned(),
        );
        index += 1;
    } else if action == "workspace" {
        sub = Some(
            args.get(index)
                .and_then(|s| s.to_str())
                .ok_or("expected workspace ls, discard or abandon-publication")?,
        );
        index += 1;
        match sub {
            Some("ls") => (),
            Some("discard" | "abandon-publication") => {
                workspace = Some(
                    args.get(index)
                        .and_then(|s| s.to_str())
                        .ok_or("workspace discard requires NAME")?
                        .to_owned(),
                );
                index += 1;
            }
            _ => return Err("expected workspace ls, discard or abandon-publication"),
        }
    }
    let mut store = None;
    let mut max_steps = None;
    let (mut fresh, mut flatten, mut yes) = (false, false, false);
    while index < args.len() {
        let option = args[index].to_str().ok_or("invalid image option")?;
        index += 1;
        match option {
            "--new" if action == "run" && !fresh => {
                fresh = true;
                continue;
            }
            "--flatten" if action == "publish" && !flatten => {
                flatten = true;
                continue;
            }
            "--yes" if sub == Some("discard") && !yes => {
                yes = true;
                continue;
            }
            _ => (),
        }
        let value = args
            .get(index)
            .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
            .ok_or("missing image option value")?;
        index += 1;
        match option {
            "--store" if store.is_none() => store = Some(PathBuf::from(value)),
            "--workspace" if action == "run" && workspace.is_none() => {
                workspace = Some(value.to_str().ok_or("invalid workspace")?.into())
            }
            "--from-workspace" if action == "publish" && workspace.is_none() => {
                workspace = Some(value.to_str().ok_or("invalid workspace")?.into())
            }
            "--tag" if action == "publish" && tag.is_none() => {
                tag = Some(value.to_str().ok_or("invalid tag")?.into())
            }
            "--max-steps" if action == "run" && max_steps.is_none() => {
                max_steps = Some(
                    value
                        .to_str()
                        .and_then(|s| s.parse().ok())
                        .filter(|n| *n > 0)
                        .ok_or("invalid step limit")?,
                )
            }
            _ => return Err("unknown or duplicate image option"),
        }
    }
    Ok(WorkspaceWords {
        action,
        sub,
        tag,
        workspace,
        store: store.ok_or("image commands require --store DIR")?,
        max_steps,
        fresh,
        flatten,
        yes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Toy;
    use image_store::{license, media_type, Descriptor, Digest};

    /// The push guard of `show` and `push` adds what each platform knows
    /// must not be redistributed to the store's own marks.
    #[test]
    fn the_guard_marks_add_each_platforms_restrictions() {
        let dir = std::env::temp_dir().join(format!("cli-core-guard-marks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir).unwrap();
        let rom = Digest::of(b"rom");
        // A ROM the image itself marks open under MIT.
        let mut layer = Descriptor::new(media_type::FIRMWARE, rom.clone(), 3);
        layer.annotations.extend([
            (license::REDISTRIBUTION.into(), license::OPEN.into()),
            (license::LICENSE.into(), "MIT".into()),
        ]);
        let manifest = Manifest::new(
            "toy",
            Descriptor::new(media_type::CONFIG, Digest::of(b"config"), 6),
            vec![
                layer,
                Descriptor::new(media_type::METADATA, Digest::of(b"{}"), 2),
            ],
        )
        .unwrap();
        let source = |platforms: &[&dyn CliPlatform]| {
            let marks = guard_marks(&store, &manifest, platforms).unwrap();
            let entry = guard::closure(&manifest, &marks)
                .into_iter()
                .find(|entry| entry.descriptor.digest == rom)
                .unwrap();
            (entry.effective, entry.source)
        };
        let restricting = Toy {
            restricted: vec![rom.clone()],
        };
        let open = (guard::Redistribution::Open, guard::MarkSource::ThisImage);
        assert_eq!(
            source(&[&restricting]),
            (
                guard::Redistribution::UserSupplied,
                guard::MarkSource::Catalogue
            )
        );
        assert_eq!(
            source(&[&Toy::default(), &restricting]),
            (
                guard::Redistribution::UserSupplied,
                guard::MarkSource::Catalogue
            )
        );
        assert_eq!(source(&[&Toy::default()]), open);
        assert_eq!(source(&[]), open);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
