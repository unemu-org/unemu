// SPDX-License-Identifier: AGPL-3.0-or-later
//! The IBM PC/AT family's part of the command line
//! (`backlog/docs/design/core-contract.md` §7.1 and §7.2): the
//! [`CliPlatform`] of the PC, which owns the parse and execution of every
//! PC command (`--boot`, `--bios`, `machine check`, `run --machine`, `image
//! import`, `image run`, `image publish`, `image workspace
//! abandon-publication`, `image machine`, `image media import`, `image
//! firmware import` and `edit`, `image template build`, `media create`) and
//! answers the neutral commands' questions: the PC's validators (its
//! provider list, [`pc_image::with_providers`]), the firmware catalogue's
//! restrictions on a push, the compression that keeps the PC import record,
//! the images PC workspaces and machines pin, the PC recipe provider and
//! the names of the PC media formats.
//!
//! The crate is in the PC layer (core-contract.md §7.3): it builds on the
//! neutral command line of `cli-core` and on the PC and emulation crates, and
//! the `upcli` program composes it with the other platforms compiled in.
//! [`command`] is the command line with this platform alone, which the tests
//! of the PC commands drive.

pub mod command;
pub mod image;
pub mod run;

/// Image creation lives with the PC provider, which the desktop calls too.
pub use pc_image::media_create as create;

use cli_core::{CliPlatform, HostSources, Io, PlatformCommand, UsageSlot};
use image_store::{ConfigValidator, Descriptor, Digest, Manifest, Store};
use run::FirmwareStore;
use std::ffi::OsString;
use std::path::PathBuf;

/// The PC platform of the command line, holding the PC's image providers.
pub struct PcPlatform<'a> {
    validators: &'a [&'a dyn ConfigValidator],
}

/// Run `job` with the PC platform, its validators taken from the PC's
/// provider list ([`pc_image::with_providers`]), the list the desktop's
/// providers name for this platform too.
pub fn with_platform<T>(job: impl FnOnce(&dyn CliPlatform) -> T) -> T {
    pc_image::with_providers(|validators| job(&PcPlatform { validators }))
}

impl CliPlatform for PcPlatform<'_> {
    fn platform(&self) -> &str {
        pc_image::PLATFORM
    }

    fn validators(&self) -> Vec<&dyn ConfigValidator> {
        self.validators.to_vec()
    }

    fn restricted(&self, _: &Store, manifest: &Manifest) -> image_store::Result<Vec<Digest>> {
        pc_image::firmware::restricted(manifest, pc_image::firmware::catalogue())
    }

    /// A set or package is compressed with its own validator (packages from
    /// older stores keep raw ROMs until compressed); a machine image keeps
    /// its import record ([`pc_image::compress_image`]).
    fn compress(&self, store: &Store, source: &str, target: &str) -> anyhow::Result<Descriptor> {
        use image_store::BlobSource;
        let descriptor = store.tag_descriptor(source)?;
        let manifest = Manifest::decode_for_summary(
            &store.read_verified(&descriptor, image_store::MAX_SMALL_BLOB)?,
        )?;
        Ok(match manifest.artifact_type() {
            image_store::media_type::MEDIA_SET => store.compress_image(
                source,
                target,
                &image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats),
            )?,
            image_store::media_type::FIRMWARE => store.compress_image(
                source,
                target,
                &image_store::firmware::Validator(&pc_image::firmware::PcFirmwareRules(
                    pc_image::firmware::catalogue(),
                )),
            )?,
            _ => pc_image::compress_image(store, source, target)?,
        })
    }

    /// Each workspace's pinned image, then each machine's.
    fn pinned(&self, store: &Store) -> anyhow::Result<Vec<Digest>> {
        let mut pinned = Vec::new();
        for name in store.list_workspaces()? {
            if let Some(digest) = pc_image::workspace::pinned_image_at(store, &name)? {
                pinned.push(digest);
            }
        }
        pinned.extend(
            store
                .read_machines()?
                .images()
                .map(|image| image.digest.clone()),
        );
        Ok(pinned)
    }

    fn recipes(&self) -> Option<&dyn recipe::provider::Provider> {
        Some(&pc_image::recipe::PcRecipes)
    }

    fn medium_format(&self, format: &toml::Table) -> anyhow::Result<&'static str> {
        Ok(pc_image::media_set::format_name(format)?)
    }

    fn usage(&self, slot: UsageSlot) -> &str {
        match slot {
        UsageSlot::First => "       upcli --boot (Real16 timer demo)\n",
        UsageSlot::AfterAbout => concat!(
            "       upcli --bios PATH [--allow-unlisted-firmware]\n",
            "       upcli machine check FILE [--allow-unlisted-firmware] [--rom-dir DIR]...\n",
            "       upcli run --machine FILE [--store DIR | --no-store] [--rom-dir DIR]... [--max-steps N] [--idle-limit N] [--print CARD] [--allow-unlisted-firmware]\n",
            "       upcli image import FILE --store DIR --tag NAME:TAG [--create] [--no-compress] [--rom-dir DIR]... [--recover MEDIUM=tip|MEDIUM=generation:N]... [--min-app-version X.Y.Z]\n",
            "         --min-app-version (here, on image media import and on image firmware import and edit) raises the oldest unemupc the image declares it needs: never below what this release computes, never above this release\n",
        ),
        UsageSlot::AfterImageSweep => "       upcli image run NAME:TAG --store DIR --workspace NAME [--new] [--max-steps N]\n",
        UsageSlot::AfterWorkspaceDiscard => concat!(
            "       upcli image workspace abandon-publication NAME --store DIR\n",
            "       upcli image publish --from-workspace NAME --store DIR --tag NAME:TAG [--flatten]\n",
            "       upcli image machine ls --store DIR\n",
            "       upcli image machine new NAME:TAG --store DIR [--name NAME]\n",
            "       upcli image machine rename ID NAME --store DIR\n",
            "       upcli image machine duplicate ID --store DIR [--name NAME]\n",
            "       upcli image machine reset ID --store DIR --yes\n",
            "       upcli image machine rm ID --store DIR --yes\n",
            "       upcli image machine settings ID --store DIR [--set PATH=VALUE]... [--clear] [--dry-run]\n",
            "       upcli image machine commit ID --store DIR --tag NAME:TAG [--title TITLE] [--keep-on-old-image]\n",
            "       upcli image machine run ID --store DIR [--max-steps N]\n",
            "       upcli image media import FILE...|DIR --title TITLE --tag NAME:TAG --store DIR [--create] [--dry-run] [--no-compress] [--label N=TEXT]... [--format N=FORMAT]... [--extra PATH[:DOSNAME]]... [--open --license LicenseRef-NAME --license-text EXTRA] [--open --license EXPRESSION --license-text EXTRA --source EXTRA] [--min-app-version X.Y.Z] [SET-METADATA]\n",
            "         SET-METADATA: [--publisher P]... [--developer D]... [--year YYYY] [--genre G]... [--platform PLATFORM] [--description TEXT] [--version V] [--edition E] [--set-label L]...\n",
            "                       [--need KIND=VALUE[,VALUE]...]... [--requirements TEXT] [--work ID] [--synopsis TEXT] [--work-id wikidata|mobygames|igdb=ID]...\n",
            "       upcli image firmware import FOLDER|FILES... [--store DIR] [--create] [--rescan] [--dry-run] [--for CATALOGUE-ID [EXTRAS] [METADATA] [--min-app-version X.Y.Z]]\n",
            "       upcli image firmware import FILE --unlisted ID --role ROLE [--store DIR] [--create] [--dry-run] [EXTRAS] [METADATA] [--min-app-version X.Y.Z]\n",
            "       upcli image firmware edit NAME:TAG [--to NAME:TAG] [--store DIR] [--regenerate-readme] [METADATA] [--min-app-version X.Y.Z]\n",
            "         EXTRAS: [--extra PATH[:DOSNAME] [--extra-license SPDX]]...\n",
            "         METADATA: [--title T] [--description T] [--notes T] [--vendor T] [--version T] [--date YYYY-MM-DD]\n",
            "                   [--label L]... [--board B]... [--class C]... [--requires KIND]...\n",
            "                   [--dumped-from T | --downloaded-from T | --built-from T] [--revision R] [--url U] [--provenance-date YYYY-MM-DD]\n",
            "                   [--open [--license SPDX] | --user-supplied] [--source ARCHIVE.tar|.zip [--source-license SPDX]]\n",
        ),
        UsageSlot::AfterRegistry => "       upcli image template build --store DIR [--vendor DIR] [--dry-run]\n",
        UsageSlot::AfterRecipe => concat!(
            "       upcli media create floppy --type 1.44M|1.2M|720K|360K OUT [--format] [--label NAME] [--force]\n",
            "       upcli media create floppy --type 1.44M|1.2M|720K|360K OUT --add PATH [--add PATH]... [--top NAME] [--label NAME] [--split] [--force]\n",
            "         --add copies host files and folders (folders whole, symlinks never followed) onto a new FAT12 disk with 8.3 names;\n",
            "         --top NAME puts them all under one folder; --split writes <stem>-1ofN.<ext>, <stem>-2ofN.<ext>... when they need more than one disk\n",
            "       upcli media create hd (--chs C/H/S | --size SIZE[MiB|GiB]) OUT [--force]\n",
        ),
        UsageSlot::Last => "DOS boot and interactive sessions: see crates/pc-harness/tests/*dos*golden.rs\n",
        }
    }

    fn parse(&self, args: &[OsString]) -> Option<Result<Box<dyn PlatformCommand>, &'static str>> {
        parse(args)
            .map(|parsed| parsed.map(|command| Box::new(command) as Box<dyn PlatformCommand>))
    }
}

/// A parsed PC command.
#[derive(Debug, PartialEq, Eq)]
pub enum PcCommand {
    Boot,
    Bios {
        path: PathBuf,
        options: assembly::AssemblyOptions,
    },
    Check {
        rom_dirs: Vec<PathBuf>,
        path: PathBuf,
        options: assembly::AssemblyOptions,
    },
    Run {
        store: FirmwareStore,
        rom_dirs: Vec<PathBuf>,
        path: PathBuf,
        max_steps: u64,
        idle_limit: u64,
        print: Option<String>,
        options: assembly::AssemblyOptions,
    },
    Create(crate::create::CreateCommand),
    Image(image::PcImageCommand),
}

/// Parse a PC command line, or `None` when the words are not the PC's.
/// `--allow-unlisted-firmware` and `--rom-dir DIR` may appear anywhere
/// outside `image`; a `--rom-dir` on any other command line is refused here.
pub fn parse(args: &[OsString]) -> Option<Result<PcCommand, &'static str>> {
    if args.first().is_some_and(|arg| arg == "image") {
        return image::parse(&args[1..]).map(|parsed| parsed.map(PcCommand::Image));
    }
    machine_words(args).transpose()
}

/// The machine runs and `media create`; `Ok(None)` for words that are not
/// the PC's.
fn machine_words(args: &[OsString]) -> Result<Option<PcCommand>, &'static str> {
    let mut options = assembly::AssemblyOptions::default();
    let mut frontend = Vec::new();
    let mut rom_dirs = Vec::new();
    let mut arguments = args.iter();
    while let Some(arg) = arguments.next() {
        if arg == "--allow-unlisted-firmware" {
            options.allow_unlisted_firmware = true;
        } else if arg == "--rom-dir" {
            let path = arguments
                .next()
                .filter(|value| !value.is_empty() && !value.as_encoded_bytes().starts_with(b"--"))
                .ok_or("missing ROM directory")?;
            rom_dirs.push(PathBuf::from(path));
        } else {
            frontend.push(arg);
        }
    }
    if !rom_dirs.is_empty()
        && !frontend
            .first()
            .is_some_and(|arg| matches!(arg.to_str(), Some("run" | "machine")))
    {
        return Err("--rom-dir requires run or machine check");
    }
    if frontend.len() == 1 && frontend[0] == "--boot" {
        return Ok(Some(PcCommand::Boot));
    }
    if frontend.len() == 2 && frontend[0] == "--bios" {
        return Ok(Some(PcCommand::Bios {
            path: frontend[1].into(),
            options,
        }));
    }
    if frontend.first().is_some_and(|arg| *arg == "machine") {
        return if frontend.len() == 3
            && frontend[1] == "check"
            && !frontend[2].is_empty()
            && !frontend[2].as_encoded_bytes().starts_with(b"--")
        {
            Ok(Some(PcCommand::Check {
                rom_dirs,
                path: frontend[2].into(),
                options,
            }))
        } else {
            Err("expected machine check FILE")
        };
    }
    if frontend.first().is_some_and(|arg| *arg == "run") {
        let (mut path, mut max_steps, mut idle_limit, mut print) = (None, None, None, None);
        let mut store = None;
        let mut args = frontend[1..].iter();
        while let Some(arg) = args.next() {
            if *arg == "--no-store" {
                if store.replace(FirmwareStore::Disabled).is_some() {
                    return Err("duplicate or conflicting store option");
                }
                continue;
            }
            let value = args
                .next()
                .filter(|value| !value.is_empty() && !value.as_encoded_bytes().starts_with(b"--"))
                .ok_or("missing option value")?;
            match arg.to_str() {
                Some("--store") if store.is_none() => {
                    store = Some(FirmwareStore::Root(PathBuf::from(value)))
                }
                Some("--machine") if path.is_none() => path = Some(PathBuf::from(value)),
                Some("--max-steps") if max_steps.is_none() => max_steps = Some(decimal(value)?),
                Some("--idle-limit") if idle_limit.is_none() => idle_limit = Some(decimal(value)?),
                Some("--print") if print.is_none() => {
                    print = Some(value.to_str().ok_or("invalid card id")?.to_owned())
                }
                _ => return Err("unknown or duplicate run option"),
            }
        }
        return Ok(Some(PcCommand::Run {
            store: store.unwrap_or(FirmwareStore::Default),
            rom_dirs,
            path: path.ok_or("run requires --machine FILE")?,
            max_steps: max_steps.unwrap_or(pc_harness::BIOS_MAX_RETIRED),
            idle_limit: idle_limit.unwrap_or(pc_harness::BIOS_IDLE_LIMIT),
            print,
            options,
        }));
    }
    if args.len() >= 2 && args[0] == "media" && args[1] == "create" {
        return crate::create::parse(&args[2..]).map(|c| Some(PcCommand::Create(c)));
    }
    Ok(None)
}

fn decimal(value: &std::ffi::OsStr) -> Result<u64, &'static str> {
    value
        .to_str()
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("expected an unsigned decimal integer")?
        .parse()
        .map_err(|_| "integer is out of range")
}

impl PlatformCommand for PcCommand {
    /// Each command's error reads as it always did: a BIOS run's with its
    /// causes after `Error: `, a machine check's and a run's as one line,
    /// an image command's with its causes.
    fn execute(self: Box<Self>, io: &mut Io<'_>, host: &HostSources) -> anyhow::Result<u8> {
        let line = |error: &dyn std::fmt::Display| anyhow::Error::msg(error.to_string());
        match *self {
            PcCommand::Boot => {
                let serial =
                    pc_harness::run_boot(pc_harness::BOOT_PROGRAM).map_err(|e| line(&e))?;
                io.output.write_all(&serial).map_err(|e| line(&e))?;
                Ok(0)
            }
            PcCommand::Bios { path, options } => {
                run::run_bios(&path, options, &mut io.output, &mut io.errors)
                    .map_err(|error| anyhow::Error::msg(format!("Error: {error:?}")))
            }
            PcCommand::Check {
                path,
                options,
                rom_dirs,
            } => {
                run::check_machine(&path, options, &rom_dirs, &mut io.errors).map_err(|e| line(&e))
            }
            PcCommand::Run {
                store,
                path,
                rom_dirs,
                max_steps,
                idle_limit,
                print,
                options,
            } => run::run_machine(
                &path,
                options,
                (max_steps, idle_limit),
                print.as_deref(),
                (
                    &rom_dirs,
                    match &store {
                        FirmwareStore::Default => host.store_root.as_deref(),
                        FirmwareStore::Disabled => None,
                        FirmwareStore::Root(path) => Some(path.as_path()),
                    },
                ),
                &mut io.output,
                &mut io.errors,
            )
            .map_err(|e| line(&e)),
            PcCommand::Create(command) => {
                crate::create::execute(command, &mut io.output).map_err(|e| line(&e))
            }
            PcCommand::Image(command) => image::execute(command, io, host),
        }
    }
}

/// Every local mark plus the firmware catalogue's: a blob whose bytes, stored
/// or decoded, the catalogue lists as a proprietary image or chip stays
/// user-supplied whatever the image's own marks say, so a pulled foreign
/// package that marks such a ROM open still cannot leave a private registry.
pub(crate) fn guard_marks(
    store: &Store,
    manifest: &Manifest,
    catalogue: &[assembly::FirmwareEntry],
) -> image_registry::Result<image_registry::guard::LocalMarks> {
    let mut marks = image_registry::guard::LocalMarks::scan(store)?;
    for digest in pc_image::firmware::restricted(manifest, catalogue)? {
        marks.restrict_catalogue(digest);
    }
    Ok(marks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_registry::{
        pull, push,
        test_support::{MockRegistry, Policy},
        Client, Reference,
    };
    use image_store::firmware;
    use pc_image::firmware::PcFirmwareRules;

    /// The neutral push guard learns the firmware catalogue through the PC
    /// platform: a blob with the catalogued hash and size of a proprietary
    /// BIOS stays user-supplied though the image marks it open. Only the
    /// catalogue's hash is used, never the BIOS's bytes.
    #[test]
    fn the_guard_restricts_a_catalogued_proprietary_bios_through_the_platform() {
        use image_store::{license, media_type};
        let entry = pc_image::firmware::catalogue()
            .iter()
            .find(|e| {
                e.distribution == assembly::FirmwareDistribution::UserSupplied
                    && e.sha256.len() == 64
            })
            .expect("a proprietary BIOS in the catalogue");
        let digest: Digest = format!("sha256:{}", entry.sha256).parse().unwrap();
        let mut rom = Descriptor::new(media_type::FIRMWARE, digest.clone(), entry.size);
        rom.annotations.extend([
            (license::REDISTRIBUTION.into(), license::OPEN.into()),
            (license::LICENSE.into(), "MIT".into()),
        ]);
        let manifest = Manifest::new(
            pc_image::PLATFORM,
            Descriptor::new(media_type::CONFIG, Digest::of(b"config"), 6),
            vec![
                rom,
                Descriptor::new(media_type::METADATA, Digest::of(b"{}"), 2),
            ],
        )
        .unwrap();
        let temp = assembly::test_support::TempDir::new("cli-pc-restricted");
        let store = Store::open(temp.path().join("store")).unwrap();
        with_platform(|pc| {
            assert_eq!(
                pc.restricted(&store, &manifest).unwrap(),
                vec![digest.clone()]
            );
            let marks = cli_core::image::guard_marks(&store, &manifest, &[pc]).unwrap();
            let closure = image_registry::guard::closure(&manifest, &marks);
            let rom = closure
                .iter()
                .find(|c| c.descriptor.digest == digest)
                .unwrap();
            assert_eq!(
                rom.effective,
                image_registry::guard::Redistribution::UserSupplied
            );
            assert_eq!(rom.source, image_registry::guard::MarkSource::Catalogue);
        });
    }

    /// The PC command a command line parses to.
    fn pc(args: &[OsString]) -> PcCommand {
        parse(args).expect("a PC command").unwrap()
    }

    #[test]
    fn file_run_parser_preserves_policy_defaults_and_explicit_limits() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            pc(&args(&["run", "--machine", "at.toml"])),
            PcCommand::Run {
                store: FirmwareStore::Default,
                rom_dirs: Vec::new(),
                path: "at.toml".into(),
                max_steps: 20_000_000,
                idle_limit: 1_000_000,
                print: None,
                options: assembly::AssemblyOptions::default(),
            }
        );
        assert_eq!(
            pc(&args(&[
                "run",
                "--machine",
                "at.toml",
                "--max-steps",
                "42",
                "--idle-limit",
                "0",
                "--print",
                "screen",
                "--allow-unlisted-firmware"
            ])),
            PcCommand::Run {
                store: FirmwareStore::Default,
                rom_dirs: Vec::new(),
                path: "at.toml".into(),
                max_steps: 42,
                idle_limit: 0,
                print: Some("screen".into()),
                options: assembly::AssemblyOptions {
                    allow_unlisted_firmware: true
                },
            }
        );
    }

    #[test]
    fn run_store_flags_are_explicit_and_exclusive() {
        let parse_run = |options: &[&str]| {
            let args = ["run", "--machine", "machine.toml"]
                .into_iter()
                .chain(options.iter().copied())
                .map(OsString::from)
                .collect::<Vec<_>>();
            parse(&args).unwrap()
        };
        assert!(matches!(
            parse_run(&[]).unwrap(),
            PcCommand::Run {
                store: FirmwareStore::Default,
                ..
            }
        ));
        assert!(matches!(
            parse_run(&["--no-store"]).unwrap(),
            PcCommand::Run {
                store: FirmwareStore::Disabled,
                ..
            }
        ));
        assert!(
            matches!(parse_run(&["--store","local"]).unwrap(),PcCommand::Run { store: FirmwareStore::Root(root), .. } if root.as_path() == std::path::Path::new("local"))
        );
        for options in [
            &["--store"][..],
            &["--no-store", "--no-store"],
            &["--store", "local", "--no-store"],
            &["--no-store", "--store", "local"],
        ] {
            assert!(parse_run(options).is_err());
        }
    }

    #[test]
    fn bios_parser_preserves_firmware_flags_and_boot_mode() {
        for args in [
            vec!["--bios", "firmware.bin"],
            vec!["--bios", "firmware.bin", "--allow-unlisted-firmware"],
            vec!["--allow-unlisted-firmware", "--bios", "firmware.bin"],
            vec![
                "--bios",
                "--allow-unlisted-firmware",
                "firmware.bin",
                "--allow-unlisted-firmware",
            ],
        ] {
            let allow = args.contains(&"--allow-unlisted-firmware");
            let args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
            assert_eq!(
                pc(&args),
                PcCommand::Bios {
                    path: PathBuf::from("firmware.bin"),
                    options: assembly::AssemblyOptions {
                        allow_unlisted_firmware: allow
                    },
                }
            );
        }
        assert_eq!(
            pc(&["--allow-unlisted-firmware".into(), "--boot".into()]),
            PcCommand::Boot
        );
        for args in [
            vec!["--bios"],
            vec!["--bios", "firmware.bin", "extra"],
            vec!["--allow-unlisted-firmware"],
        ] {
            let args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
            // `--allow-unlisted-firmware` alone is no PC command; the others are refused ones.
            assert!(parse(&args).is_none_or(|parsed| parsed.is_err()));
            assert!(crate::command::parse(&args).is_err());
        }
    }
    #[test]
    fn rom_directory_option_is_repeatable_for_launch_and_check() {
        for prefix in [
            vec!["run", "--machine", "machine.toml"],
            vec!["machine", "check", "machine.toml"],
        ] {
            let mut args: Vec<OsString> = prefix.into_iter().map(Into::into).collect();
            args.extend(
                ["--rom-dir", "first", "--rom-dir", "second"]
                    .into_iter()
                    .map(OsString::from),
            );
            let roots = match pc(&args) {
                PcCommand::Run { rom_dirs, .. } | PcCommand::Check { rom_dirs, .. } => rom_dirs,
                _ => panic!("machine command expected"),
            };
            assert_eq!(roots, [PathBuf::from("first"), PathBuf::from("second")]);
        }
    }
    /// A firmware package made where the catalogue does not know its ROM,
    /// marking it open under MIT, pushed to a registry and pulled back into
    /// another store. There the catalogue lists the same bytes as proprietary,
    /// so the guard keeps the pulled copy private whatever its marks say.
    #[test]
    fn a_pulled_package_marking_proprietary_firmware_open_cannot_leave() {
        let temp = assembly::test_support::TempDir::new("pulled-foreign-firmware");
        let bytes = vec![0x6b; 8192];
        let rom = temp.path().join("rom.bin");
        std::fs::write(&rom, &bytes).unwrap();
        let foreign = Store::open(temp.path().join("foreign")).unwrap();
        let candidate =
            pc_image::firmware_import::unlisted(&rom, "foreign-rom", "option-rom", &[]).unwrap();
        let statement = pc_image::firmware_import::Statement {
            open: true,
            license: Some("MIT".into()),
            ..Default::default()
        };
        pc_image::firmware_import::publish_with(&foreign, &candidate, &[], &statement, &[])
            .unwrap();
        let rules = PcFirmwareRules(&[]);
        let validator = firmware::Validator(&rules);
        let providers: [&dyn ConfigValidator; 1] = [&validator];
        let registry = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            anonymous_write: vec!["*".into()],
            ..Policy::default()
        });
        let entry = registry.entry(false);
        let client = Client::new(&entry, Box::new(image_registry::credentials::Anonymous)).unwrap();
        let reference = Reference::parse(
            &format!("{}/owner/rom:r1", registry.host()),
            "unused.example",
        )
        .unwrap();
        let plan = push::plan(
            &foreign,
            "private/firmware/ibm-pc/foreign-rom:r1",
            reference.clone(),
            Some(&entry),
        )
        .unwrap();
        assert_eq!(plan.verdict(), &Ok(()), "all open where the ROM is unknown");
        push::push(
            &foreign,
            &plan,
            &providers,
            &client,
            &push::PushOptions::default(),
            &mut |_, _| {},
        )
        .unwrap();
        let local = Store::open(temp.path().join("local")).unwrap();
        pull::pull(
            &local,
            &client,
            &reference,
            "pulled/rom:1",
            &providers,
            &|_| {},
        )
        .unwrap();
        let catalogue = [assembly::FirmwareEntry {
            id: "synthetic-vendor-option-rom",
            kind: assembly::FirmwareKind::OptionRom,
            sha256: Box::leak(
                image_store::Digest::of(&bytes)
                    .hex()
                    .to_owned()
                    .into_boxed_str(),
            ),
            size: 8192,
            distribution: assembly::FirmwareDistribution::UserSupplied,
            ..assembly::KISELEV_XI8088
        }];
        let image = local.tag_descriptor("pulled/rom:1").unwrap();
        let manifest = image_store::Manifest::decode_for_summary(
            &image_store::BlobSource::read_verified(&local, &image, image_store::MAX_SMALL_BLOB)
                .unwrap(),
        )
        .unwrap();
        for (target, refusal) in [
            ("owner/rom:2", "not marked private"),
            ("unemu/rom:r1", "official namespace"),
        ] {
            let target =
                Reference::parse(&format!("{}/{target}", registry.host()), "unused.example")
                    .unwrap();
            let marks = guard_marks(&local, &manifest, &catalogue).unwrap();
            let plan =
                push::plan_with_marks(&local, "pulled/rom:1", target, Some(&entry), marks).unwrap();
            let rom = plan
                .closure()
                .iter()
                .find(|c| c.descriptor.decoded_media_type() == image_store::media_type::FIRMWARE)
                .unwrap();
            assert_eq!(rom.mark, image_registry::guard::Redistribution::Open);
            assert_eq!(rom.source, image_registry::guard::MarkSource::Catalogue);
            let error = plan.verdict().clone().unwrap_err().to_string();
            assert!(error.contains(refusal), "{error}");
        }
        // Without the catalogue's knowledge the pulled copy reads as open.
        let marks = guard_marks(&local, &manifest, &[]).unwrap();
        let plan =
            push::plan_with_marks(&local, "pulled/rom:1", reference, Some(&entry), marks).unwrap();
        assert_eq!(plan.verdict(), &Ok(()));
    }
}
