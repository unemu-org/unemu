// SPDX-License-Identifier: AGPL-3.0-or-later
//! The IBM PC/AT family's part of the command line
//! (`backlog/docs/design/core-contract.md` §7.1 and §7.2): the
//! [`CliPlatform`] of the PC, which owns the parse and execution of every
//! PC command (`run --machine`, `image
//! import`, `image publish`, `image workspace
//! abandon-publication`, `image machine`, `image media import`, `image
//! firmware import` and `edit`, `image template build`, `media create`) and
//! answers the neutral commands' questions: the PC's validators (its
//! provider list, [`pc_image::with_providers`]), the firmware catalogue's
//! restrictions on a push, the compression that keeps the PC import record,
//! the images PC workspaces and machines pin, the PC recipe provider and
//! the names of the PC media formats, and ownership and preparation for the
//! neutral stored-image runners (core-contract.md §14.2.7).
//!
//! The crate is in the PC layer (core-contract.md §7.3): it builds on the
//! neutral command line of `cli-core` and on the PC and emulation crates, and
//! the `unemu` program composes it with the other platforms compiled in.
//! [`command`] is the command line with this platform alone, which the tests
//! of the PC commands drive.

pub mod command;
pub mod image;
/// Offline journal commands shared with the standalone PC command line.
pub use unemupc::journal;
/// Machine-run types shared with the standalone core command line.
pub use unemupc::run;

/// Image creation lives with the PC provider, which the desktop calls too.
pub use pc_image::media_create as create;

use cli_core::{CliPlatform, HostSources, Io, PlatformCommand, UsageSlot};
use image_store::{ConfigValidator, Descriptor, Digest, Manifest, Store};
use run::FirmwareStore;
use std::ffi::OsString;

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

    fn export_snapshot<'a>(
        &self,
        store: &'a Store,
        id: &str,
    ) -> anyhow::Result<Option<Box<dyn cli_core::image::machine_export::Snapshot + 'a>>> {
        let image = pc_image::machine::pinned_image(store, id)?;
        if !image_store::platform::same(
            &cli_core::image::run_platform(store, &image)?,
            self.platform(),
        ) {
            return Ok(None);
        }
        Ok(Some(Box::new(image::ExportSnapshot {
            snapshot: pc_image::machine::export::open(store, id)?,
            store,
        })))
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

    fn own_run<'a>(
        &self,
        store: &'a Store,
        request: &cli_core::image::RunRequest<'_>,
        errors: &mut dyn std::io::Write,
    ) -> anyhow::Result<Option<cli_core::image::OwnedRun<'a>>> {
        if request.tag.is_none() {
            let (_, migration) = pc_image::machine::open_store(store.root())?;
            for id in migration.adopted {
                writeln!(errors, "migrated:{id}")?;
            }
            for (name, reason) in migration.skipped {
                writeln!(errors, "migration-skipped:{name}\t{reason}")?;
            }
        }
        let is_machine =
            request.tag.is_none() || store.read_machines()?.get(request.workspace).is_some();
        let (owner, image) = if is_machine {
            if request.fresh {
                anyhow::bail!("--new requires a fresh workspace name");
            }
            if let Some(tag) = request.tag {
                let pinned = pc_image::machine::pinned_image(store, request.workspace)?;
                match store.tag_descriptor(tag) {
                    Ok(requested) if requested.digest != pinned.digest => anyhow::bail!(
                        "workspace pins {}; requested tag points to {}",
                        pinned.digest,
                        requested.digest
                    ),
                    Ok(_) | Err(image_store::Error::TagNotFound(_)) => (),
                    Err(error) => return Err(error.into()),
                }
            }
            let owner = pc_image::machine::own(store, request.workspace)?;
            let image = pc_image::machine::select_run_image(&owner)?;
            (owner, image)
        } else {
            let owner = store.workspace(request.workspace)?;
            let image = pc_image::workspace::select_run_image(
                &owner,
                request.tag.unwrap_or_default(),
                request.fresh,
                None,
            )?;
            (owner, image)
        };
        if is_machine {
            pc_image::workspace::select_run_image(&owner, "", false, Some(&image))?;
        }
        if !image_store::platform::same(
            &cli_core::image::run_platform(store, &image)?,
            self.platform(),
        ) {
            return Ok(None);
        }
        Ok(Some(cli_core::image::OwnedRun {
            owner,
            image,
            machine: is_machine,
        }))
    }

    fn prepare_run<'a>(
        &self,
        owner: &'a cli_core::image::OwnedRun<'_>,
        request: &cli_core::image::RunRequest<'_>,
        core: &'static dyn unemu_core_api::Core,
    ) -> anyhow::Result<cli_core::image::StoredRun<'a>> {
        image::prepare_run(owner, request, core)
    }

    fn usage(&self, slot: UsageSlot) -> &str {
        match slot {
        UsageSlot::First => "",
        UsageSlot::AfterAbout => MACHINE_AND_IMPORT_USAGE,
        UsageSlot::AfterImageSweep => "",
        UsageSlot::AfterWorkspaceDiscard => concat!(
            "       unemu image workspace abandon-publication NAME --store DIR\n",
            "       unemu image publish --from-workspace NAME --store DIR --tag NAME:TAG [--flatten]\n",
        ),
        UsageSlot::BeforeMachineRun => concat!(
            "       unemu image machine ls --store DIR\n",
            "       unemu image machine new NAME:TAG --store DIR [--name NAME]\n",
            "       unemu image machine rename ID NAME --store DIR\n",
            "       unemu image machine duplicate ID --store DIR [--name NAME]\n",
            "       unemu image machine reset ID --store DIR --yes\n",
            "       unemu image machine rm ID --store DIR --yes\n",
            "       unemu image machine settings ID --store DIR [--set PATH=VALUE]... [--clear] [--dry-run]\n",
            "       unemu image machine commit ID --store DIR --tag NAME:TAG [--title TITLE] [--keep-on-old-image]\n",
        ),
        UsageSlot::AfterMachineRun => concat!(
            "       unemu image media import FILE...|DIR --title TITLE --tag NAME:TAG --store DIR [--create] [--dry-run] [--no-compress] [--label N=TEXT]... [--format N=FORMAT]... [--extra PATH[:DOSNAME]]... [--open --license LicenseRef-NAME --license-text EXTRA] [--open --license EXPRESSION --license-text EXTRA --source EXTRA] [--min-app-version X.Y.Z] [SET-METADATA]\n",
            "         SET-METADATA: [--publisher P]... [--developer D]... [--year YYYY] [--genre G]... [--platform PLATFORM] [--description TEXT] [--version V] [--edition E] [--set-label L]...\n",
            "                       [--need KIND=VALUE[,VALUE]...]... [--requirements TEXT] [--work ID] [--synopsis TEXT] [--work-id wikidata|mobygames|igdb=ID]...\n",
            "       unemu image firmware import FOLDER|FILES... [--store DIR] [--create] [--rescan] [--dry-run] [--for CATALOGUE-ID [EXTRAS] [METADATA] [--min-app-version X.Y.Z]]\n",
            "       unemu image firmware import FILE --unlisted ID --role ROLE [--store DIR] [--create] [--dry-run] [EXTRAS] [METADATA] [--min-app-version X.Y.Z]\n",
            "       unemu image firmware edit NAME:TAG [--to NAME:TAG] [--store DIR] [--regenerate-readme] [METADATA] [--min-app-version X.Y.Z]\n",
            "         EXTRAS: [--extra PATH[:DOSNAME] [--extra-license SPDX]]...\n",
            "         METADATA: [--title T] [--description T] [--notes T] [--vendor T] [--version T] [--date YYYY-MM-DD]\n",
            "                   [--label L]... [--board B]... [--class C]... [--requires KIND]...\n",
            "                   [--dumped-from T | --downloaded-from T | --built-from T] [--revision R] [--url U] [--provenance-date YYYY-MM-DD]\n",
            "                   [--open [--license SPDX] | --user-supplied] [--source ARCHIVE.tar|.zip [--source-license SPDX]]\n",
        ),
        UsageSlot::AfterRegistry => concat!("       unemu image template build --store DIR [--vendor DIR]... [--dry-run]\n", "       unemu image template config --out DIR\n"),
        UsageSlot::AfterRecipe => concat!(
            "       unemu media create floppy --type 1.44M|1.2M|720K|360K OUT [--format] [--label NAME] [--force]\n",
            "       unemu media create floppy --type 1.44M|1.2M|720K|360K OUT --add PATH [--add PATH]... [--top NAME] [--label NAME] [--split] [--force]\n",
            "         --add copies host files and folders (folders whole, symlinks never followed) onto a new FAT12 disk with 8.3 names;\n",
            "         --top NAME puts them all under one folder; --split writes <stem>-1ofN.<ext>, <stem>-2ofN.<ext>... when they need more than one disk\n",
            "       unemu media create hd (--chs C/H/S | --size SIZE[MiB|GiB]) OUT [--force]\n",
        ),
        UsageSlot::Journal => unemupc::JOURNAL_USAGE,
        UsageSlot::Last => "DOS boot and interactive sessions: see the DOS goldens of the PC core (unemupc: crates/pc-harness/tests/*dos*golden.rs)\n",
        }
    }

    fn parse(&self, args: &[OsString]) -> Option<Result<Box<dyn PlatformCommand>, &'static str>> {
        parse(args)
            .map(|parsed| parsed.map(|command| Box::new(command) as Box<dyn PlatformCommand>))
    }
}

const MACHINE_AND_IMPORT_USAGE: &str = unemupc::machine_usage!(
            "       unemu image import FILE --store DIR --tag NAME:TAG [--create] [--no-compress] [--rom-dir DIR]... [--recover MEDIUM=tip|MEDIUM=generation:N]... [--min-app-version X.Y.Z]\n",
            "         --min-app-version (here, on image media import and on image firmware import and edit) raises the oldest unemupc the image declares it needs: never below what this release computes, never above this release\n",
);

/// A parsed PC command.
#[derive(PartialEq, Eq)]
pub enum PcCommand {
    Machine(unemupc::Command),
    Create(crate::create::CreateCommand),
    Image(image::PcImageCommand),
}

impl std::fmt::Debug for PcCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Machine(command) => command.fmt(f),
            Self::Create(command) => f.debug_tuple("Create").field(command).finish(),
            Self::Image(command) => f.debug_tuple("Image").field(command).finish(),
        }
    }
}

/// Parse PC commands, preserving the machine parser's refusal of misplaced
/// ROM options before trying media creation.
pub fn parse(args: &[OsString]) -> Option<Result<PcCommand, &'static str>> {
    if args.first().is_some_and(|arg| arg == "image") {
        return image::parse(&args[1..]).map(|parsed| parsed.map(PcCommand::Image));
    }
    if let Some(parsed) = unemupc::parse_with_store(args) {
        return Some(parsed.map(PcCommand::Machine));
    }
    if args.len() >= 2 && args[0] == "media" && args[1] == "create" {
        return Some(crate::create::parse(&args[2..]).map(PcCommand::Create));
    }
    None
}

impl PlatformCommand for PcCommand {
    fn execute(self: Box<Self>, io: &mut Io<'_>, host: &HostSources) -> anyhow::Result<u8> {
        match *self {
            PcCommand::Machine(command) => {
                let store_root = match &command {
                    unemupc::Command::Run { store, .. } => match store {
                        FirmwareStore::Default => host.store_root.clone(),
                        FirmwareStore::Disabled => None,
                        FirmwareStore::Root(path) => Some(path.clone()),
                    },
                    _ => None,
                };
                unemupc::execute_with_loader(
                    command,
                    &mut io.output,
                    &mut io.errors,
                    |path, options, dirs, errors| {
                        let roots = unemupc::run::firmware_roots(dirs);
                        let mut warnings = Vec::new();
                        let result = pc_image::firmware::prepare_for_assembly_with_compatibility(
                            path,
                            &roots,
                            &mut warnings,
                            store_root.as_deref(),
                            &host.compatibility,
                        )
                        .and_then(|document| document.load(options));
                        for warning in warnings {
                            writeln!(errors, "warning: {warning}")?;
                        }
                        Ok(result?)
                    },
                )
            }
            PcCommand::Create(command) => crate::create::execute(command, &mut io.output)
                .map_err(|error| anyhow::Error::msg(error.to_string())),
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
    let standalone;
    let store = if store.compatibility().frontend.is_none() {
        standalone = store
            .clone()
            .with_compatibility(pc_image::compat::compatibility(
                image_store::compat::EXTRACTION_BASELINE,
            ));
        &standalone
    } else {
        store
    };
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
        let temp = machine_file::test_support::TempDir::new("cli-pc-restricted");
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

    #[test]
    fn run_store_flags_are_explicit_and_exclusive() {
        assert!(parse(&["another-platform-command".into()]).is_none());
        let parse_run = |options: &[&str]| {
            let args = ["run", "--machine", "machine.toml"]
                .into_iter()
                .chain(options.iter().copied())
                .map(OsString::from)
                .collect::<Vec<_>>();
            parse(&args).unwrap().map(|command| match command {
                PcCommand::Machine(command) => command,
                _ => panic!("machine command expected"),
            })
        };
        assert!(matches!(
            parse_run(&[]).unwrap(),
            unemupc::Command::Run {
                store: FirmwareStore::Default,
                ..
            }
        ));
        assert!(matches!(
            parse_run(&["--no-store"]).unwrap(),
            unemupc::Command::Run {
                store: FirmwareStore::Disabled,
                ..
            }
        ));
        assert!(
            matches!(parse_run(&["--store","local"]).unwrap(),unemupc::Command::Run { store: FirmwareStore::Root(root), .. } if root.as_path() == std::path::Path::new("local"))
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
    fn firmware_push_planning_uses_the_injected_pc_core_release() {
        let temp = machine_file::test_support::TempDir::new("firmware-push-release");
        let rom = temp.path().join("rom.bin");
        std::fs::write(&rom, [0x6b; 8192]).unwrap();
        let store = Store::open(temp.path().join("store"))
            .unwrap()
            .with_compatibility(pc_image::compat::compatibility(
                image_store::compat::EXTRACTION_BASELINE,
            ));
        let candidate =
            pc_image::firmware_import::unlisted(&rom, "foreign-rom", "option-rom", &[]).unwrap();
        let statement = pc_image::firmware_import::Statement {
            open: true,
            license: Some("MIT".into()),
            ..Default::default()
        };
        pc_image::firmware_import::publish_with(&store, &candidate, &[], &statement, &[]).unwrap();
        let entry = image_registry::RegistryEntry::builtin("registry.example");
        let plan = push::plan(
            &store,
            "private/firmware/ibm-pc/foreign-rom:r1",
            Reference::parse("registry.example/owner/rom:r1", "unused.example").unwrap(),
            Some(&entry),
        )
        .unwrap();
        assert_eq!(plan.verdict(), &Ok(()));
        let unconfigured = Store::open(store.root()).unwrap();
        assert!(matches!(
            push::plan(
                &unconfigured,
                "private/firmware/ibm-pc/foreign-rom:r1",
                plan.target().clone(),
                Some(&entry),
            ),
            Err(image_registry::Error::Store(
                image_store::Error::NeedsNewer {
                    component: image_store::compat::Component::Core,
                    ..
                }
            ))
        ));
    }

    /// A firmware package made where the catalogue does not know its ROM,
    /// marking it open under MIT, pushed to a registry and pulled back into
    /// another store. There the catalogue lists the same bytes as proprietary,
    /// so the guard keeps the pulled copy private whatever its marks say.
    #[test]
    fn a_pulled_package_marking_proprietary_firmware_open_cannot_leave() {
        let temp = machine_file::test_support::TempDir::new("pulled-foreign-firmware");
        let bytes = vec![0x6b; 8192];
        let rom = temp.path().join("rom.bin");
        std::fs::write(&rom, &bytes).unwrap();
        let foreign = Store::open(temp.path().join("foreign"))
            .unwrap()
            .with_compatibility(pc_image::compat::compatibility(
                image_store::compat::EXTRACTION_BASELINE,
            ));
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
        let local = Store::open(temp.path().join("local"))
            .unwrap()
            .with_compatibility(foreign.compatibility().clone());
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
