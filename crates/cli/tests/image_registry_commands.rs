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

/// The FreeDOS reference machine; import marks every blob user-supplied.
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
    assert!(
        closure
            .iter()
            .all(|l| l.ends_with("\tuser-supplied\tthis image")),
        "{output}"
    );
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
    assert_eq!(listed, format!("private/lab:1.0.0\t{digest}\tmachine\n"));

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

/// The open Xi8088 BIOS as a firmware package. Its ROM layer is marked open
/// with licence and provenance; its config, like every config, defaults to
/// user-supplied (machine-images §5.3). Returns the tag and package digest,
/// or `None` when the BIOS fixture is not fetched.
fn import_open_firmware(dir: &Dir, store: &Path) -> Option<(String, String)> {
    let bytes = fixtures::fixture("kiselev-bios/bios-xi8088.bin")?;
    let folder = dir.join("roms");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("bios.bin"), bytes).unwrap();
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
    assert_eq!(listed, format!("{tag}\t{digest}\tfirmware\n"));
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
    let public = config(&dir, &registry, false);
    let (code, _, errors) = push(format!("{host}/private/rom:1"), &public, false);
    assert_eq!(code, 1);
    assert!(errors.contains("not marked private"), "{errors}");
    assert!(errors.contains(&rom), "{errors}");
    let private = config(&dir, &registry, true);
    let (code, _, errors) = push(format!("{host}/unemu/rom:1"), &private, false);
    assert_eq!(code, 1);
    assert!(errors.contains("official namespace"), "{errors}");
    assert!(errors.contains(&rom), "{errors}");
    let (code, output, errors) = push(format!("{host}/private/rom:1"), &private, true);
    assert_eq!(code, 0, "{errors}");
    assert!(
        output.contains(&format!(
            "layer\t{rom}\t32768\tapplication/vnd.unemu.firmware.v1\tuser-supplied\tthis image\n"
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
    let rom_layer = format!("layer\t{rom}\t32768\tapplication/vnd.unemu.firmware.v1\t");
    assert!(
        output.contains(&format!("{rom_layer}open\tthis image\n")),
        "{output}"
    );
    assert!(
        output.contains(
            "\tapplication/vnd.unemu.firmware.config.v1+toml\tuser-supplied\tthis image\n"
        ),
        "{output}"
    );
    assert!(output.ends_with("guard:allowed\n"), "{output}");

    // A machine image embedding the same BIOS records it as user-supplied;
    // the most restrictive local mark decides.
    import(&store, "lab:1.0.0");
    let (code, output, errors) = dry_run(&private);
    assert_eq!(code, 0, "{errors}");
    assert!(
        output.contains(&format!("{rom_layer}user-supplied\tlocal tag lab:1.0.0\n")),
        "{output}"
    );
    let public = config(&dir, &registry, false);
    let (code, output, errors) = dry_run(&public);
    assert_eq!(code, 1, "{output}");
    assert!(output.ends_with("guard:refused\n"), "{output}");
    assert!(errors.contains(&rom), "{errors}");
    assert!(registry.log().is_empty(), "{:?}", registry.log());
}
