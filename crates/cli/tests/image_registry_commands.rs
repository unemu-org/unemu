// SPDX-License-Identifier: AGPL-3.0-or-later
//! Registry push, pull and listing commands against the in-process mock
//! registry; machine-images §5.1. Commands that talk to the registry run the
//! binary as a child process so each gets its own environment: synthetic
//! credentials, and none of the caller's registry settings.

use image_registry::test_support::{MockRegistry, Policy};
use std::{
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
