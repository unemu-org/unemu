// SPDX-License-Identifier: AGPL-3.0-or-later
//! The platform-neutral command line (`backlog/docs/design/core-contract.md`
//! §7.1 and §7.2): the top-level parser, the usage text, the exit-code
//! policy, the commands that need no emulator, and the provider seam through
//! which each platform compiled into the program adds its own commands and
//! answers what the neutral ones ask of it.
//!
//! The neutral commands are `image ls`, `sweep`, `compress`, `image
//! workspace ls` and `discard`, `image media ls` and `show`, `image firmware
//! ls` and `show`, the registry commands (`image push`, `pull`, `remote ls`,
//! `login`, `registry set`), `recipe check`, `show` and `plan`, the
//! library commands (`library ls`, `add`, `remove`, `scan`), and the journal
//! commands `media generations`, `verify`, `rollback` and `commit`. What they
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
pub mod journal;
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
    /// The first lines, before `--version | --about`.
    First,
    /// After `--version | --about`.
    AfterAbout,
    /// After `image sweep`.
    AfterImageSweep,
    /// After `image workspace discard`.
    AfterWorkspaceDiscard,
    /// After `image registry set`.
    AfterRegistry,
    /// After `recipe show`.
    AfterRecipe,
    /// The closing lines.
    Last,
}

/// One platform's part of the command line.
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
    Journal(journal::JournalCommand),
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
    // A platform's own options may appear anywhere on its command lines, so
    // its words are looked at before the journal commands.
    if let Some(parsed) = offer(args, platforms) {
        return parsed;
    }
    journal::parse(args).map(Command::Journal)
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
    text.push_str("       upcli --version | --about (licence, source and third-party notices)\n");
    slot(&mut text, UsageSlot::AfterAbout);
    text.push_str(concat!(
        "       upcli image compress NAME:TAG --store DIR --tag NAME:TAG\n",
        "       upcli image ls --store DIR\n",
        "       upcli image sweep --store DIR\n",
    ));
    slot(&mut text, UsageSlot::AfterImageSweep);
    text.push_str(concat!(
        "       upcli image workspace ls --store DIR\n",
        "       upcli image workspace discard NAME --store DIR --yes\n",
    ));
    slot(&mut text, UsageSlot::AfterWorkspaceDiscard);
    text.push_str(concat!(
        "       upcli image firmware ls [--store DIR]\n",
        "       upcli image firmware show NAME:TAG [--store DIR]\n",
        "       upcli image media ls --store DIR\n",
        "       upcli image media show NAME:TAG --store DIR\n",
        "       upcli image push NAME:TAG [REFERENCE] --store DIR [--registry HOST] [--move] [--dry-run] [--config FILE]\n",
        "       upcli image pull REFERENCE --store DIR [--tag NAME:TAG] [--create] [--registry HOST] [--config FILE]\n",
        "       upcli image remote ls REPOSITORY|REGISTRY [--registry HOST] [--config FILE]\n",
        "       upcli image login HOST --user NAME [--config FILE]\n",
        "       upcli image registry set HOST [--private | --namespace NAME[:private|:public]...] [--config FILE]\n",
    ));
    slot(&mut text, UsageSlot::AfterRegistry);
    text.push_str(concat!(
        "       upcli recipe check FILE|FOLDER... (the static checks of recipes and work files; exit 0 clean, 1 with findings)\n",
        "       upcli recipe show FILE (a media recipe's title record merged with its work file; exit 1 with the check's findings)\n",
        "       upcli recipe plan FILE [--json] [--library DIR]... [--store DIR] [--config FILE] (where each input is found, or why not; exit 0 all found, 1 missing)\n",
        "       upcli library ls [--store DIR] [--config FILE]\n",
        "       upcli library add|remove DIR [--config FILE]\n",
        "       upcli library scan [DIR...] [--store DIR] [--config FILE] (lists files, zip directories and catalogues into the index; hashes nothing)\n",
    ));
    slot(&mut text, UsageSlot::AfterRecipe);
    text.push_str(concat!(
        "       upcli media generations --base IMG --journal J\n",
        "       upcli media verify --base IMG --journal J [--repair-tail --backup PATH]\n",
        "       upcli media rollback --base IMG --journal J --to N --backup PATH\n",
        "       upcli media commit (alias compact) --base IMG --journal J --to N --output OUT\n",
    ));
    slot(&mut text, UsageSlot::Last);
    // Every command line is indented as far as `usage: ` is long.
    match text.strip_prefix("       ") {
        Some(rest) => format!("usage: {rest}"),
        None => text,
    }
}

/// Run a command line and return the exit code: 0 success, 1 a failed
/// command, 2 a usage or configuration error, 3 a journal that needs
/// recovery ([`journal::report_exit_code`]). A refused command line prints
/// the usage text and the reason on the error stream.
pub fn run(
    args: &[OsString],
    io: &mut Io<'_>,
    host: &HostSources,
    platforms: &[&dyn CliPlatform],
) -> u8 {
    let command = match parse(args, platforms) {
        Ok(command) => command,
        Err(error) => {
            let _ = writeln!(io.errors, "{}{error}", usage(platforms));
            return 2;
        }
    };
    match command {
        Command::Recipe(command) => recipe::execute(command, io, host, platforms),
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
            let result = image::execute(command, io, host, platforms);
            finish(result, io)
        }
        Command::Journal(command) => journal::run(command, io),
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
            "toy"
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
        fn usage(&self, slot: UsageSlot) -> &str {
            match slot {
                UsageSlot::First => "       upcli toy\n",
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
            text.starts_with("usage: upcli toy\n       upcli --version"),
            "{text}"
        );
        assert!(text.ends_with("--output OUT\ntoy footer\n"), "{text}");
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
        assert!(text.starts_with("usage: upcli --version"), "{text}");
        assert!(!text.contains("--boot") && !text.contains("image import"));
        assert!(matches!(
            parse(&words("image ls --store s"), &[]),
            Ok(Command::Image(image::ImageCommand::List { .. }))
        ));
        assert!(matches!(
            parse(&words("media verify --base b --journal j"), &[]),
            Ok(Command::Journal(_))
        ));
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
