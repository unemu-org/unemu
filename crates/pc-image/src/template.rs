// SPDX-License-Identifier: AGPL-3.0-or-later
//! Official template images, built reproducibly from the fetched open
//! fixtures; machine-images §5.5.
//!
//! A build imports the open firmware packages with the source archives their
//! fetch scripts build (the archives the firmware catalogue pins), then
//! creates each template machine through [`crate::create`], never from a
//! machine file. Nothing depends on the clock, the host or the fixture paths:
//! the same fixtures give the same manifests and digests on every run and
//! host. Each image is offered to a caller's guard before anything of it is
//! tagged, so a build can refuse what the official namespace would refuse.
//! A template holds firmware and blank zero-base disks only; operating
//! systems on disk images need their own licence decision.
//!
//! The machines are IBM 5170-class boards, as the IBM Personal Computer AT
//! Technical Reference, System Board, describes: an 80286 with 640 KiB
//! conventional and 1 MiB extended memory. The video BIOS and the fixed-disk
//! ROM take the adapter-ROM placement their catalogue entries give ("Adapters
//! with System-Accessible ROM Modules").

use crate::create::{self, Chs, Ems, Hardware, NewImage, PackageRom, Sb16, Serial, Video};
use crate::firmware_import::{self, Statement};
use crate::{invalid, PcProvider, Result};
use image_store::{firmware::Validator, ConfigValidator, Descriptor, Digest, Manifest, Store};
use std::path::{Path, PathBuf};

/// The version tag of this template generation. Version tags never move, so
/// a changed template needs a new version here.
pub const VERSION: &str = "1.0.1";
/// The moving tag that follows the newest version.
pub const ALIAS: &str = "latest";
/// The registry namespace every template lives in.
pub const NAMESPACE: &str = "unemu";

/// One open firmware package of the templates: the catalogue entry and the
/// fixture files, relative to the vendor directory, that its fetch script
/// produces.
#[derive(Clone, Copy, Debug)]
pub struct FirmwareSource {
    pub catalogue_id: &'static str,
    pub rom: &'static str,
    pub source_archive: &'static str,
}

impl FirmwareSource {
    /// `unemu/firmware/<catalogue id>`.
    pub fn repository(&self) -> String {
        format!("{NAMESPACE}/firmware/{}", self.catalogue_id)
    }
}

/// The system BIOS first, then the video BIOS and the fixed-disk ROM, in the
/// order a template binds them.
pub const FIRMWARE: [FirmwareSource; 3] = [
    FirmwareSource {
        catalogue_id: "kiselev-xi8088-1.0.2-atreset",
        rom: "kiselev-bios/bios-xi8088-atreset.bin",
        source_archive: "kiselev-bios/kiselev-bios-a4af458-atreset-src.tar",
    },
    FirmwareSource {
        catalogue_id: "vgabios-0.8a-unemu.1-std",
        rom: "vgabios/vgabios-std.bin",
        source_archive: "vgabios/vgabios-0.8a-unemu.2-src.tar",
    },
    FirmwareSource {
        catalogue_id: "xub-r638-xtp-ata1f0",
        rom: "xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
        source_archive: "xtide-universal-bios/xtide-universal-bios-r638-src.tar",
    },
];

/// One template machine.
#[derive(Clone, Copy, Debug)]
pub struct Template {
    /// The repository name inside the official namespace.
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// Sound Blaster 16, a serial mouse on COM1 and a Lo-tech EMS board.
    pub games: bool,
}

impl Template {
    /// `unemu/<name>`.
    pub fn repository(&self) -> String {
        format!("{NAMESPACE}/{}", self.name)
    }

    /// The hardware: one 1.2M diskette drive with an empty catalogue and one
    /// blank 504 MiB disk (1024/16/63, the largest INT 13h CHS geometry) on
    /// the primary ATA channel, which XTIDE serves. The games template adds
    /// the shipped open game profile's cards: SB16 at 220h, IRQ 5, DMA 1 and
    /// 5; a Microsoft mouse on the COM1 UART; a 2 MiB Lo-tech EMS board at
    /// port 260h with its page frame at D000h.
    pub fn hardware(&self) -> Result<Hardware> {
        let board = assembly::BoardModel::Ibm5170;
        let speed = create::speeds(board)
            .into_iter()
            .find(|s| s.calibrated)
            .ok_or_else(|| invalid("the board has no calibrated speed"))?;
        Ok(Hardware {
            board,
            rate: (speed.steps, speed.pit_clocks),
            fpu: false,
            conventional_kib: 640,
            extended_kib: 1024,
            video: Video::Vga,
            sound: self.games.then_some(Sb16 {
                base: 0x220,
                irq: 5,
                dma8: 1,
                dma16: 5,
            }),
            serial: if self.games {
                Serial::Mouse
            } else {
                Serial::Empty
            },
            ems: self.games.then_some(Ems {
                port: 0x260,
                frame: 0xD000,
                size_kib: 2048,
            }),
            floppies: [Some("1.2M".into()), None],
            ide_port: 0x1F0,
            disks: [Some(Chs::new(1024, 16, 63)), None],
        })
    }
}

pub const TEMPLATES: [Template; 2] = [
    Template {
        name: "open-at",
        title: "Open AT 286",
        description: "An IBM 5170-class 80286 with open firmware only: the Kiselev AT BIOS, the unemu VGA BIOS and XTIDE Universal BIOS r638, each with its source archive. 640 KiB and 1 MiB of memory, VGA, one 1.2M diskette drive and a blank 504 MiB fixed disk. No operating system.",
        games: false,
    },
    Template {
        name: "open-at-games",
        title: "Open AT 286 for games",
        description: "Open AT 286 plus a Sound Blaster 16 (220h, IRQ 5, DMA 1 and 5), a Microsoft serial mouse on COM1 and a 2 MiB Lo-tech EMS board (port 260h, page frame D000h). No operating system.",
        games: true,
    },
];

/// What happened, or what a build would do, to one local tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The tag is new.
    Created,
    /// The tag already names this image.
    Present,
    /// The alias moves away from this digest, which only lower versions of
    /// the repository name.
    Moved(Digest),
    /// The alias names this other image, a higher version or one no version
    /// of the repository names, and stays.
    Kept(Digest),
    /// The version tag names this other image, so a build refuses; only
    /// [`plan_tags`] reports it.
    Conflict(Digest),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Firmware,
    Machine,
}

/// One built image and its local tags.
#[derive(Clone, Debug)]
pub struct Built {
    pub kind: Kind,
    /// The registry repository, such as `unemu/open-at`.
    pub repository: String,
    pub descriptor: Descriptor,
    /// The version tag, then the alias. Local tags are the short form a pull
    /// of the repository writes, so `image push TAG` reaches the repository.
    pub tags: Vec<(String, Outcome)>,
}

/// The local tag a pull of `repository:tag` writes: without the official
/// namespace when one name remains.
pub fn local_tag(repository: &str, tag: &str) -> String {
    let name = match repository.strip_prefix(&format!("{NAMESPACE}/")) {
        Some(rest) if !rest.contains('/') => rest,
        _ => repository,
    };
    format!("{name}:{tag}")
}

/// A version tag's precedence, Semantic Versioning 2.0.0 §11: the numeric
/// core, then a release above its pre-releases, whose dot-separated
/// identifiers compare numerically when numeric and as text otherwise,
/// numeric ones first and a shorter list first. Build metadata is ignored.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Precedence {
    core: [u64; 3],
    release: bool,
    pre: Vec<Identifier>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Identifier {
    Numeric(u64),
    Text(String),
}

impl Precedence {
    /// A release, not a pre-release.
    pub fn is_release(&self) -> bool {
        self.release
    }
}

/// The precedence of a version-shaped tag; `None` for any other tag.
pub fn precedence(tag: &str) -> Option<Precedence> {
    if !Store::is_version_tag(tag) {
        return None;
    }
    let tag = tag.split('+').next()?;
    let (core, pre) = match tag.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (tag, None),
    };
    let mut numbers = core.split('.').map(|n| n.parse::<u64>().ok());
    let core = [numbers.next()??, numbers.next()??, numbers.next()??];
    let pre = pre
        .map(|pre| {
            pre.split('.')
                .map(|id| match id.parse::<u64>() {
                    Ok(n) if id.bytes().all(|b| b.is_ascii_digit()) => Identifier::Numeric(n),
                    _ => Identifier::Text(id.to_owned()),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Precedence {
        core,
        release: tag.split_once('-').is_none(),
        pre,
    })
}

/// Asked with `repository:VERSION` and each image's manifest before the image
/// is tagged; an error refuses the build.
pub type Guard<'a> = &'a dyn Fn(&str, &Manifest) -> std::result::Result<(), String>;

/// Build every firmware package and template machine from the open fixtures
/// under `vendor` into `store`, in a fixed order, handing each image to
/// `built` once its tags are written. A tag that already names the same image
/// is kept; a version tag naming another image refuses the build. The alias
/// moves by compare-and-swap, and only away from an image that lower versions
/// of the repository name alone: never from a higher version, and never from
/// an image the build did not version.
pub fn build(
    store: &Store,
    vendor: &Path,
    guard: Guard<'_>,
    built: &mut dyn FnMut(&Built),
) -> Result<Vec<Built>> {
    let catalogue = crate::firmware::catalogue();
    let mut images = Vec::new();
    let mut packages = Vec::new();
    for source in &FIRMWARE {
        let package = build_package(store, vendor, source, catalogue, guard)?;
        packages.push(package.descriptor.digest.clone());
        built(&package);
        images.push(package);
    }
    let stored = create::packages(store)?;
    let rom = |digest: &Digest, id: &str| -> Result<PackageRom> {
        let package = stored
            .iter()
            .find(|p| &p.descriptor.digest == digest)
            .ok_or_else(|| invalid(format!("the {id} package is not readable")))?;
        let rom = package
            .config
            .roms
            .iter()
            .find(|r| r.catalogue_id.as_deref() == Some(id))
            .ok_or_else(|| invalid(format!("the {id} package lacks its ROM")))?;
        Ok(PackageRom::new(package, rom))
    };
    for template in &TEMPLATES {
        let image = NewImage {
            hardware: template.hardware()?,
            firmware: FIRMWARE
                .iter()
                .zip(&packages)
                .map(|(source, digest)| rom(digest, source.catalogue_id))
                .collect::<Result<_>>()?,
            title: Some(template.title.into()),
            description: Some(template.description.into()),
        };
        let machine = build_machine(store, template, &image, guard)?;
        built(&machine);
        images.push(machine);
    }
    Ok(images)
}

fn build_package(
    store: &Store,
    vendor: &Path,
    source: &FirmwareSource,
    catalogue: &[assembly::FirmwareEntry],
    guard: Guard<'_>,
) -> Result<Built> {
    let entry = catalogue
        .iter()
        .find(|e| e.id == source.catalogue_id)
        .ok_or_else(|| invalid(format!("{} is not in the catalogue", source.catalogue_id)))?;
    let assembly::FirmwareDistribution::Open {
        fetch,
        source_archive_sha256: Some(pin),
        ..
    } = entry.distribution
    else {
        return Err(invalid(format!(
            "{} is not open firmware with a pinned source archive",
            entry.id
        )));
    };
    let fixture = |relative: &str| -> Result<PathBuf> {
        let path = vendor.join(relative);
        if path.is_file() {
            Ok(path)
        } else {
            Err(invalid(format!(
                "missing fixture {}; run {fetch}",
                path.display()
            )))
        }
    };
    let rom = fixture(source.rom)?;
    let archive = fixture(source.source_archive)?;
    // Another archive would keep the copyleft ROM user-supplied: only the one
    // the fetch script builds is open without a licence statement.
    if Digest::of(&std::fs::read(&archive)?).hex() != pin {
        return Err(invalid(format!(
            "{} is not the source archive the catalogue pins for {}; run {fetch}",
            archive.display(),
            entry.id
        )));
    }
    let scan = firmware_import::scan(std::slice::from_ref(&rom), catalogue)?;
    let candidate = scan
        .candidates
        .into_iter()
        .find(|c| c.catalogue_id == entry.id)
        .ok_or_else(|| {
            invalid(format!(
                "{} is not the {} build; run {fetch}",
                rom.display(),
                entry.id
            ))
        })?;
    let statement = Statement {
        source: Some(archive),
        ..Statement::default()
    };
    let repository = source.repository();
    let preview = firmware_import::preview_manifest_with(&candidate, &[], &statement, catalogue)?;
    check(guard, &repository, &preview.manifest)?;
    let judged = Digest::of(&preview.manifest.encode()?);
    let version = local_tag(&repository, VERSION);
    let before = version_state(store, &version, &judged)?;
    let descriptor = firmware_import::publish_as(
        store,
        &candidate,
        &[],
        &statement,
        catalogue,
        &version,
        Some(&judged),
    )?;
    let rules = crate::firmware::PcFirmwareRules(catalogue);
    let alias = alias(store, &repository, &descriptor, &Validator(&rules))?;
    Ok(Built {
        kind: Kind::Firmware,
        repository,
        tags: vec![(version, before), alias],
        descriptor,
    })
}

fn build_machine(
    store: &Store,
    template: &Template,
    image: &NewImage,
    guard: Guard<'_>,
) -> Result<Built> {
    let repository = template.repository();
    let manifest = create::preview(image, store)?;
    check(guard, &repository, &manifest)?;
    let judged = Digest::of(&manifest.encode_with_types(PcProvider.payload_types())?);
    let version = local_tag(&repository, VERSION);
    let before = version_state(store, &version, &judged)?;
    let descriptor = match before {
        Outcome::Present => store.tag_descriptor(&version)?,
        _ => create::create_judged(store, image, &version, None, &judged)?,
    };
    let alias = alias(store, &repository, &descriptor, &PcProvider)?;
    Ok(Built {
        kind: Kind::Machine,
        repository,
        tags: vec![(version, before), alias],
        descriptor,
    })
}

fn check(guard: Guard<'_>, repository: &str, manifest: &Manifest) -> Result<()> {
    guard(&format!("{repository}:{VERSION}"), manifest)
        .map_err(|reason| invalid(format!("{repository}:{VERSION} refused: {reason}")))
}

/// Created when absent, Present when it names `digest`; another image under
/// the version tag refuses the build.
fn version_state(store: &Store, tag: &str, digest: &Digest) -> Result<Outcome> {
    match planned_version(store, tag, digest)? {
        Outcome::Conflict(current) => Err(invalid(format!(
            "{tag} already names {current}, not {digest}; a version tag never moves, so give the changed template a new version"
        ))),
        outcome => Ok(outcome),
    }
}

fn current(store: &Store, tag: &str) -> Result<Option<Digest>> {
    match store.tag_descriptor(tag) {
        Ok(current) => Ok(Some(current.digest)),
        Err(image_store::Error::TagNotFound(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn planned_version(store: &Store, tag: &str, digest: &Digest) -> Result<Outcome> {
    Ok(match current(store, tag)? {
        None => Outcome::Created,
        Some(current) if &current == digest => Outcome::Present,
        Some(current) => Outcome::Conflict(current),
    })
}

/// What the alias of `repository` does for an image with `digest`.
fn planned_alias(store: &Store, repository: &str, digest: &Digest) -> Result<(String, Outcome)> {
    let tag = local_tag(repository, ALIAS);
    let Some(current) = current(store, &tag)? else {
        return Ok((tag, Outcome::Created));
    };
    if &current == digest {
        return Ok((tag, Outcome::Present));
    }
    let ours = precedence(VERSION).ok_or_else(|| invalid("VERSION is not a version"))?;
    let prefix = local_tag(repository, "");
    let (mut lower, mut other) = (false, false);
    let mut after: Option<String> = None;
    loop {
        let page = store.list_tags(after.as_deref(), 1000)?;
        for named in page.tags.iter().filter(|t| t.manifest.digest == current) {
            match named.name.strip_prefix(&prefix).and_then(precedence) {
                Some(version) if version < ours => lower = true,
                Some(_) => other = true,
                None => {}
            }
        }
        match page.next_after {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    Ok(if lower && !other {
        (tag, Outcome::Moved(current))
    } else {
        (tag, Outcome::Kept(current))
    })
}

/// Point the alias at `descriptor` when [`planned_alias`] allows it, moving
/// it only from the digest read there.
fn alias(
    store: &Store,
    repository: &str,
    descriptor: &Descriptor,
    validator: &dyn ConfigValidator,
) -> Result<(String, Outcome)> {
    let (tag, outcome) = planned_alias(store, repository, &descriptor.digest)?;
    match &outcome {
        Outcome::Created => store.update_tag(&tag, descriptor, None, validator)?,
        Outcome::Moved(old) => store.update_tag(&tag, descriptor, Some(old), validator)?,
        _ => {}
    }
    Ok((tag, outcome))
}

/// What a build of `built` would do to the tags of `store`, which is only
/// read: a dry run's report against the store it would write.
pub fn plan_tags(store: &Store, built: &Built) -> Result<Vec<(String, Outcome)>> {
    let digest = &built.descriptor.digest;
    let version = local_tag(&built.repository, VERSION);
    let state = planned_version(store, &version, digest)?;
    Ok(vec![
        (version, state),
        planned_alias(store, &built.repository, digest)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_registry_grammar_and_the_pull_short_form() {
        assert_eq!(local_tag("unemu/open-at", VERSION), "open-at:1.0.1");
        assert_eq!(
            local_tag("unemu/firmware/xub-r638-xtp-ata1f0", ALIAS),
            "unemu/firmware/xub-r638-xtp-ata1f0:latest"
        );
        for source in &FIRMWARE {
            image_store::Store::validate_tag_name(&local_tag(&source.repository(), VERSION))
                .unwrap();
        }
        for template in &TEMPLATES {
            image_store::Store::validate_tag_name(&local_tag(&template.repository(), VERSION))
                .unwrap();
        }
        assert!(image_store::Store::is_version_tag(VERSION));
        assert!(!image_store::Store::is_version_tag(ALIAS));
    }

    #[test]
    fn versions_order_by_semantic_version_precedence() {
        let order = [
            "0.9.0",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.10.0",
            "2.0.0",
        ];
        for pair in order.windows(2) {
            assert!(
                precedence(pair[0]).unwrap() < precedence(pair[1]).unwrap(),
                "{pair:?}"
            );
        }
        assert_eq!(precedence("1.0.0+b1"), precedence("1.0.0"));
        assert_eq!(precedence(ALIAS), None);
    }

    #[test]
    fn the_games_template_adds_the_open_game_profile_cards_only() {
        let plain = TEMPLATES[0].hardware().unwrap();
        let games = TEMPLATES[1].hardware().unwrap();
        assert_eq!(plain.sound, None);
        assert_eq!(plain.ems, None);
        assert_eq!(plain.serial, Serial::Empty);
        assert_eq!(games.serial, Serial::Mouse);
        assert_eq!(
            Hardware {
                sound: None,
                serial: Serial::Empty,
                ems: None,
                ..games.clone()
            },
            plain
        );
        assert_eq!(plain.rate, (8, 13));
        assert_eq!(plain.disks, [Some(Chs::new(1024, 16, 63)), None]);
    }
}
