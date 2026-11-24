// SPDX-License-Identifier: AGPL-3.0-or-later
//! The platform-neutral command line (`backlog/docs/design/core-contract.md`
//! §7.1 and §7.2): the top-level parser, the usage text, the exit-code
//! policy, stored-image runs through the core contract, and the provider seam through
//! which each platform compiled into the program adds its own commands and
//! answers what the neutral ones ask of it.
//!
//! The neutral commands are `image run`, `image machine run`, `image ls`, `sweep`, `compress`, `image
//! workspace ls` and `discard`, `image media ls` and `show`, `image firmware
//! ls` and `show`, `image export`, the registry commands (`image push`, `pull`, `remote ls`,
//! `login`, `registry set`), `recipe check`, `show`, `plan` and `build`, the
//! library commands (`library ls`, `add`, `remove`, `scan`). The journal
//! commands `media generations`, `verify`, `rollback` and `commit` belong to
//! the platform. What the neutral commands
//! need of a platform (the validators of its artifact types, the blobs it
//! knows must not be redistributed, how it compresses an image, what its
//! sessions pin, its recipe provider, the names of its media formats) comes
//! through [`CliPlatform`]. The crate is in the neutral layer and depends on
//! no emulator, PC or application crate, which `scripts/crate-boundaries.sh`
//! checks on its manifest (core-contract.md §7.3).
//!
//! [`run`] parses the neutral words first, in the order the command line
//! always had, and offers the rest to each platform; with one platform every
//! command keeps its exact words, output and exit code.

pub mod image;
pub mod library;
pub mod recipe;

use image_store::{ConfigValidator, Descriptor, Digest, Manifest, Store};
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

/// Host locations the executable supplies; library callers and tests leave
/// them unset, so a command never discovers a host path on its own.
#[derive(Clone, Debug, Default)]
pub struct HostSources {
    /// Application and core release levels, supplied by command composition.
    pub compatibility: image_store::compat::Compatibility,
    /// Default store for `run` and `image firmware` when no `--store` is given.
    pub store_root: Option<PathBuf>,
    /// Persistent ROM hash index reused by `image firmware import`.
    pub index: Option<PathBuf>,
}

/// The two output streams of one command: results on `output`, warnings,
/// stop reports and errors on `errors`.
pub struct Io<'a> {
    pub output: &'a mut dyn Write,
    pub errors: &'a mut dyn Write,
}

/// Where a platform's usage lines go in the usage text. The neutral lines
/// and the platform lines interleave, by command family, as the text always
/// had them; each slot is named after the neutral line it follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageSlot {
    /// The platform's offline journal commands, after media creation.
    Journal,
    /// The first lines, before `--version | --about`.
    First,
    /// After `--version | --about`.
    AfterAbout,
    /// After `image sweep`.
    AfterImageSweep,
    /// After `image workspace discard`.
    AfterWorkspaceDiscard,
    /// Before the neutral machine-run line.
    BeforeMachineRun,
    /// After the neutral machine-run line.
    AfterMachineRun,
    /// After `image registry set`.
    AfterRegistry,
    /// After `recipe show`.
    AfterRecipe,
    /// The closing lines.
    Last,
}

/// One platform's part of the command line (core-contract.md §§7.2, 14.2.7).
pub trait CliPlatform {
    /// The platform id, in its current spelling.
    fn platform(&self) -> &str;
    /// Validators for every artifact type of the platform, for transfer and
    /// listing.
    fn validators(&self) -> Vec<&dyn ConfigValidator>;
    /// Digests of a manifest's blobs the platform knows must not be
    /// redistributed, for the push guard.
    fn restricted(&self, store: &Store, manifest: &Manifest) -> image_store::Result<Vec<Digest>>;
    /// Publish `target` as a compressed copy of `source`, with the local
    /// records the platform keeps beside an image.
    fn compress(&self, store: &Store, source: &str, target: &str) -> anyhow::Result<Descriptor>;
    /// The images this platform's workspaces and machines pin, which a
    /// sweep of the materialized cache keeps.
    fn pinned(&self, store: &Store) -> anyhow::Result<Vec<Digest>>;
    /// The platform's recipe provider, for `recipe check` and `show`.
    fn recipes(&self) -> Option<&dyn ::recipe::provider::Provider>;
    /// The name of a medium's format, from its table in a media set's
    /// config, as `image media show` prints it.
    fn medium_format(&self, format: &toml::Table) -> anyhow::Result<&'static str>;
    /// Select this platform's pinned image and retain its workspace ownership.
    /// `None` leaves the request to another platform. Session formats and
    /// machine migration stay with the platform that owns them.
    fn own_run<'a>(
        &self,
        store: &'a Store,
        request: &image::RunRequest<'_>,
        errors: &mut dyn Write,
    ) -> anyhow::Result<Option<image::OwnedRun<'a>>> {
        let _ = errors;
        let image = match request.tag {
            Some(tag) => store.tag_descriptor(tag)?,
            None => return Ok(None),
        };
        if !image_store::platform::same(&image::run_platform(store, &image)?, self.platform()) {
            return Ok(None);
        }
        Ok(Some(image::OwnedRun {
            owner: store.workspace(request.workspace)?,
            image,
            machine: false,
        }))
    }
    /// Prepare a stored run through the selected core. A platform without
    /// stored-image support keeps the default refusal.
    fn prepare_run<'a>(
        &self,
        owner: &'a image::OwnedRun<'_>,
        request: &image::RunRequest<'_>,
        core: &'static dyn unemu_core_api::Core,
    ) -> anyhow::Result<image::StoredRun<'a>> {
        let _ = (owner, request, core);
        anyhow::bail!(image::NO_STORED_RUN)
    }
    /// Retain the store's stopped-machine snapshot for the neutral exporter.
    fn export_snapshot<'a>(
        &self,
        store: &'a Store,
        id: &str,
    ) -> anyhow::Result<Option<Box<dyn image::machine_export::Snapshot + 'a>>> {
        let _ = (store, id);
        Ok(None)
    }
    /// The usage lines of the platform's own commands that go in `slot`,
    /// each ending in a line break; empty when none do.
    fn usage(&self, slot: UsageSlot) -> &str;
    /// Parse the platform's own command, or `None` when the words are not
    /// this platform's.
    fn parse(&self, args: &[OsString]) -> Option<Result<Box<dyn PlatformCommand>, &'static str>>;
}

/// A parsed platform command. `Debug` lets a test pin how a command line
/// parses.
pub trait PlatformCommand: std::fmt::Debug {
    /// Run the command. An error is printed on the error stream with its
    /// causes (`{:#}`) and the program exits 1.
    fn execute(self: Box<Self>, io: &mut Io<'_>, host: &HostSources) -> anyhow::Result<u8>;
}

/// A parsed command line.
#[derive(Debug)]
pub enum Command {
    /// No arguments: the usage text on standard output.
    Usage,
    Recipe(recipe::RecipeCommand),
    Library(library::LibraryCommand),
    Image(image::ImageCommand),
    /// A command a platform took.
    Platform(Box<dyn PlatformCommand>),
}

/// The refusal of words that only a platform takes when none compiled in
/// takes them.
const NO_PLATFORM: &str = "no platform compiled into this program takes this command";

/// Parse a command line: the neutral words first, in the command line's own
/// order, then each platform's.
pub fn parse(args: &[OsString], platforms: &[&dyn CliPlatform]) -> Result<Command, &'static str> {
    if args.first().is_some_and(|arg| arg == "recipe") {
        return Ok(Command::Recipe(recipe::parse(&args[1..])));
    }
    if args.is_empty() {
        return Ok(Command::Usage);
    }
    if args[0] == "library" {
        return library::parse(&args[1..]).map(Command::Library);
    }
    if args[0] == "image" {
        return match image::parse(&args[1..])? {
            Some(command) => Ok(Command::Image(command)),
            None => offer(args, platforms).unwrap_or(Err(NO_PLATFORM)),
        };
    }
    // Platform options may appear anywhere, so each platform sees all words.
    if let Some(parsed) = offer(args, platforms) {
        return parsed;
    }
    Err(if args[0] == "media" {
        NO_PLATFORM
    } else {
        "unknown or extra arguments"
    })
}

fn offer(
    args: &[OsString],
    platforms: &[&dyn CliPlatform],
) -> Option<Result<Command, &'static str>> {
    platforms
        .iter()
        .find_map(|platform| platform.parse(args))
        .map(|parsed| parsed.map(Command::Platform))
}

/// The usage text: the neutral lines with each platform's lines in their
/// slots. The first command line starts with `usage: `.
pub fn usage(platforms: &[&dyn CliPlatform]) -> String {
    let mut text = String::new();
    let slot = |text: &mut String, slot| {
        for platform in platforms {
            text.push_str(platform.usage(slot));
        }
    };
    slot(&mut text, UsageSlot::First);
    text.push_str("       unemu --version | --about (licence, source and third-party notices)\n");
    slot(&mut text, UsageSlot::AfterAbout);
    text.push_str(concat!(
        "       unemu image compress NAME:TAG --store DIR --tag NAME:TAG\n",
        "       unemu image ls --store DIR\n",
        "       unemu image sweep --store DIR\n",
    ));
    text.push_str(
        "       unemu image run NAME:TAG --store DIR --workspace NAME [--new] [--max-steps N]\n",
    );
    slot(&mut text, UsageSlot::AfterImageSweep);
    text.push_str(concat!(
        "       unemu image workspace ls --store DIR\n",
        "       unemu image workspace discard NAME --store DIR --yes\n",
    ));
    slot(&mut text, UsageSlot::AfterWorkspaceDiscard);
    slot(&mut text, UsageSlot::BeforeMachineRun);
    text.push_str("       unemu image machine run ID --store DIR [--max-steps N]\n");
    slot(&mut text, UsageSlot::AfterMachineRun);
    text.push_str(concat!(
        "       unemu image firmware ls [--store DIR]\n",
        "       unemu image firmware show NAME:TAG [--store DIR]\n",
        "       unemu image media ls --store DIR\n",
        "       unemu image media show NAME:TAG --store DIR\n",
        "       unemu image machine export ID [--store DIR]\n",
        "       unemu image machine export ID --format FMT [--slot NAME] --to DIR [--store DIR]\n",
        "       unemu image export NAME:TAG --to DIR [--files] [--store DIR] (every layer as a plain file under its file name, with config.toml, manifest.json, EXPORT-VERSION and SHA256SUMS; --files also the files of the set's FAT12 disks; exit 1 refused, nothing overwritten)\n",
        "       unemu image push NAME:TAG [REFERENCE] --store DIR [--registry HOST] [--move] [--dry-run] [--config FILE]\n",
        "       unemu image pull REFERENCE --store DIR [--tag NAME:TAG] [--create] [--registry HOST] [--config FILE]\n",
        "       unemu image remote ls REPOSITORY|REGISTRY [--registry HOST] [--config FILE]\n",
        "       unemu image login HOST --user NAME [--config FILE]\n",
        "       unemu image registry set HOST [--private | --namespace NAME[:private|:public]...] [--config FILE]\n",
    ));
    slot(&mut text, UsageSlot::AfterRegistry);
    text.push_str(concat!(
        "       unemu recipe check FILE|FOLDER... [--deny-drift] (the static checks of recipes and work files; exit 0 clean, 1 with findings)\n",
        "       unemu recipe show FILE (a media recipe's title record merged with its work file; exit 1 with the check's findings)\n",
        "       unemu recipe plan FILE|REF [--json] [--library DIR]... [--store DIR] [--config FILE] (where each input is found, or why not; a REF is judged from its manifest and config; exit 0 all found, 1 missing)\n",
        "       unemu recipe build FILE [--fetch] [--library DIR]... [--store DIR] [--config FILE] [--keep-as TAG] [--deny-drift] (builds a recipe and tags it, a pinned one from its lock; --fetch downloads missing inputs; exit 0 tagged, 1 not built)\n",
        "       unemu recipe pin FILE [--fetch] [--library DIR]... [--store DIR] [--config FILE] (builds a draft, writes its lock beside it and its digest into it; exit 0 pinned, 1 not built)\n",
        "       unemu recipe conformance FILE|FOLDER... [--library DIR]... [--store DIR] [--config FILE] [--allow-drafts] (builds each pinned recipe into a scratch store and compares its digest; exit 0 all conform, 1 not)\n",
        "       unemu recipe scaffold FILE|FOLDER... (the [input] tables, member lists with the archive's times and file sets of files the author has; exit 1 for one a recipe cannot hold)\n",
        "       unemu recipe inputs FILE --format dat|csv|sha256sums (a recipe's inputs for a ROM manager or sha256sum -c; exit 1 with the check's findings)\n",
        "       unemu recipe push FILE REF [--dry-run] [--registry HOST] [--store DIR] [--config FILE] (publishes a pinned recipe with its work file, bundled inputs and lock under the recipe guard; exit 1 refused)\n",
        "       unemu recipe pull REF [--to DIR] [--registry HOST] [--config FILE] (writes a recipe and its parts into DIR, where it plans and builds; exit 1 refused)\n",
        "       unemu library ls [--store DIR] [--config FILE]\n",
        "       unemu library add|remove DIR [--config FILE]\n",
        "       unemu library scan [DIR...] [--store DIR] [--config FILE] (lists files, zip directories and catalogues into the index; hashes nothing)\n",
    ));
    slot(&mut text, UsageSlot::AfterRecipe);
    slot(&mut text, UsageSlot::Journal);
    slot(&mut text, UsageSlot::Last);
    // Every command line is indented as far as `usage: ` is long.
    match text.strip_prefix("       ") {
        Some(rest) => format!("usage: {rest}"),
        None => text,
    }
}

/// Run a command line and return the exit code: 0 success, 1 a failed
/// command, 2 a usage or configuration error, 3 a journal that needs
/// recovery (reported by the platform). A refused command line prints
/// the usage text and the reason on the error stream.
pub fn run(
    args: &[OsString],
    io: &mut Io<'_>,
    host: &HostSources,
    platforms: &[&dyn CliPlatform],
    cores: &core_launch::Cores,
) -> u8 {
    let command = match parse(args, platforms) {
        Ok(command) => command,
        Err(error) => {
            let _ = writeln!(io.errors, "{}{error}", usage(platforms));
            return 2;
        }
    };
    match command {
        Command::Recipe(command) => recipe::execute(command, io, host, platforms, cores),
        Command::Library(command) => {
            let result = library::execute(command, &mut *io.output, &mut *io.errors, host);
            finish(result, io)
        }
        Command::Usage => match io.output.write_all(usage(platforms).as_bytes()) {
            Ok(()) => 0,
            Err(error) => {
                let _ = writeln!(io.errors, "{error}");
                1
            }
        },
        Command::Image(command) => {
            let result = image::execute(command, io, host, platforms, cores);
            finish(result, io)
        }
        Command::Platform(command) => {
            let result = command.execute(io, host);
            finish(result, io)
        }
    }
}

/// A command's result as its exit code: an error is printed with its causes
/// and exits 1.
fn finish(result: anyhow::Result<u8>, io: &mut Io<'_>) -> u8 {
    match result {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(io.errors, "{error:#}");
            1
        }
    }
}

/// The platform that answers for a command naming one artifact. One platform
/// is compiled in today; how a command picks one of several is decided with
/// the second platform (core-contract.md §7.2).
fn platform_of<'p>(platforms: &[&'p dyn CliPlatform]) -> anyhow::Result<&'p dyn CliPlatform> {
    platforms
        .first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("{NO_PLATFORM}"))
}

/// Every validator of every platform, in platform order.
fn validators<'p>(platforms: &[&'p dyn CliPlatform]) -> Vec<&'p dyn ConfigValidator> {
    platforms
        .iter()
        .flat_map(|platform| platform.validators())
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A platform that takes `toy`, knows the blobs it lists must not be
    /// redistributed, and says nothing else.
    #[derive(Default)]
    pub(crate) struct Toy {
        pub(crate) restricted: Vec<Digest>,
    }
    #[derive(Debug)]
    struct ToyCommand;
    impl PlatformCommand for ToyCommand {
        fn execute(self: Box<Self>, io: &mut Io<'_>, _: &HostSources) -> anyhow::Result<u8> {
            writeln!(io.output, "toy ran")?;
            anyhow::bail!("toy failed")
        }
    }
    impl CliPlatform for Toy {
        fn platform(&self) -> &str {
            unemu_test_core::PLATFORM
        }
        fn validators(&self) -> Vec<&dyn ConfigValidator> {
            Vec::new()
        }
        fn restricted(&self, _: &Store, _: &Manifest) -> image_store::Result<Vec<Digest>> {
            Ok(self.restricted.clone())
        }
        fn compress(&self, _: &Store, _: &str, _: &str) -> anyhow::Result<Descriptor> {
            anyhow::bail!("toy images do not compress")
        }
        fn pinned(&self, _: &Store) -> anyhow::Result<Vec<Digest>> {
            Ok(Vec::new())
        }
        fn recipes(&self) -> Option<&dyn ::recipe::provider::Provider> {
            None
        }
        fn medium_format(&self, _: &toml::Table) -> anyhow::Result<&'static str> {
            anyhow::bail!("toy media have no formats")
        }
        fn prepare_run<'a>(
            &self,
            owner: &'a image::OwnedRun<'_>,
            _: &image::RunRequest<'_>,
            core: &'static dyn unemu_core_api::Core,
        ) -> anyhow::Result<image::StoredRun<'a>> {
            use image_store::BlobSource;
            let store = owner.owner.store();
            let manifest = Manifest::decode_for_summary(
                &store.read_verified(&owner.image, image_store::MAX_SMALL_BLOB)?,
            )?;
            let table: toml::Table = std::str::from_utf8(
                &store.read_verified(&manifest.config, image_store::MAX_SMALL_BLOB)?,
            )?
            .parse()?;
            let script = table["script"].as_str().unwrap().as_bytes().to_vec();
            let digest = Digest::of(&script);
            let prepared = core_launch::Prepared::from_parts(core_launch::Parts {
                core,
                platform: unemu_test_core::PLATFORM,
                title: "Toy".into(),
                notes: Vec::new(),
                config: unemu_core_api::ConfigDocumentBuf::new(
                    unemu_test_core::CONFIG_TYPE,
                    format!(
                        "unemu-test-core-config 1\nscript {} {}\n",
                        digest.hex(),
                        script.len()
                    )
                    .into_bytes(),
                ),
                content: Box::new(ToyContent(script)),
                storage: Box::new(ToyStorage),
                options: Vec::new(),
            });
            let mut stored = image::StoredRun::new(prepared, owner.image.digest.clone());
            stored.created = Box::new(move || {
                assert!(
                    store.workspace(owner.owner.name()).is_err(),
                    "ownership must span creation"
                );
                Ok(vec!["toy created".into()])
            });
            Ok(stored)
        }
        fn usage(&self, slot: UsageSlot) -> &str {
            match slot {
                UsageSlot::First => "       unemu toy\n",
                UsageSlot::Last => "toy footer\n",
                _ => "",
            }
        }
        fn parse(
            &self,
            args: &[OsString],
        ) -> Option<Result<Box<dyn PlatformCommand>, &'static str>> {
            match args {
                [word] if word == "toy" => Some(Ok(Box::new(ToyCommand))),
                [word, ..] if word == "toy" => Some(Err("toy takes no arguments")),
                _ => None,
            }
        }
    }

    struct ToyContent(Vec<u8>);
    impl core_launch::ContentPlan for ToyContent {
        fn open(&self) -> Result<Box<dyn unemu_core_api::ContentSource>, unemu_core_api::Error> {
            Ok(Box::new(Self(self.0.clone())))
        }
    }
    impl unemu_core_api::ContentSource for ToyContent {
        fn open(
            &self,
            sha256: &[u8; 32],
            size: u64,
        ) -> Result<unemu_core_api::Content, unemu_core_api::Error> {
            let hex: String = sha256.iter().map(|byte| format!("{byte:02x}")).collect();
            assert_eq!(hex, Digest::of(&self.0).hex());
            assert_eq!(size, self.0.len() as u64);
            Ok(unemu_core_api::Content::Bytes(self.0.clone()))
        }
    }
    struct ToyStorage;
    thread_local! {
        static TOY_RECORDS: std::cell::RefCell<unemu_core_api::test_support::MemoryStorage> =
            std::cell::RefCell::new(unemu_core_api::test_support::MemoryStorage::new());
    }
    impl core_launch::StoragePlan for ToyStorage {
        fn open(&self) -> Result<std::rc::Rc<dyn unemu_core_api::Storage>, unemu_core_api::Error> {
            Ok(TOY_RECORDS.with(|records| {
                std::rc::Rc::new(records.borrow().clone())
                    as std::rc::Rc<dyn unemu_core_api::Storage>
            }))
        }
    }

    #[test]
    fn the_toy_platform_runs_a_stored_image_through_the_test_core() {
        use image_store::{media_type, BlobSource};
        struct Validator;
        impl ConfigValidator for Validator {
            fn platform(&self) -> &str {
                unemu_test_core::PLATFORM
            }
            fn validate(
                &self,
                _: &toml::Table,
                _: &[Descriptor],
                _: &dyn BlobSource,
            ) -> image_store::Result<Vec<Digest>> {
                Ok(Vec::new())
            }
        }
        let root = std::env::temp_dir().join(format!("cli-toy-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::open(&root).unwrap();
        let config = format!("schema = 1\nplatform = {:?}\nscript = \"\"\"\nunemu-test-core 1\nbattery\nat 2 note hello\nat 3 warning careful\nat 5 power-off\n\"\"\"\n", unemu_test_core::PLATFORM);
        let blob = store.import(config.as_bytes(), None).unwrap();
        let metadata = image_store::metadata::Metadata::new(unemu_test_core::PLATFORM)
            .encode(&[])
            .unwrap();
        let metadata = store.import(metadata.as_slice(), None).unwrap();
        let manifest = Manifest::new(
            unemu_test_core::PLATFORM,
            Descriptor::new(media_type::CONFIG, blob.digest, blob.size),
            vec![Descriptor::new(
                media_type::METADATA,
                metadata.digest,
                metadata.size,
            )],
        )
        .unwrap();
        let image = store.import_manifest(&manifest).unwrap();
        store.update_tag("toy:1", &image, None, &Validator).unwrap();
        let args = |steps: &str| {
            vec![
                "image".into(),
                "run".into(),
                "toy:1".into(),
                "--store".into(),
                root.clone().into_os_string(),
                "--workspace".into(),
                "play".into(),
                "--max-steps".into(),
                steps.into(),
            ]
        };
        assert!(matches!(
            parse(&args("4"), &[]),
            Ok(Command::Image(image::ImageCommand::Run { .. }))
        ));
        let cores = core_launch::Cores::new(&[&unemu_test_core::TestCore]);
        TOY_RECORDS.with(|records| {
            *records.borrow_mut() = unemu_core_api::test_support::MemoryStorage::new()
        });
        for (steps, stop, starts) in [("4", "Steps", 1), ("10", "PoweredOff", 2)] {
            let (mut output, mut errors) = (Vec::new(), Vec::new());
            assert_eq!(
                run(
                    &args(steps),
                    &mut Io {
                        output: &mut output,
                        errors: &mut errors
                    },
                    &HostSources::default(),
                    &[&Toy::default()],
                    &cores
                ),
                0
            );
            assert_eq!(
                String::from_utf8(output).unwrap(),
                format!(
                    "pinned-image:{}\ntoy created\nworkspace:play\t{stop}\n",
                    image.digest
                )
            );
            assert_eq!(errors, b"hello\ncareful\n");
            TOY_RECORDS.with(|records| {
                assert!(String::from_utf8(
                    records
                        .borrow()
                        .durable(unemu_test_core::BATTERY_RECORD)
                        .unwrap()
                )
                .unwrap()
                .ends_with(&format!("starts {starts}\n")))
            });
            assert!(store.workspace("play").is_ok(), "finish releases ownership");
        }
        let (mut output, mut errors) = (Vec::new(), Vec::new());
        assert_eq!(
            run(
                &args("4"),
                &mut Io {
                    output: &mut output,
                    errors: &mut errors
                },
                &HostSources::default(),
                &[&Toy::default()],
                &core_launch::Cores::new(&[])
            ),
            1
        );
        assert!(output.is_empty());
        assert_eq!(
            String::from_utf8(errors).unwrap(),
            format!("{}\n", image::NO_STORED_RUN)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    fn words(text: &str) -> Vec<OsString> {
        text.split_whitespace().map(OsString::from).collect()
    }

    fn run_with(args: &str, platforms: &[&dyn CliPlatform]) -> (u8, String, String) {
        let (mut output, mut errors) = (Vec::new(), Vec::new());
        let code = run(
            &words(args),
            &mut Io {
                output: &mut output,
                errors: &mut errors,
            },
            &HostSources::default(),
            platforms,
            &core_launch::Cores::new(&[]),
        );
        (
            code,
            String::from_utf8(output).unwrap(),
            String::from_utf8(errors).unwrap(),
        )
    }

    #[test]
    fn a_platform_fills_its_usage_slots_and_its_words_reach_it() {
        let toy: &dyn CliPlatform = &Toy::default();
        let text = usage(&[toy]);
        assert!(
            text.starts_with("usage: unemu toy\n       unemu --version"),
            "{text}"
        );
        assert!(text.ends_with("toy footer\n"), "{text}");
        assert_eq!(
            run_with("toy", &[toy]),
            (1, "toy ran\n".into(), "toy failed\n".into())
        );
        let (code, output, errors) = run_with("toy extra", &[toy]);
        assert_eq!((code, output.as_str()), (2, ""));
        assert!(errors.starts_with(&text) && errors.ends_with("toy takes no arguments\n"));
    }

    #[test]
    fn without_a_platform_the_neutral_commands_still_parse_and_the_rest_is_refused() {
        let text = usage(&[]);
        assert!(text.starts_with("usage: unemu --version"), "{text}");
        assert!(!text.contains("--boot") && !text.contains("image import"));
        assert!(matches!(
            parse(&words("image ls --store s"), &[]),
            Ok(Command::Image(image::ImageCommand::List { .. }))
        ));
        assert_eq!(
            parse(&words("media verify --base b --journal j"), &[]).unwrap_err(),
            NO_PLATFORM
        );
        assert!(matches!(
            parse(&words("recipe check a.media.toml"), &[]),
            Ok(Command::Recipe(_))
        ));
        assert_eq!(
            parse(&words("image import m.toml --store s --tag a:b"), &[]).unwrap_err(),
            NO_PLATFORM
        );
        assert_eq!(
            parse(&words("--boot"), &[]).unwrap_err(),
            "unknown or extra arguments"
        );
    }
}
