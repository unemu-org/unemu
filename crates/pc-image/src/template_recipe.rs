// SPDX-License-Identifier: AGPL-3.0-or-later
//! Offline copies of the published template inputs, built from the shipped
//! locked recipes (recipes §19.2; machine-images §5.5).

use super::{check, version_state, Guard, PACKAGE_CONFLICT};
use crate::{invalid, recipe::PcRecipes, Result};
use image_store::{ConfigValidator, Descriptor, Store};
use recipe::build::{Published, Publisher, Staged};
use recipe::provider::{FirmwareProfile, MediaProfile, Provider};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static PC: PcRecipes = PcRecipes;

struct Bundle {
    reference: &'static str,
    recipe: &'static str,
    files: &'static [(&'static str, &'static [u8])],
}

// Embed the locks too: installed builds must reproduce the published writer's
// bytes without needing the source checkout or regenerating its metadata.
macro_rules! bundle {
    ($reference:literal, $folder:literal, $recipe:literal, [$($lock:literal),+], [$($work:literal),*]) => {
        Bundle {
            reference: $reference,
            recipe: $recipe,
            files: &[
                ($recipe, include_bytes!(concat!("../data/recipes/", $folder, "/", $recipe))),
                $((concat!($recipe, ".lock/sha256/", $lock), include_bytes!(concat!("../data/recipes/", $folder, "/", $recipe, ".lock/sha256/", $lock)))),+,
                $(($work, include_bytes!(concat!("../data/recipes/", $folder, "/", $work)))),*
            ],
        }
    };
}

const BUNDLES: &[Bundle] = &[
    bundle!(
        "unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1",
        "unemu-at-bios",
        "unemu-at-bios-1.0.2-unemu.3.firmware.toml",
        [
            "0db9090a14cc29cd4e83f467d0dc7d38c77f953f0e910cb802cdd613ae041622",
            "bb28befa3d0838ca6606babf16e665d264672a7a8e3cedf669533dd031c2d441",
            "c4c53a6a5c0727d8cccb26b22a92dcbf68e906aed7d9424be80763ea30533ff2"
        ],
        []
    ),
    bundle!(
        "unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std:r1",
        "vgabios",
        "vgabios-0.9d-unemu.1-std.firmware.toml",
        [
            "91905d8f6a628d3a738729e4f6ea3a6016671e7ced88f308e7a96cd37bb492f9",
            "c1972247ba5fc03dee5bc011b137e49c547ef1f4fc32dc3f74fa09b205c235e7",
            "f701c379b5abab219ae098c8b4e86cc8199959487640294114657af47181bf36"
        ],
        []
    ),
    bundle!(
        "unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1",
        "xtide-universal-bios",
        "xub-r638-xtp-ata1f0.firmware.toml",
        [
            "3446552c5291c89f1e1506e3ec37212324732aabe14e06cc68a86157363d27ed",
            "4dc25f1c00342e44b2476e84befc9dbbb1817d478cdda9b1fd0a5d8e026026a1",
            "8185e0b187181a694d0928f9a5a48181bf1159cd8f607c9d95c7783b3cb68e09"
        ],
        []
    ),
    bundle!(
        "unemu/media/ibm-pc/freedos:1.4-r1",
        "freedos-1.4-floppy",
        "freedos-1.4.media.toml",
        [
            "2282671de117a3500e5d40b28ffb5ec2079b7f80b86de9ecf61ed594aea90a5b",
            "77060710c8e05136896a9e84ebee82035b1212114f9e288630dfabe730a5b754",
            "a38f2ebcf49c1bda04519684b0b5224c9a865b782b668ff79a5f31cb25f7c528"
        ],
        ["freedos.work.toml"]
    ),
    bundle!(
        "unemu/media/ibm-pc/freedos:1.4-1.2m-r1",
        "freedos-1.4-floppy",
        "freedos-1.4-1.2m.media.toml",
        [
            "4f0de9632d81bd28448a277c428f52103d757a9f724f7a63c702867551964a2a",
            "6508d997b31b3cd673a732cf18591c387dbd3c7e44f5c90f83463af038f1c044",
            "b0bc9036917bd3a1328fd3922605ca108c104bdad7ba7013c5b32570c41a3565"
        ],
        ["freedos.work.toml"]
    ),
    bundle!(
        "unemu/media/ibm-pc/freedos:1.4-720k-r1",
        "freedos-1.4-floppy",
        "freedos-1.4-720k.media.toml",
        [
            "1ddda09178071c67bb7ab6b8872a61afae0dcf22fb74000346acf01973e03905",
            "4811ed943d26b38a03529c554988f2c3d0638fa106ecf3e3d513aff1c2aeb989",
            "d8c593a829fedf14c118c099efa07883c200191f5acc0103d3dc475cc804186e"
        ],
        ["freedos.work.toml"]
    ),
];

struct Folder(PathBuf);
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Guarded<'a> {
    guard: Guard<'a>,
    validator: &'a dyn ConfigValidator,
}

impl Provider for Guarded<'_> {
    fn platform(&self) -> &str {
        PC.platform()
    }
    fn media(&self) -> &dyn MediaProfile {
        PC.media()
    }
    fn firmware(&self) -> &dyn FirmwareProfile {
        PC.firmware()
    }
    fn needs(&self) -> Option<&dyn image_store::needs::Vocabulary> {
        PC.needs()
    }
    fn software_platforms(&self) -> &[&str] {
        PC.software_platforms()
    }
    fn work_id_catalogues(&self) -> &[&str] {
        PC.work_id_catalogues()
    }
    fn publisher(&self) -> Option<&dyn Publisher> {
        Some(self)
    }
}

impl Publisher for Guarded<'_> {
    fn publish(&self, _: &Store, _: &Staged) -> std::result::Result<Published, String> {
        Err("template inputs must use pinned recipes".into())
    }

    fn tag_locked(
        &self,
        store: &Store,
        reference: &str,
        descriptor: &Descriptor,
    ) -> std::result::Result<Published, String> {
        let image = store
            .resolve_descriptor(descriptor, self.validator)
            .map_err(|e| e.to_string())?;
        let (repository, tag) = reference
            .rsplit_once(':')
            .ok_or("recipe reference lacks a tag")?;
        check(self.guard, repository, tag, &image.manifest, &image.config)
            .map_err(|e| e.to_string())?;
        version_state(store, reference, &descriptor.digest, PACKAGE_CONFLICT)
            .map_err(|e| e.to_string())?;
        recipe::lock::tag(store, reference, descriptor, self.validator)
    }
}

pub(super) fn build(
    store: &Store,
    vendor: &Path,
    reference: &str,
    validator: &dyn ConfigValidator,
    guard: Guard<'_>,
) -> Result<Descriptor> {
    let bundle = BUNDLES
        .iter()
        .find(|b| b.reference == reference)
        .ok_or_else(|| invalid(format!("no template recipe for {reference}")))?;
    let root = store.root().join("cache/template-recipes");
    std::fs::create_dir_all(&root)?;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let folder = loop {
        let path = root.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => break Folder(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    };
    for (name, bytes) in bundle.files {
        let path = folder.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }
    let provider = Guarded { guard, validator };
    let providers: [&dyn Provider; 1] = [&provider];
    let mut libraries = [recipe::library::Library::scan(vendor, None)?];
    let outcome = recipe::build::build_path(
        recipe::build::Request {
            path: &folder.0.join(bundle.recipe),
            providers: recipe::Providers(&providers),
            store,
            index: None,
            store_read: None,
            libraries: &mut libraries,
            fetcher: None,
            keep_as: None,
            find_outputs: true,
        },
        &mut |_| {},
    )?;
    match outcome {
        recipe::build::Outcome::Built(report) => Ok(report.published.descriptor),
        recipe::build::Outcome::Failed { error, .. } => Err(invalid(error)),
        other => Err(invalid(format!("{reference}: {other:#?}"))),
    }
}
