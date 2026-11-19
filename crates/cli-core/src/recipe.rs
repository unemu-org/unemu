// SPDX-License-Identifier: AGPL-3.0-or-later
//! `upcli recipe check`: the static checks of recipes, which read the
//! recipe, its work file and whether its lock exists, and no input
//! (`backlog/docs/design/recipes.md` §6.11 and §6.15). `upcli recipe show`:
//! a media recipe's description merged with its work file (recipes §6.16),
//! printed as `upcli image media show` prints a set's title metadata.
//! `upcli recipe plan`: where each input of a recipe is found, or why not,
//! which steps would run and what can be built (recipes §3.4, §3.5), from
//! the store, the recipe's folder and the libraries of the preferences and
//! of `--library`. `upcli recipe build`: the draft build of recipes §3.1,
//! from the same sources plus the download cache, with downloads only on
//! `--fetch`, the user's consent (recipes §6.13); the outputs go through the
//! platform's importers and the artifact is tagged last; a pinned recipe
//! publishes its lock's manifest, config and metadata unchanged after its
//! payloads matched them (recipes §3.8). `upcli recipe pin`: the author's
//! build, which writes the lock beside the recipe and its digest into it.
//! `upcli recipe conformance`: builds every pinned recipe of the files and
//! folders named into a scratch store and compares the digests (recipes
//! §9). The recipe providers are the platforms'
//! ([`super::CliPlatform::recipes`]).

use super::{library, CliPlatform, HostSources, Io};
use recipe::library::Library;
use recipe::merge::{Merged, Origin, Sourced};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage: upcli recipe check FILE|FOLDER...
       upcli recipe show FILE
       upcli recipe plan FILE [--json] [--library DIR]... [--store DIR] [--config FILE]
       upcli recipe build FILE [--fetch] [--library DIR]... [--store DIR] [--config FILE] [--keep-as TAG]
       upcli recipe pin FILE [--fetch] [--library DIR]... [--store DIR] [--config FILE]
       upcli recipe conformance FILE|FOLDER... [--library DIR]... [--store DIR] [--config FILE]
  check: checks recipes and work files; a folder is checked with every *.media.toml, *.firmware.toml, *.machine.toml and *.work.toml below it
  show: prints a media recipe's title record merged with its work file, with a comment on each value that came from the work file; for a pinned recipe the record of its lock, which a build writes
  plan: where each input is found (the store, the download cache, beside the recipe, the libraries of the preferences, then each --library) or why not, the steps that would run and the outputs
  build: builds a recipe and tags it <name>:<tag> in the store, or <name>:TAG with --keep-as; a pinned recipe publishes its lock's bytes once every output matched them, and with --keep-as a result that differs is kept under TAG; --fetch downloads the missing inputs from their URLs into the store's download cache, after printing what it downloads
  pin: builds a draft as build does, then writes its manifest, config and metadata to FILE.lock/sha256/ and its manifest digest into FILE
  conformance: builds each pinned recipe into a scratch store from the libraries only and compares the digest with its pin; drafts are listed, not built
  exit 0: no findings, for plan every input found, for build and pin the artifact tagged, for conformance every pinned recipe built to its digest; 1: findings, which show, plan, build and pin print instead, for plan, build and pin an input missing, for build and pin a failed download, read, step or import, or a lock mismatch, with nothing tagged, for conformance a recipe that differs or was not built, or no pinned recipe found; 2: usage or a file that cannot be read
";

/// A parsed `upcli recipe` command line.
#[derive(Debug, PartialEq, Eq)]
pub enum RecipeCommand {
    Check(Vec<PathBuf>),
    Show(PathBuf),
    Plan(PlanOptions),
    Build(BuildOptions),
    /// `recipe pin`: the build's words without `--keep-as`.
    Pin(BuildOptions),
    Conformance(ConformanceOptions),
    /// Words `recipe` does not take: its own usage text, exit 2.
    Usage,
}

/// The words of `upcli recipe plan`.
#[derive(Debug, PartialEq, Eq)]
pub struct PlanOptions {
    pub path: PathBuf,
    pub json: bool,
    /// Searched after the libraries of the preferences, in this order.
    pub libraries: Vec<PathBuf>,
    pub store: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

/// The words of `upcli recipe build`.
#[derive(Debug, PartialEq, Eq)]
pub struct BuildOptions {
    pub path: PathBuf,
    /// The user's consent to download missing inputs.
    pub fetch: bool,
    pub libraries: Vec<PathBuf>,
    pub store: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub keep_as: Option<String>,
}

/// The words of `upcli recipe conformance`.
#[derive(Debug, PartialEq, Eq)]
pub struct ConformanceOptions {
    pub paths: Vec<PathBuf>,
    pub libraries: Vec<PathBuf>,
    pub store: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

fn conformance_options(args: &[OsString]) -> Option<ConformanceOptions> {
    let mut options = ConformanceOptions {
        paths: Vec::new(),
        libraries: Vec::new(),
        store: None,
        config: None,
    };
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if word == "--library" {
            options.libraries.push(PathBuf::from(words.next()?));
        } else if word == "--store" || word == "--config" {
            let slot = if word == "--store" {
                &mut options.store
            } else {
                &mut options.config
            };
            if slot.replace(PathBuf::from(words.next()?)).is_some() {
                return None;
            }
        } else if word.is_empty() || word.as_encoded_bytes().starts_with(b"-") {
            return None;
        } else {
            options.paths.push(PathBuf::from(word));
        }
    }
    (!options.paths.is_empty()).then_some(options)
}

fn build_options(args: &[OsString]) -> Option<BuildOptions> {
    let mut words = Vec::new();
    let mut fetch = false;
    let mut keep_as = None;
    let mut rest = args.iter();
    while let Some(word) = rest.next() {
        if word == "--fetch" {
            if fetch {
                return None;
            }
            fetch = true;
        } else if word == "--keep-as" {
            let tag = rest.next()?.to_str()?.to_owned();
            if keep_as.replace(tag).is_some() {
                return None;
            }
        } else {
            words.push(word.clone());
            if word == "--library" || word == "--store" || word == "--config" {
                words.push(rest.next()?.clone());
            }
        }
    }
    let plan = plan_options(&words)?;
    if plan.json {
        return None;
    }
    Some(BuildOptions {
        path: plan.path,
        fetch,
        libraries: plan.libraries,
        store: plan.store,
        config: plan.config,
        keep_as,
    })
}

fn plan_options(args: &[OsString]) -> Option<PlanOptions> {
    let mut path = None;
    let mut options = PlanOptions {
        path: PathBuf::new(),
        json: false,
        libraries: Vec::new(),
        store: None,
        config: None,
    };
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if word == "--json" {
            if options.json {
                return None;
            }
            options.json = true;
        } else if word == "--library" {
            options.libraries.push(PathBuf::from(words.next()?));
        } else if word == "--store" || word == "--config" {
            let slot = if word == "--store" {
                &mut options.store
            } else {
                &mut options.config
            };
            if slot.replace(PathBuf::from(words.next()?)).is_some() {
                return None;
            }
        } else if word.is_empty() || word.as_encoded_bytes().starts_with(b"-") || path.is_some() {
            return None;
        } else {
            path = Some(PathBuf::from(word));
        }
    }
    options.path = path?;
    Some(options)
}

/// Parse the words after `recipe`.
pub fn parse(args: &[OsString]) -> RecipeCommand {
    let operand = |p: &OsString| !p.is_empty() && !p.as_encoded_bytes().starts_with(b"-");
    match args {
        [command, paths @ ..]
            if command == "check" && !paths.is_empty() && paths.iter().all(operand) =>
        {
            RecipeCommand::Check(paths.iter().map(PathBuf::from).collect())
        }
        [command, path] if command == "show" && operand(path) => {
            RecipeCommand::Show(PathBuf::from(path))
        }
        [command, rest @ ..] if command == "plan" => {
            plan_options(rest).map_or(RecipeCommand::Usage, RecipeCommand::Plan)
        }
        [command, rest @ ..] if command == "build" => {
            build_options(rest).map_or(RecipeCommand::Usage, RecipeCommand::Build)
        }
        [command, rest @ ..] if command == "pin" => match build_options(rest) {
            Some(options) if options.keep_as.is_none() => RecipeCommand::Pin(options),
            _ => RecipeCommand::Usage,
        },
        [command, rest @ ..] if command == "conformance" => {
            conformance_options(rest).map_or(RecipeCommand::Usage, RecipeCommand::Conformance)
        }
        _ => RecipeCommand::Usage,
    }
}

/// Run a parsed recipe command with every platform's recipe provider;
/// returns the exit code.
pub fn execute(
    command: RecipeCommand,
    io: &mut Io<'_>,
    host: &HostSources,
    platforms: &[&dyn CliPlatform],
) -> u8 {
    let providers: Vec<&dyn recipe::provider::Provider> = platforms
        .iter()
        .filter_map(|platform| platform.recipes())
        .collect();
    let providers = recipe::Providers(&providers);
    match command {
        RecipeCommand::Check(paths) => check(&paths, providers, &mut io.output, &mut io.errors),
        RecipeCommand::Show(path) => show(&path, providers, &mut io.output, &mut io.errors),
        RecipeCommand::Plan(options) => plan(&options, providers, host, io),
        RecipeCommand::Build(options) => {
            build_command(&options, false, providers, host, platforms, io)
        }
        RecipeCommand::Pin(options) => {
            build_command(&options, true, providers, host, platforms, io)
        }
        RecipeCommand::Conformance(options) => conformance(&options, providers, host, io),
        RecipeCommand::Usage => {
            let _ = io.errors.write_all(USAGE.as_bytes());
            2
        }
    }
}

/// `recipe build` or, with `pin`, `recipe pin`, with the downloader of
/// `--fetch`.
fn build_command(
    options: &BuildOptions,
    pin: bool,
    providers: recipe::Providers<'_>,
    host: &HostSources,
    platforms: &[&dyn CliPlatform],
    io: &mut Io<'_>,
) -> u8 {
    let fetcher = if options.fetch {
        match image_registry::download::Downloader::new() {
            Ok(downloader) => Some(HttpsFetcher(downloader)),
            Err(error) => {
                let _ = writeln!(
                    io.errors,
                    "upcli recipe {}: {error}",
                    if pin { "pin" } else { "build" }
                );
                return 2;
            }
        }
    } else {
        None
    };
    build(
        options,
        pin,
        providers,
        host,
        platforms,
        fetcher.as_ref().map(|d| d as &dyn recipe::build::Fetcher),
        io,
    )
}

fn check(
    paths: &[PathBuf],
    providers: recipe::Providers<'_>,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> u8 {
    let report = match recipe::check_paths(paths, providers) {
        Ok(report) => report,
        Err(error) => {
            let _ = writeln!(errors, "upcli recipe check: {error}");
            return 2;
        }
    };
    if output.write_all(report.render().as_bytes()).is_err() {
        return 2;
    }
    u8::from(!report.is_clean())
}

fn show(
    path: &Path,
    providers: recipe::Providers<'_>,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> u8 {
    if path.is_dir() {
        let _ = writeln!(
            errors,
            "upcli recipe show: {}: a folder; show takes one recipe file",
            path.display()
        );
        return 2;
    }
    let text = match recipe::show_path(path, providers) {
        Ok(recipe::Showing::Shown(recipe::Shown::Merged(merged))) => record(&merged, &[]),
        Ok(recipe::Showing::Locked {
            record: locked,
            drift,
        }) => record(&locked, &drift),
        // The check's own report: the findings, then the file's line.
        Ok(recipe::Showing::Shown(recipe::Shown::Refused(report))) => {
            return if output.write_all(report.render().as_bytes()).is_err() {
                2
            } else {
                1
            };
        }
        Ok(recipe::Showing::Shown(recipe::Shown::NotMedia(what))) => {
            let what = match what {
                recipe::What::Firmware => "a firmware recipe",
                recipe::What::Work => "a work file",
                recipe::What::Media => "a media recipe without a title record",
            };
            let _ = writeln!(
                errors,
                "upcli recipe show: {}: {what}; show prints the title record of a media recipe",
                path.display()
            );
            return 2;
        }
        Err(error) => {
            let _ = writeln!(errors, "upcli recipe show: {error}");
            return 2;
        }
    };
    if output.write_all(text.as_bytes()).is_err() {
        return 2;
    }
    0
}

/// Plan a recipe: open the store when there is one, scan the libraries
/// (the preferences' first, then each `--library`, a folder named twice
/// searched once), plan, keep the hashes in the index caches, print.
fn plan(
    options: &PlanOptions,
    providers: recipe::Providers<'_>,
    host: &HostSources,
    io: &mut Io<'_>,
) -> u8 {
    let fail = |io: &mut Io<'_>, error: &dyn std::fmt::Display| {
        let _ = writeln!(io.errors, "upcli recipe plan: {error:#}");
        2
    };
    if options.path.is_dir() {
        return fail(
            io,
            &format!(
                "{}: a folder; plan takes one recipe file",
                options.path.display()
            ),
        );
    }
    let store = match &options.store {
        Some(root) => match image_store::Store::open_existing(root) {
            Ok(store) => Some(store),
            Err(error) => return fail(io, &format!("{}: {error}", root.display())),
        },
        None => library::store_root(&None, host)
            .and_then(|root| image_store::Store::open_existing(root).ok()),
    };
    let index = match store.as_ref().map(recipe::store_index::StoreIndex::load) {
        Some(Ok(index)) => Some(index),
        Some(Err(error)) => return fail(io, &error),
        None => None,
    };
    let (mut libraries, caches) = match open_libraries(
        &options.config,
        &options.libraries,
        store.as_ref(),
        "plan",
        io,
    ) {
        Ok(opened) => opened,
        Err(error) => return fail(io, &error),
    };
    let downloads = store
        .as_ref()
        .map(|s| s.root().join(recipe::build::DOWNLOADS));
    let planned = recipe::plan_path(
        &options.path,
        providers,
        recipe::plan::Sources {
            store: index.as_ref(),
            downloads: downloads.as_deref(),
            libraries: &mut libraries,
        },
    );
    save_libraries(&libraries, &caches, "plan", io);
    let (text, code) = match planned {
        Ok(recipe::Planned::Plan(plan)) => {
            let code = u8::from(!plan.complete());
            let text = if options.json {
                plan.render_json()
            } else {
                plan.render()
            };
            (text, code)
        }
        Ok(recipe::Planned::Refused(report)) => {
            let text = if options.json {
                recipe::plan::refused_json(&options.path.display().to_string(), &report)
            } else {
                report.render()
            };
            (text, 1)
        }
        Ok(recipe::Planned::NotRecipe) => {
            return fail(
                io,
                &format!(
                    "{}: a work file; plan takes a recipe",
                    options.path.display()
                ),
            )
        }
        Err(error) => return fail(io, &error),
    };
    if io.output.write_all(text.as_bytes()).is_err() {
        return 2;
    }
    code
}

/// The libraries a plan or build searches: the preferences' first, then
/// each `--library`, a folder named twice searched once, each scanned with
/// its index cache in `store`. A library of the preferences that is not
/// there is skipped with a warning; one named on the command line must be
/// there.
#[allow(clippy::type_complexity)]
fn open_libraries(
    config: &Option<PathBuf>,
    named: &[PathBuf],
    store: Option<&image_store::Store>,
    command: &str,
    io: &mut Io<'_>,
) -> anyhow::Result<(Vec<Library>, Vec<Option<PathBuf>>)> {
    let configured = library::preferences(config).and_then(|path| library::configured(&path))?;
    let mut seen = std::collections::BTreeSet::new();
    let mut libraries = Vec::new();
    let mut caches = Vec::new();
    let all = configured
        .iter()
        .map(|dir| (dir, true))
        .chain(named.iter().map(|dir| (dir, false)));
    for (dir, from_preferences) in all {
        let absolute = match dir.canonicalize() {
            Ok(path) => path,
            Err(error) if from_preferences => {
                let _ = writeln!(
                    io.errors,
                    "upcli recipe {command}: warning: library {} skipped: {error}",
                    dir.display()
                );
                continue;
            }
            Err(error) => anyhow::bail!("{}: {error}", dir.display()),
        };
        if !seen.insert(absolute.clone()) {
            continue;
        }
        let cache = store.map(|s| recipe::library::cache_file(s.root(), &absolute));
        libraries.push(Library::scan(dir, cache.as_deref())?);
        caches.push(cache);
    }
    Ok((libraries, caches))
}

/// Keep the hashes a plan or build computed in the libraries' index caches.
fn save_libraries(
    libraries: &[Library],
    caches: &[Option<PathBuf>],
    command: &str,
    io: &mut Io<'_>,
) {
    for (library, cache) in libraries.iter().zip(caches) {
        if let Some(cache) = cache {
            if let Err(error) = library.save(cache) {
                let _ = writeln!(
                    io.errors,
                    "upcli recipe {command}: warning: the index of {} was not kept: {error}",
                    library.named.display()
                );
            }
        }
    }
}

/// The downloads of `--fetch`: the registry crate's HTTPS client.
pub struct HttpsFetcher(pub image_registry::download::Downloader);

impl recipe::build::Fetcher for HttpsFetcher {
    fn fetch(&self, url: &str, size: u64, sink: &mut dyn Write) -> Result<(), String> {
        self.0.fetch(url, size, sink)
    }
}

/// Build a recipe into the store, or with `pin` pin it: `--store`, or the
/// default store, created when it does not exist yet.
fn build(
    options: &BuildOptions,
    pin: bool,
    providers: recipe::Providers<'_>,
    host: &HostSources,
    platforms: &[&dyn CliPlatform],
    fetcher: Option<&dyn recipe::build::Fetcher>,
    io: &mut Io<'_>,
) -> u8 {
    let command = if pin { "pin" } else { "build" };
    let fail = |io: &mut Io<'_>, error: &dyn std::fmt::Display| {
        let _ = writeln!(io.errors, "upcli recipe {command}: {error:#}");
        2
    };
    if options.path.is_dir() {
        return fail(
            io,
            &format!(
                "{}: a folder; {command} takes one recipe file",
                options.path.display()
            ),
        );
    }
    let Some(root) = library::store_root(&options.store, host) else {
        return fail(
            io,
            &format!("a {command} needs a store: name one with --store"),
        );
    };
    let store = match image_store::Store::open(&root) {
        Ok(store) => store,
        Err(error) => return fail(io, &format!("{}: {error}", root.display())),
    };
    let index = match recipe::store_index::StoreIndex::load(&store) {
        Ok(index) => index,
        Err(error) => return fail(io, &error),
    };
    let (mut libraries, caches) = match open_libraries(
        &options.config,
        &options.libraries,
        Some(&store),
        command,
        io,
    ) {
        Ok(opened) => opened,
        Err(error) => return fail(io, &error),
    };
    let content = recipe::build::StoreContent::new(
        &store,
        platforms.iter().flat_map(|p| p.validators()).collect(),
    );
    // The fetch's list is printed as it is made, before the first download.
    let output = &mut *io.output;
    let request = recipe::build::Request {
        path: &options.path,
        providers,
        store: &store,
        index: Some(&index),
        store_read: Some(&content),
        libraries: &mut libraries,
        fetcher,
        keep_as: options.keep_as.as_deref(),
        find_outputs: true,
    };
    let mut log = |line: &str| {
        let _ = writeln!(output, "{line}");
        let _ = output.flush();
    };
    let outcome = if pin {
        match recipe::pin::pin_path(request, &mut log) {
            Ok(recipe::pin::Pinned::NotBuilt(outcome)) => Ok(*outcome),
            Ok(recipe::pin::Pinned::Already(digest)) => {
                let _ = writeln!(
                    io.errors,
                    "upcli recipe pin: {}: already pinned to {digest}; a changed artifact is a new revision: raise revision and remove digest, then pin",
                    options.path.display()
                );
                save_libraries(&libraries, &caches, command, io);
                return 1;
            }
            Ok(recipe::pin::Pinned::Pinned {
                report,
                digest,
                lock,
                files,
            }) => {
                save_libraries(&libraries, &caches, command, io);
                let text = format!(
                    "{}pinned {}: digest {digest} written into {}, lock {} ({files} file{})\n",
                    report.render(),
                    report.reference,
                    options.path.display(),
                    lock.display(),
                    if files == 1 { "" } else { "s" }
                );
                return u8::from(io.output.write_all(text.as_bytes()).is_err()) * 2;
            }
            Err(error) => Err(error),
        }
    } else {
        recipe::build::build_path(request, &mut log)
    };
    save_libraries(&libraries, &caches, command, io);
    let (text, code) = match outcome {
        Ok(recipe::build::Outcome::Built(report)) => (report.render(), 0),
        Ok(recipe::build::Outcome::Missing(plan)) => (
            format!(
                "{}not built: {}; nothing was tagged{}\n",
                plan.render(),
                plan.summary(),
                if fetcher.is_none() && plan.inputs.iter().any(|i| !i.urls.is_empty()) {
                    "; --fetch downloads the inputs that have URLs"
                } else {
                    ""
                }
            ),
            1,
        ),
        Ok(recipe::build::Outcome::Failed { plan, error }) => {
            let _ = writeln!(io.errors, "upcli recipe {command}: {error}");
            (
                format!(
                    "{}: not built: {}; nothing was tagged\n",
                    plan.file,
                    plan.name.as_deref().unwrap_or("(no name)")
                ),
                1,
            )
        }
        Ok(recipe::build::Outcome::Mismatch {
            plan,
            mismatches,
            kept,
        }) => {
            for mismatch in &mismatches {
                let _ = writeln!(io.errors, "upcli recipe {command}: {mismatch}");
            }
            let kept = match kept {
                Some((reference, published)) => format!(
                    "; the result of this build is kept as {reference}\t{}",
                    published.descriptor.digest
                ),
                None => String::new(),
            };
            (
                format!(
                    "{}: not built: {} differs from its lock; nothing was tagged under its tag{kept}\n",
                    plan.file,
                    plan.name.as_deref().unwrap_or("(no name)")
                ),
                1,
            )
        }
        Ok(recipe::build::Outcome::Refused(report)) => (report.render(), 1),
        Ok(recipe::build::Outcome::NotRecipe) => {
            return fail(
                io,
                &format!(
                    "{}: a work file; {command} takes a recipe",
                    options.path.display()
                ),
            )
        }
        Err(error) => return fail(io, &error),
    };
    if io.output.write_all(text.as_bytes()).is_err() {
        return 2;
    }
    code
}

/// Build every pinned recipe named into a scratch store under the store's
/// cache, from the libraries only, compare the digests, and remove the
/// scratch store.
fn conformance(
    options: &ConformanceOptions,
    providers: recipe::Providers<'_>,
    host: &HostSources,
    io: &mut Io<'_>,
) -> u8 {
    let fail = |io: &mut Io<'_>, error: &dyn std::fmt::Display| {
        let _ = writeln!(io.errors, "upcli recipe conformance: {error:#}");
        2
    };
    let Some(root) = library::store_root(&options.store, host) else {
        return fail(
            io,
            &"conformance needs a store for its scratch builds: name one with --store",
        );
    };
    let store = match image_store::Store::open(&root) {
        Ok(store) => store,
        Err(error) => return fail(io, &format!("{}: {error}", root.display())),
    };
    let (mut libraries, caches) = match open_libraries(
        &options.config,
        &options.libraries,
        Some(&store),
        "conformance",
        io,
    ) {
        Ok(opened) => opened,
        Err(error) => return fail(io, &error),
    };
    let scratch_root = root.join(CONFORMANCE).join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&scratch_root);
    let results = image_store::Store::open(&scratch_root)
        .map_err(|e| std::io::Error::other(format!("{}: {e}", scratch_root.display())))
        .and_then(|scratch| {
            recipe::pin::conformance(&options.paths, providers, &scratch, &mut libraries)
        });
    let _ = std::fs::remove_dir_all(&scratch_root);
    save_libraries(&libraries, &caches, "conformance", io);
    let results = match results {
        Ok(results) => results,
        Err(error) => return fail(io, &error),
    };
    let (text, all) = recipe::pin::render(&results);
    if io.output.write_all(text.as_bytes()).is_err() {
        return 2;
    }
    // Nothing pinned found proves nothing: a run that was meant to check a
    // release must not pass by finding no recipe.
    if !results
        .iter()
        .any(|r| !matches!(r.result, recipe::pin::Conformed::Draft))
    {
        let _ = writeln!(
            io.errors,
            "upcli recipe conformance: no pinned recipe found in the files and folders named"
        );
        return 1;
    }
    u8::from(!all)
}

/// Where `recipe conformance` keeps its scratch stores, in the store's
/// cache, one per process, removed when the run ends.
const CONFORMANCE: &str = "cache/recipe-conformance";

/// The merged record, one line per value as `upcli image media show`
/// writes a set's catalogue fields: a field name, a tab and the value, text
/// quoted, in that command's names and order, with the title fields it
/// leaves out beside their neighbours. The first line names the tag, the
/// digest (`draft` without one) and the platform. A value that came from
/// the work file ends in a comment that names the file. A pinned recipe's
/// record is its lock's: a comment line after the first says so, and one
/// line per field of `drift` names what the work file now says instead.
fn record(merged: &Merged, drift: &[recipe::lock::Difference]) -> String {
    let work = merged.work_file.as_deref().unwrap_or_default();
    let mut lines: Vec<(String, Origin)> = Vec::new();
    let mut push = |text: String, from: Origin| lines.push((text, from));
    let recipe = Origin::Recipe;
    let one = |push: &mut dyn FnMut(String, Origin), key: &str, v: &Option<Sourced<String>>| {
        if let Some(v) = v {
            push(format!("{key}\t{:?}", v.value), v.from);
        }
    };
    let each =
        |push: &mut dyn FnMut(String, Origin), key: &str, v: &Option<Sourced<Vec<String>>>| {
            for value in v
                .iter()
                .flat_map(|v| v.value.iter().map(move |i| (i, v.from)))
            {
                push(format!("{key}\t{:?}", value.0), value.1);
            }
        };
    push(
        format!("title\t{:?}", merged.title.value),
        merged.title.from,
    );
    one(&mut push, "sort-title", &merged.sort_title);
    one(&mut push, "series", &merged.series);
    each(&mut push, "publisher", &merged.publishers);
    each(&mut push, "developer", &merged.developers);
    if let Some(year) = merged.year {
        push(format!("year\t{year}"), recipe);
    }
    each(&mut push, "genre", &merged.genres);
    one(&mut push, "platform", &merged.software_platform);
    if let Some(p) = &merged.players {
        push(format!("players\t{}\t{}", p.value.min, p.value.max), p.from);
    }
    for language in &merged.languages {
        push(format!("language\t{language:?}"), recipe);
    }
    for region in &merged.regions {
        push(format!("region\t{region:?}"), recipe);
    }
    one(&mut push, "description", &merged.description);
    for (key, value) in [
        ("version", &merged.version),
        ("edition", &merged.edition),
        ("source-media", &merged.source_media),
        ("requirements", &merged.requirements),
    ] {
        if let Some(value) = value {
            push(format!("{key}\t{value:?}"), recipe);
        }
    }
    for (kind, values) in &merged.needs {
        push(
            format!("need\t{kind}\t{}", values.value.join(",")),
            values.from,
        );
    }
    if let Some(id) = &merged.work {
        push(format!("work\t{id:?}"), recipe);
    }
    if let Some(synopsis) = &merged.synopsis {
        // The work file's own field, unless the record is the lock's.
        let from = if merged.title.from == Origin::Lock {
            Origin::Lock
        } else {
            Origin::Work
        };
        push(format!("synopsis\t{synopsis:?}"), from);
    }
    for (catalogue, id) in &merged.ids {
        push(format!("work-id\t{catalogue}\t{:?}", id.value), id.from);
    }
    for label in &merged.labels {
        push(format!("set-label\t{:?}", label.value), label.from);
    }
    let mut out = format!(
        "{}:{}\t{}\t{}\n",
        merged.name,
        merged.tag(),
        merged.digest.as_deref().unwrap_or("draft"),
        image_store::platform::canonical(&merged.platform)
    );
    if merged.title.from == Origin::Lock {
        out.push_str(&format!(
            "# locked: the record of {}, which a build writes; the work file's edits wait for a new revision\n",
            merged.digest.as_deref().unwrap_or_default()
        ));
        for difference in drift {
            out.push_str(&format!("# drift: {difference}\n"));
        }
    }
    for (text, from) in lines {
        out.push_str(&text);
        if from == Origin::Work {
            out.push_str(&format!("\t# from {work}"));
        }
        out.push('\n');
    }
    out
}
