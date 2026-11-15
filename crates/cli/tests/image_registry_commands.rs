// SPDX-License-Identifier: AGPL-3.0-or-later
//! Registry push, pull and listing commands against the in-process mock
//! registry, for machine images and firmware packages; machine-images §§5.1
//! and 5.3. Commands that talk to the registry run the binary as a child
//! process so each gets its own environment: synthetic credentials, and none
//! of the caller's registry settings.
#[path = "support/fixtures.rs"]
mod fixtures;

use image_registry::test_support::{MockRegistry, Policy};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

struct Dir(PathBuf);
impl Dir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "image-registry-command-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn os(args: &[&dyn AsRef<std::ffi::OsStr>]) -> Vec<OsString> {
    args.iter().map(|a| a.as_ref().to_owned()).collect()
}

fn run(args: &[OsString]) -> (u8, String, String) {
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli::command::run(args, &mut output, &mut errors);
    (
        code,
        String::from_utf8(output).unwrap(),
        String::from_utf8(errors).unwrap(),
    )
}

/// `credentials` is the environment pair with the host it is bound to, or
/// `None` for the pair's default host.
fn child(
    args: &[OsString],
    home: &Path,
    credentials: Option<(Option<&str>, &str, &str)>,
) -> (i32, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_unemupc98"));
    command
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env_remove("UNEMU_REGISTRY")
        .env_remove("UNEMU_REGISTRY_HOST")
        .env_remove("UNEMU_REGISTRY_USER")
        .env_remove("UNEMU_REGISTRY_PASSWORD");
    if let Some((host, user, password)) = credentials {
        command
            .env("UNEMU_REGISTRY_USER", user)
            .env("UNEMU_REGISTRY_PASSWORD", password);
        if let Some(host) = host {
            command.env("UNEMU_REGISTRY_HOST", host);
        }
    }
    let output = command.output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// The FreeDOS reference machine; import marks every blob from the user's
/// files user-supplied and its generated configuration and metadata open.
fn import(store: &Path, tag: &str) -> String {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml");
    let (code, output, errors) = run(&os(&[
        &"image",
        &"import",
        &source,
        &"--store",
        &store,
        &"--tag",
        &tag,
        &"--create",
    ]));
    assert_eq!(code, 0, "{errors}");
    output
        .split('\t')
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned()
}

fn config(dir: &Dir, registry: &MockRegistry, private: bool) -> PathBuf {
    let path = dir.join("preferences.toml");
    std::fs::write(
        &path,
        format!(
            "rom_dirs = []\n[[registries]]\nhost = \"{}\"\nprivate = {private}\nplain_http = true\n",
            registry.host()
        ),
    )
    .unwrap();
    path
}

#[test]
fn registry_commands_validate_their_arguments() {
    for args in [
        vec!["image", "push"],
        vec!["image", "push", "lab:1.0.0"],
        vec!["image", "push", "lab:1.0.0", "a", "b", "--store", "s"],
        vec!["image", "push", "lab:1.0.0", "--store", "s", "--tag", "x:1"],
        vec![
            "image",
            "push",
            "lab:1.0.0",
            "--store",
            "s",
            "--move",
            "--move",
        ],
        vec!["image", "pull", "--store", "s"],
        vec!["image", "pull", "a", "b", "--store", "s"],
        vec!["image", "pull", "a", "--store", "s", "--move"],
        vec!["image", "remote", "list", "lab"],
        vec!["image", "remote", "ls"],
        vec!["image", "login", "registry.unemu.dev"],
        vec![
            "image",
            "login",
            "registry.unemu.dev",
            "--user",
            "me",
            "--store",
            "s",
        ],
    ] {
        let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
        let (code, output, errors) = run(&args);
        assert_eq!(code, 2, "{args:?}");
        assert!(output.is_empty());
        assert!(errors.contains("usage:"), "{args:?}");
    }
}

#[test]
fn dry_run_lists_redistribution_marks_and_refuses_the_official_namespace_offline() {
    let dir = Dir::new("dry-run");
    let store = dir.join("store");
    let digest = import(&store, "lab:1.0.0");
    let preferences = dir.join("empty.toml");
    std::fs::write(&preferences, "").unwrap();
    let (code, output, errors) = run(&os(&[
        &"image",
        &"push",
        &"lab:1.0.0",
        &"--store",
        &store,
        &"--registry",
        &"registry.unemu.dev",
        &"--config",
        &preferences,
        &"--dry-run",
    ]));
    assert_eq!(code, 1, "{output}{errors}");
    assert!(
        output.starts_with("target:registry.unemu.dev/unemu/lab:1.0.0\n"),
        "{output}"
    );
    assert!(output.contains(&format!("image:{digest}\n")));
    let closure: Vec<_> = output
        .lines()
        .filter(|l| l.starts_with("config\t") || l.starts_with("layer\t"))
        .collect();
    assert!(closure.len() >= 3, "{output}");
    // The generated configuration and metadata are open; the user's ROMs and
    // disks stay user-supplied, so the official namespace still refuses.
    for line in &closure {
        let generated = line.starts_with("config\t")
            || line.contains("\tapplication/vnd.unemu.metadata.v1+json\t");
        let mark = if generated { "open" } else { "user-supplied" };
        assert!(line.ends_with(&format!("\t{mark}\tthis image")), "{output}");
    }
    assert!(output.ends_with("guard:refused\n"));
    assert!(errors.contains("official namespace"), "{errors}");
}

#[test]
fn push_refusals_happen_before_any_request() {
    let dir = Dir::new("refusals");
    let store = dir.join("store");
    import(&store, "lab:1.0.0");
    let registry = MockRegistry::start(Policy {
        anonymous_write: vec!["*".into()],
        ..Policy::default()
    });
    let public = config(&dir, &registry, false);
    let host = registry.host();
    let push = |target: String, preferences: &Path| {
        run(&os(&[
            &"image",
            &"push",
            &"lab:1.0.0",
            &target,
            &"--store",
            &store,
            &"--config",
            &preferences,
        ]))
    };
    let (code, _, errors) = push(format!("{host}/private/lab:1.0.0"), &public);
    assert_eq!(code, 1);
    assert!(errors.contains("not marked private"), "{errors}");
    let private = config(&dir, &registry, true);
    let (code, _, errors) = push(format!("{host}/unemu/lab:1.0.0"), &private);
    assert_eq!(code, 1);
    assert!(errors.contains("official namespace"), "{errors}");
    let unlisted = dir.join("unlisted.toml");
    std::fs::write(&unlisted, "").unwrap();
    let (code, _, errors) = push(format!("{host}/private/lab:1.0.0"), &unlisted);
    assert_eq!(code, 1);
    assert!(errors.contains("not a configured registry"), "{errors}");
    assert!(registry.log().is_empty(), "{:?}", registry.log());
}

#[test]
fn push_pull_and_list_through_a_private_registry() {
    let dir = Dir::new("round-trip");
    let source = dir.join("source");
    let digest = import(&source, "lab:1.0.0");
    let registry = MockRegistry::start(Policy {
        user: Some(("ci".into(), "synthetic-secret".into())),
        ..Policy::default()
    });
    let preferences = config(&dir, &registry, true);
    let host = registry.host();
    let reference = format!("{host}/private/lab:1.0.0");
    let credentials = Some((Some(host.as_str()), "ci", "synthetic-secret"));
    let push = os(&[
        &"image",
        &"push",
        &"lab:1.0.0",
        &reference,
        &"--store",
        &source,
        &"--config",
        &preferences,
    ]);
    let (code, output, errors) = child(&push, &dir.0, None);
    assert_ne!(code, 0, "{output}");
    assert!(errors.contains("requires authentication"), "{errors}");
    // Without UNEMU_REGISTRY_HOST the pair belongs to the default registry only.
    let unbound = Some((None, "ci", "synthetic-secret"));
    let (code, output, errors) = child(&push, &dir.0, unbound);
    assert_ne!(code, 0, "{output}");
    assert!(errors.contains("requires authentication"), "{errors}");
    assert!(
        registry.authorized_log().is_empty(),
        "{:?}",
        registry.authorized_log()
    );
    let (code, output, errors) = child(&push, &dir.0, credentials);
    assert_eq!(code, 0, "{errors}");
    assert!(
        output.ends_with(&format!("pushed\t{reference}\t{digest}\n")),
        "{output}"
    );
    assert!(
        output
            .lines()
            .filter(|l| l.starts_with("uploaded\t"))
            .count()
            >= 3
    );
    let (code, output, errors) = child(&push, &dir.0, credentials);
    assert_eq!(code, 0, "{errors}");
    assert_eq!(output, format!("unchanged\t{reference}\t{digest}\n"));

    let target = dir.join("target");
    let pull = os(&[
        &"image",
        &"pull",
        &reference,
        &"--store",
        &target,
        &"--create",
        &"--config",
        &preferences,
    ]);
    let (code, output, errors) = child(&pull, &dir.0, credentials);
    assert_eq!(code, 0, "{errors}");
    assert!(
        output.ends_with(&format!("pulled\tprivate/lab:1.0.0\t{digest}\n")),
        "{output}"
    );
    let (code, listed, errors) = run(&os(&[&"image", &"ls", &"--store", &target]));
    assert_eq!(code, 0, "{errors}");
    // The pulled image arrived as compressed frames and nothing was decoded.
    let (row, cache) = listed.split_once('\n').unwrap();
    assert_eq!(cache, "materialized:0\n");
    let fields: Vec<_> = row.split('\t').collect();
    assert_eq!(
        fields[..3],
        ["private/lab:1.0.0", digest.as_str(), "machine"]
    );
    let size =
        |field: &str, key: &str| -> u64 { field.strip_prefix(key).unwrap().parse().unwrap() };
    assert!(
        size(fields[3], "stored:") < size(fields[4], "decoded:"),
        "{row}"
    );

    let (code, output, errors) = child(
        &os(&[
            &"image",
            &"remote",
            &"ls",
            &format!("{host}/private/lab"),
            &"--config",
            &preferences,
        ]),
        &dir.0,
        credentials,
    );
    assert_eq!(code, 0, "{errors}");
    assert_eq!(output, format!("{reference}\n"));
    let (code, output, errors) = child(
        &os(&[&"image", &"remote", &"ls", &host, &"--config", &preferences]),
        &dir.0,
        credentials,
    );
    assert_eq!(code, 0, "{errors}");
    assert_eq!(output, format!("{host}/private/lab\n"));
}

/// The open Xi8088 BIOS as a firmware package with the source archive its
/// fetch script builds, which the catalogue pins. Its ROM and archive layers
/// are open under the catalogue licence, and, because every other layer is
/// open, so are its README and config (machine-images §5.3). Returns the tag
/// and package digest, or `None` when the BIOS fixtures are not fetched.
fn import_open_firmware(dir: &Dir, store: &Path) -> Option<(String, String)> {
    let bytes = fixtures::fixture("kiselev-bios/bios-xi8088.bin")?;
    let archive = fixtures::fixture("kiselev-bios/kiselev-bios-a4af458-atreset-src.tar")?;
    let folder = dir.join("roms");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("bios.bin"), bytes).unwrap();
    let source = dir.join("kiselev-bios-a4af458-atreset-src.tar");
    std::fs::write(&source, archive).unwrap();
    let (code, output, errors) = run(&os(&[
        &"image",
        &"firmware",
        &"import",
        &folder,
        &"--store",
        &store,
        &"--create",
        &"--for",
        &assembly::KISELEV_XI8088.id,
        &"--source",
        &source,
    ]));
    assert_eq!(code, 0, "{errors}");
    let tag = format!("firmware/{}:1", assembly::KISELEV_XI8088.id);
    let prefix = format!("imported\t{tag}\t");
    let digest = output
        .lines()
        .find_map(|l| l.strip_prefix(prefix.as_str()))
        .unwrap_or_else(|| panic!("{output}"))
        .to_owned();
    Some((tag, digest))
}

/// A synthetic ROM published as a firmware package from a one-entry
/// catalogue that lists it as user-supplied, as a proprietary BIOS is, so
/// every descriptor carries the user-supplied mark. Returns the tag and the
/// ROM digest.
fn import_user_supplied_firmware(dir: &Dir, store: &Path) -> (String, String) {
    let bytes = vec![0x5a; 32768];
    let rom = dir.join("user-supplied.bin");
    std::fs::write(&rom, &bytes).unwrap();
    let digest = image_store::Digest::of(&bytes);
    let catalogue = [assembly::FirmwareEntry {
        id: "synthetic-user-supplied-bios",
        sha256: Box::leak(digest.hex().to_owned().into_boxed_str()),
        size: 32768,
        distribution: assembly::FirmwareDistribution::UserSupplied,
        ..assembly::KISELEV_XI8088
    }];
    let store = image_store::Store::open(store).unwrap();
    let scan = pc_image::firmware_import::scan(&[rom], &catalogue).unwrap();
    let [candidate] = scan.candidates.as_slice() else {
        panic!("{:?}", scan.issues)
    };
    pc_image::firmware_import::publish(&store, candidate, &[], &catalogue).unwrap();
    (candidate.tag(), digest.to_string())
}

/// The manifest digest of a local tag and the digests of its config and layers.
fn closure(store: &Path, tag: &str) -> (String, BTreeSet<String>) {
    use image_store::BlobSource;
    let store = image_store::Store::open_existing(store).unwrap();
    let image = store.tag_descriptor(tag).unwrap();
    let bytes = store
        .read_verified(&image, image_store::MAX_SMALL_BLOB)
        .unwrap();
    let manifest = image_store::Manifest::decode_for_summary(&bytes).unwrap();
    let blobs = std::iter::once(&manifest.config)
        .chain(&manifest.layers)
        .map(|d| d.digest.to_string())
        .collect();
    (image.digest.to_string(), blobs)
}

/// The stored ROM layer of a local firmware package, found by the decoded
/// SHA-256 its config binds, whether the layer is raw or `+zstd`.
fn rom_layer(store: &Path, tag: &str, rom: &str) -> image_store::Descriptor {
    use image_store::BlobSource;
    let store = image_store::Store::open_existing(store).unwrap();
    let image = store.tag_descriptor(tag).unwrap();
    let bytes = store
        .read_verified(&image, image_store::MAX_SMALL_BLOB)
        .unwrap();
    let manifest = image_store::Manifest::decode_for_summary(&bytes).unwrap();
    let rom: image_store::Digest = rom.parse().unwrap();
    let layer = manifest
        .layers
        .into_iter()
        .find(|d| d.names(&rom))
        .unwrap_or_else(|| panic!("no layer holds {rom}"));
    assert_eq!(
        layer.decoded_media_type(),
        image_store::media_type::FIRMWARE
    );
    layer
}
/// The dry-run line of a stored layer: `layer digest size media-type `.
fn layer_line(layer: &image_store::Descriptor) -> String {
    format!(
        "layer\t{}\t{}\t{}\t",
        layer.digest, layer.size, layer.media_type
    )
}

/// Digests of the progress lines that start with `word`.
fn progress(output: &str, word: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|l| l.strip_prefix(word)?.strip_prefix('\t'))
        .map(str::to_owned)
        .collect()
}

/// An open firmware package pushed to a private mock registry that requires
/// credentials, under the same repository name as its local tag.
struct PushedFirmware {
    dir: Dir,
    source: PathBuf,
    registry: MockRegistry,
    host: String,
    preferences: PathBuf,
    tag: String,
    digest: String,
    reference: String,
    output: String,
}

impl PushedFirmware {
    fn start(label: &str) -> Option<Self> {
        let dir = Dir::new(label);
        let source = dir.join("source");
        let (tag, digest) = import_open_firmware(&dir, &source)?;
        let registry = MockRegistry::start(Policy {
            user: Some(("ci".into(), "synthetic-secret".into())),
            ..Policy::default()
        });
        let preferences = config(&dir, &registry, true);
        let host = registry.host();
        let reference = format!("{host}/{tag}");
        let mut pushed = Self {
            dir,
            source,
            registry,
            host,
            preferences,
            tag,
            digest,
            reference,
            output: String::new(),
        };
        let (code, output, errors) = child(
            &os(&[
                &"image",
                &"push",
                &pushed.tag,
                &pushed.reference,
                &"--store",
                &pushed.source,
                &"--config",
                &pushed.preferences,
            ]),
            &pushed.dir.0,
            pushed.credentials(),
        );
        assert_eq!(code, 0, "{output}{errors}");
        pushed.output = output;
        Some(pushed)
    }

    fn credentials(&self) -> Option<(Option<&str>, &str, &str)> {
        Some((Some(self.host.as_str()), "ci", "synthetic-secret"))
    }

    fn pull(&self, store: &Path, extra: &[&str]) -> (i32, String, String) {
        let mut args = os(&[
            &"image",
            &"pull",
            &self.reference,
            &"--store",
            &store,
            &"--create",
            &"--config",
            &self.preferences,
        ]);
        args.extend(extra.iter().map(OsString::from));
        child(&args, &self.dir.0, self.credentials())
    }
}

#[test]
fn an_open_firmware_package_round_trips_through_a_private_registry() {
    let Some(pushed) = PushedFirmware::start("firmware-round-trip") else {
        return;
    };
    let (tag, digest) = (&pushed.tag, &pushed.digest);
    let (manifest, blobs) = closure(&pushed.source, tag);
    assert_eq!(&manifest, digest);
    assert!(
        pushed
            .output
            .ends_with(&format!("pushed\t{}\t{digest}\n", pushed.reference)),
        "{}",
        pushed.output
    );
    assert_eq!(progress(&pushed.output, "uploaded"), blobs);

    let target = pushed.dir.join("target");
    let (code, output, errors) = pushed.pull(&target, &[]);
    assert_eq!(code, 0, "{errors}");
    assert!(
        output.ends_with(&format!("pulled\t{tag}\t{digest}\n")),
        "{output}"
    );
    assert_eq!(progress(&output, "fetched"), blobs);
    assert_eq!(closure(&target, tag), (digest.clone(), blobs));

    let (code, listed, errors) = run(&os(&[&"image", &"firmware", &"ls", &"--store", &target]));
    assert_eq!(code, 0, "{errors}");
    let row = listed
        .lines()
        .find(|l| l.starts_with(&format!("{tag}\t{digest}\t")))
        .unwrap_or_else(|| panic!("{listed}"));
    let catalogue_ids = format!("catalogue_ids: [\"{}\"]", assembly::KISELEV_XI8088.id);
    for facet in [
        "roles: [\"system-bios\"]",
        "boards: [\"ibm-5170\"]",
        "machine_classes: [\"at\"]",
        catalogue_ids.as_str(),
        assembly::KISELEV_XI8088.sha256,
    ] {
        assert!(row.contains(facet), "{facet} missing from {row}");
    }
    // A firmware package lists as firmware, not as a machine, so the library
    // offers it no Start, and running it as a machine is refused.
    let (code, listed, errors) = run(&os(&[&"image", &"ls", &"--store", &target]));
    assert_eq!(code, 0, "{errors}");
    let (row, cache) = listed.split_once('\n').unwrap();
    assert!(
        row.starts_with(&format!("{tag}\t{digest}\tfirmware\tstored:")),
        "{listed}"
    );
    assert_eq!(cache, "materialized:0\n");
    let (code, output, errors) = run(&os(&[
        &"image",
        &"run",
        tag,
        &"--store",
        &target,
        &"--workspace",
        &"try",
        &"--new",
    ]));
    assert_eq!(code, 1, "{output}");
    assert!(output.is_empty(), "{output}");
    assert!(errors.contains("artifact type does not match"), "{errors}");

    let (code, output, errors) = child(
        &os(&[
            &"image",
            &"remote",
            &"ls",
            &format!("{}/firmware/{}", pushed.host, assembly::KISELEV_XI8088.id),
            &"--config",
            &pushed.preferences,
        ]),
        &pushed.dir.0,
        pushed.credentials(),
    );
    assert_eq!(code, 0, "{errors}");
    assert_eq!(output, format!("{}\n", pushed.reference));
}

#[test]
fn pulling_a_firmware_package_the_store_already_holds_fetches_nothing() {
    let Some(pushed) = PushedFirmware::start("firmware-no-op") else {
        return;
    };
    let (tag, digest) = (&pushed.tag, &pushed.digest);
    let before = closure(&pushed.source, tag);
    pushed.registry.clear_log();
    let (code, output, errors) = pushed.pull(&pushed.source, &[]);
    assert_eq!(code, 0, "{errors}");
    assert_eq!(output, format!("unchanged\t{tag}\t{digest}\n"));
    assert_eq!(closure(&pushed.source, tag), before);

    // Under another local name the blobs are already present, so only the
    // new tag is written.
    let (code, output, errors) = pushed.pull(&pushed.source, &["--tag", "copy/xi8088:1"]);
    assert_eq!(code, 0, "{errors}");
    assert!(
        output.ends_with(&format!("pulled\tcopy/xi8088:1\t{digest}\n")),
        "{output}"
    );
    assert!(progress(&output, "fetched").is_empty(), "{output}");
    assert_eq!(progress(&output, "present"), before.1);
    let log = pushed.registry.log();
    assert!(
        log.iter()
            .all(|l| l.starts_with("GET ") || l.starts_with("HEAD ")),
        "{log:?}"
    );
    assert!(log.iter().all(|l| !l.contains("/blobs/")), "{log:?}");
}

#[test]
fn user_supplied_firmware_is_refused_by_public_registries_and_the_official_namespace() {
    let dir = Dir::new("firmware-refusals");
    let store = dir.join("store");
    let (tag, rom) = import_user_supplied_firmware(&dir, &store);
    let registry = MockRegistry::start(Policy {
        anonymous_write: vec!["*".into()],
        ..Policy::default()
    });
    let host = registry.host();
    let push = |target: String, preferences: &Path, dry_run: bool| {
        let mut args = os(&[
            &"image",
            &"push",
            &tag,
            &target,
            &"--store",
            &store,
            &"--config",
            &preferences,
        ]);
        if dry_run {
            args.push("--dry-run".into());
        }
        run(&args)
    };
    // Refusals and the dry run name the blob that would be uploaded: the
    // ROM's stored layer, raw or compressed, found by its decoded digest.
    let layer = rom_layer(&store, &tag, &rom);
    assert_eq!(layer.decoded_size().unwrap(), 32768);
    let stored = layer.digest.to_string();
    let public = config(&dir, &registry, false);
    let (code, _, errors) = push(format!("{host}/private/rom:1"), &public, false);
    assert_eq!(code, 1);
    assert!(errors.contains("not marked private"), "{errors}");
    assert!(errors.contains(&stored), "{errors}");
    let private = config(&dir, &registry, true);
    let (code, _, errors) = push(format!("{host}/unemu/rom:1"), &private, false);
    assert_eq!(code, 1);
    assert!(errors.contains("official namespace"), "{errors}");
    assert!(errors.contains(&stored), "{errors}");
    let (code, output, errors) = push(format!("{host}/private/rom:1"), &private, true);
    assert_eq!(code, 0, "{errors}");
    assert!(
        output.contains(&format!(
            "{}user-supplied\tthis image\n",
            layer_line(&layer)
        )),
        "{output}"
    );
    assert!(output.ends_with("guard:allowed\n"), "{output}");
    assert!(registry.log().is_empty(), "{:?}", registry.log());
}

#[test]
fn a_firmware_dry_run_shows_the_rom_layer_effective_class() {
    let dir = Dir::new("firmware-dry-run");
    let store = dir.join("store");
    let Some((tag, digest)) = import_open_firmware(&dir, &store) else {
        return;
    };
    let registry = MockRegistry::start(Policy::default());
    let host = registry.host();
    let rom = format!("sha256:{}", assembly::KISELEV_XI8088.sha256);
    let layer = rom_layer(&store, &tag, &rom);
    assert_eq!(layer.decoded_size().unwrap(), 32768);
    let dry_run = |preferences: &Path| {
        run(&os(&[
            &"image",
            &"push",
            &tag,
            &format!("{host}/{tag}"),
            &"--store",
            &store,
            &"--config",
            &preferences,
            &"--dry-run",
        ]))
    };
    let private = config(&dir, &registry, true);
    let (code, output, errors) = dry_run(&private);
    assert_eq!(code, 0, "{errors}");
    assert!(output.contains(&format!("image:{digest}\n")), "{output}");
    let rom_layer = layer_line(&layer);
    assert!(
        output.contains(&format!("{rom_layer}open\tthis image\n")),
        "{output}"
    );
    // Every layer is open, so the config is too.
    assert!(
        output.contains("\tapplication/vnd.unemu.firmware.config.v1+toml\topen\tthis image\n"),
        "{output}"
    );
    assert!(output.ends_with("guard:allowed\n"), "{output}");

    // Every layer is open and the ROM carries its source, so a public
    // registry outside unemu/ may take it too.
    let public = config(&dir, &registry, false);
    let (code, output, errors) = dry_run(&public);
    assert_eq!(code, 0, "{errors}");
    assert!(output.ends_with("guard:allowed\n"), "{output}");

    // A machine image that took the same BIOS from a plain file, before the
    // package existed, records it as user-supplied; the most restrictive
    // local mark decides. It holds the BIOS raw, so the package's compressed
    // copy inherits the mark through its decoded digest.
    let restricted = dir.join("restricted");
    let machine = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml");
    let (code, _, errors) = run(&os(&[
        &"image",
        &"import",
        &machine,
        &"--store",
        &restricted,
        &"--create",
        &"--tag",
        &"lab:1.0.0",
        &"--no-compress",
    ]));
    assert_eq!(code, 0, "{errors}");
    import_open_firmware(&dir, &restricted).unwrap();
    let dry_run = |store: &Path, preferences: &Path| {
        run(&os(&[
            &"image",
            &"push",
            &tag,
            &format!("{host}/{tag}"),
            &"--store",
            &store,
            &"--config",
            &preferences,
            &"--dry-run",
        ]))
    };
    let (code, output, errors) = dry_run(&restricted, &config(&dir, &registry, true));
    assert_eq!(code, 0, "{errors}");
    assert!(
        output.contains(&format!("{rom_layer}user-supplied\tlocal tag lab:1.0.0\n")),
        "{output}"
    );
    let (code, output, errors) = dry_run(&restricted, &config(&dir, &registry, false));
    assert_eq!(code, 1, "{output}");
    assert!(output.ends_with("guard:refused\n"), "{output}");
    assert!(errors.contains(&layer.digest.to_string()), "{errors}");
    assert!(registry.log().is_empty(), "{:?}", registry.log());
}

/// An unlisted synthetic option ROM imported as open GPL-2.0-only firmware
/// together with its source archive, large enough to be stored compressed.
/// Returns the tag.
fn import_copyleft_with_source(dir: &Dir, store: &Path) -> String {
    let rom = dir.join("copyleft.bin");
    std::fs::write(&rom, [0x3c; 8192]).unwrap();
    let mut archive = vec![0_u8; 20 * 1024];
    archive[257..263].copy_from_slice(b"ustar\0");
    let source = dir.join("copyleft-src.tar");
    std::fs::write(&source, archive).unwrap();
    let (code, output, errors) = run(&os(&[
        &"image",
        &"firmware",
        &"import",
        &rom,
        &"--unlisted",
        &"copyleft-rom",
        &"--role",
        &"option-rom",
        &"--open",
        &"--license",
        &"GPL-2.0-only",
        &"--source",
        &source,
        &"--source-license",
        &"GPL-2.0-only",
        &"--store",
        &store,
        &"--create",
    ]));
    assert_eq!(code, 0, "{errors}");
    assert!(
        output.contains("imported\tfirmware/copyleft-rom:1\t"),
        "{output}"
    );
    "firmware/copyleft-rom:1".into()
}

#[test]
fn a_copyleft_package_with_its_source_round_trips_and_may_enter_the_official_namespace() {
    let dir = Dir::new("firmware-source");
    let store = dir.join("store");
    let tag = import_copyleft_with_source(&dir, &store);
    let registry = MockRegistry::start(Policy {
        anonymous_read: vec!["*".into()],
        anonymous_write: vec!["*".into()],
        ..Policy::default()
    });
    let host = registry.host();
    let public = config(&dir, &registry, false);
    let reference = format!("{host}/unemu/copyleft-rom:1");
    let push = |dry_run: bool| {
        let mut args = os(&[
            &"image",
            &"push",
            &tag,
            &reference,
            &"--store",
            &store,
            &"--config",
            &public,
        ]);
        if dry_run {
            args.push("--dry-run".into());
        }
        child(&args, &dir.0, None)
    };
    // Every layer is open and the copyleft ROM travels with its source, so
    // even the official namespace of a public registry accepts it.
    let (code, output, errors) = push(true);
    assert_eq!(code, 0, "{errors}");
    assert!(
        output.contains("\tapplication/x-tar+zstd\topen\tthis image\n"),
        "{output}"
    );
    assert!(output.ends_with("guard:allowed\n"), "{output}");
    let (code, output, errors) = push(false);
    assert_eq!(code, 0, "{output}{errors}");
    let (digest, blobs) = closure(&store, &tag);
    assert!(
        output.ends_with(&format!("pushed\t{reference}\t{digest}\n")),
        "{output}"
    );
    let target = dir.join("target");
    let (code, output, errors) = child(
        &os(&[
            &"image",
            &"pull",
            &reference,
            &"--store",
            &target,
            &"--create",
            &"--tag",
            &tag,
            &"--config",
            &public,
        ]),
        &dir.0,
        None,
    );
    assert_eq!(code, 0, "{errors}");
    assert!(
        output.ends_with(&format!("pulled\t{tag}\t{digest}\n")),
        "{output}"
    );
    assert_eq!(closure(&target, &tag), (digest, blobs));
    let (code, show, errors) = run(&os(&[
        &"image",
        &"firmware",
        &"show",
        &tag,
        &"--store",
        &target,
    ]));
    assert_eq!(code, 0, "{errors}");
    assert!(
        show.contains("source\t\"copyleft-src.tar\"\ttar\t20480\t"),
        "{show}"
    );
}

#[test]
fn an_open_copyleft_rom_without_its_source_stays_in_private_registries() {
    let dir = Dir::new("firmware-no-source");
    let store = dir.join("store");
    // This program never writes such a package; another program might. Make
    // one from an all-open MIT package by restating the ROM layer's licence.
    let rom = dir.join("open.bin");
    std::fs::write(&rom, [0x4d; 8192]).unwrap();
    let (code, _, errors) = run(&os(&[
        &"image",
        &"firmware",
        &"import",
        &rom,
        &"--unlisted",
        &"permissive-rom",
        &"--role",
        &"option-rom",
        &"--open",
        &"--license",
        &"MIT",
        &"--store",
        &store,
        &"--create",
    ]));
    assert_eq!(code, 0, "{errors}");
    let opened = image_store::Store::open_existing(&store).unwrap();
    let image = opened
        .resolve(
            "firmware/permissive-rom:1",
            &image_store::firmware::Validator(&pc_image::firmware::PcFirmwareRules(&[])),
        )
        .unwrap();
    let mut manifest = image.manifest;
    for layer in &mut manifest.layers {
        if layer.decoded_media_type() == image_store::media_type::FIRMWARE {
            layer
                .annotations
                .insert("unemu.license".into(), "GPL-2.0-only".into());
        }
    }
    let crafted = opened.import_manifest(&manifest).unwrap();
    let tag = "foreign/bios:1";
    opened
        .update_tag(
            tag,
            &crafted,
            None,
            &image_store::firmware::Validator(&pc_image::firmware::PcFirmwareRules(&[])),
        )
        .unwrap();
    let registry = MockRegistry::start(Policy::default());
    let host = registry.host();
    let dry_run = |reference: String, private: bool| {
        run(&os(&[
            &"image",
            &"push",
            &tag,
            &reference,
            &"--store",
            &store,
            &"--config",
            &config(&dir, &registry, private),
            &"--dry-run",
        ]))
    };
    for (reference, private) in [
        (format!("{host}/owner/bios:1"), false),
        (format!("{host}/unemu/bios:1"), false),
        (format!("{host}/unemu/bios:1"), true),
    ] {
        let (code, output, errors) = dry_run(reference.clone(), private);
        assert_eq!(code, 1, "{reference}: {output}");
        assert!(output.ends_with("guard:refused\n"), "{output}");
        assert!(
            errors.contains("open copyleft ROMs have no source archive")
                && errors.contains("image firmware edit --source adds one"),
            "{errors}"
        );
    }
    let (code, output, errors) = dry_run(format!("{host}/private/bios:1"), true);
    assert_eq!(code, 0, "{errors}");
    assert!(output.ends_with("guard:allowed\n"), "{output}");
    let (code, show, errors) = run(&os(&[
        &"image",
        &"firmware",
        &"show",
        &tag,
        &"--store",
        &store,
    ]));
    assert_eq!(code, 0, "{errors}");
    assert!(
        show.contains(
            "push\tprivate registries outside unemu/ only: copyleft ROM without its source archive\n"
        ),
        "{show}"
    );
    assert!(registry.log().is_empty(), "{:?}", registry.log());
}
