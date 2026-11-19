// SPDX-License-Identifier: AGPL-3.0-or-later
//! Registry push, pull, listing, login and privacy commands; machine-images
//! §5.1. The registries list comes from the shared preferences file (or
//! `--config`); secrets come from the environment or the host credential store
//! only. `image login` goes through the same accounts functions and credential
//! store as the desktop shell's Registries page, so a login in one is visible
//! in the other, and `image registry set` writes the same keys as the page's
//! Add and Edit dialog. Transfers validate with every platform's
//! validators, and the push guard adds each platform's restrictions to the
//! local marks ([`super::super::CliPlatform::restricted`]).

use super::super::CliPlatform;
use anyhow::Context;
pub use image_registry::accounts::{registry_client, registry_probe_client};
use image_registry::{
    accounts,
    config::{self, RegistryConfig, DEFAULT_REGISTRY_ENV},
    credential_store::{self, CredentialStore, Secret},
    credentials::{EnvCredentials, HOST_ENV, PASSWORD_ENV, USER_ENV},
    pull, push, Client, Reference,
};
use image_store::Store;
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, PartialEq, Eq)]
pub enum RegistryCommand {
    Push {
        store: PathBuf,
        local_tag: String,
        target: Option<String>,
        options: Common,
        allow_move: bool,
        dry_run: bool,
    },
    Pull {
        store: PathBuf,
        source: String,
        local_tag: Option<String>,
        create: bool,
        options: Common,
    },
    RemoteList {
        target: String,
        options: Common,
    },
    Login {
        host: String,
        user: String,
        config: Option<PathBuf>,
    },
    /// `image registry set`: the whole registry private or public, or the
    /// listed namespaces, each `NAME`, `NAME:private` or `NAME:public`.
    Set {
        host: String,
        private: bool,
        namespaces: Vec<String>,
        config: Option<PathBuf>,
    },
}

/// Options shared by every networked command.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Common {
    registry: Option<String>,
    config: Option<PathBuf>,
}

pub fn parse(action: &str, args: &[OsString]) -> Result<RegistryCommand, &'static str> {
    let mut positional = Vec::new();
    let (mut store, mut tag, mut user) = (None, None, None);
    let mut common = Common::default();
    let (mut allow_move, mut dry_run, mut create, mut private) = (false, false, false, false);
    let mut namespaces = Vec::new();
    let transfer = matches!(action, "push" | "pull");
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let flag = match arg.to_str() {
            Some("--move") if action == "push" && !allow_move => &mut allow_move,
            Some("--dry-run") if action == "push" && !dry_run => &mut dry_run,
            Some("--create") if action == "pull" && !create => &mut create,
            Some("--private") if action == "registry" && !private => &mut private,
            Some(text) if text.starts_with("--") => {
                let value = args
                    .next()
                    .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
                    .ok_or("missing registry option value")?;
                let text_value = value.to_str().ok_or("invalid registry option value")?;
                match text {
                    "--store" if transfer && store.is_none() => store = Some(PathBuf::from(value)),
                    "--registry"
                        if action != "login"
                            && action != "registry"
                            && common.registry.is_none() =>
                    {
                        common.registry = Some(text_value.to_owned())
                    }
                    "--namespace" if action == "registry" => namespaces.push(text_value.to_owned()),
                    "--config" if common.config.is_none() => {
                        common.config = Some(PathBuf::from(value))
                    }
                    "--tag" if action == "pull" && tag.is_none() => {
                        tag = Some(text_value.to_owned())
                    }
                    "--user" if action == "login" && user.is_none() => {
                        user = Some(text_value.to_owned())
                    }
                    _ => return Err("unknown or duplicate registry option"),
                }
                continue;
            }
            Some(text) if !text.is_empty() => {
                positional.push(text.to_owned());
                continue;
            }
            _ => return Err("invalid registry argument"),
        };
        *flag = true;
    }
    match action {
        "push" => {
            let mut positional = positional.into_iter();
            let local_tag = positional.next().ok_or("image push requires NAME:TAG")?;
            let target = positional.next();
            if positional.next().is_some() {
                return Err("image push takes a local tag and an optional reference");
            }
            Ok(RegistryCommand::Push {
                store: store.ok_or("image commands require --store DIR")?,
                local_tag,
                target,
                options: common,
                allow_move,
                dry_run,
            })
        }
        "pull" => {
            let [source] = <[String; 1]>::try_from(positional)
                .map_err(|_| "image pull requires exactly one reference")?;
            Ok(RegistryCommand::Pull {
                store: store.ok_or("image commands require --store DIR")?,
                source,
                local_tag: tag,
                create,
                options: common,
            })
        }
        "remote" => match positional.as_slice() {
            [ls, target] if ls == "ls" => Ok(RegistryCommand::RemoteList {
                target: target.clone(),
                options: common,
            }),
            _ => Err("expected image remote ls REPOSITORY|REGISTRY"),
        },
        "login" => {
            let [host] = <[String; 1]>::try_from(positional)
                .map_err(|_| "image login requires exactly one registry host")?;
            Ok(RegistryCommand::Login {
                host,
                user: user.ok_or("image login requires --user NAME")?,
                config: common.config,
            })
        }
        "registry" => match <[String; 2]>::try_from(positional) {
            Ok([set, host]) if set == "set" => {
                if private && !namespaces.is_empty() {
                    return Err("--private marks the whole registry; mark a listed namespace NAME:private instead");
                }
                Ok(RegistryCommand::Set {
                    host,
                    private,
                    namespaces,
                    config: common.config,
                })
            }
            _ => Err("expected image registry set HOST"),
        },
        _ => Err("invalid registry command"),
    }
}

fn load(options: &Common) -> anyhow::Result<(RegistryConfig, String)> {
    let path = match &options.config {
        Some(path) => path.clone(),
        None => image_store::location::preferences::path()?,
    };
    let config = RegistryConfig::from_table(&config::read_preferences(&path)?)?;
    let env = std::env::var(DEFAULT_REGISTRY_ENV).ok();
    let host = config.default_host(options.registry.as_deref(), env.as_deref())?;
    Ok((config, host))
}

fn client(
    config: &RegistryConfig,
    host: &str,
    store: &Arc<dyn CredentialStore>,
) -> anyhow::Result<Client> {
    Ok(registry_client(
        config,
        host,
        store,
        &EnvCredentials::from_env(),
    )?)
}

/// A reference resolved as `image push` and `image pull` resolve theirs:
/// against the default registry of `--registry`, the environment or the
/// preferences (`config`, or the shared file), with its registry entry and
/// a client holding the registry's credentials. `recipe push` and `recipe
/// pull` use it.
pub(crate) fn resolve(
    registry: Option<&str>,
    config: Option<&Path>,
    text: &str,
) -> anyhow::Result<(Reference, Option<image_registry::RegistryEntry>, Client)> {
    let options = Common {
        registry: registry.map(str::to_owned),
        config: config.map(Path::to_path_buf),
    };
    let (config, default_host) = load(&options)?;
    let reference = Reference::parse(text, &default_host)?;
    if reference.host != default_host {
        conflicting_registry(&options, &reference)?;
    }
    let entry = config.entry(&reference.host);
    let client = client(&config, &reference.host, &credential_store::detect())?;
    Ok((reference, entry, client))
}

fn conflicting_registry(options: &Common, reference: &Reference) -> anyhow::Result<()> {
    if let Some(flag) = &options.registry {
        if !flag.eq_ignore_ascii_case(&reference.host) {
            anyhow::bail!("--registry {flag} conflicts with the host in {}", reference);
        }
    }
    Ok(())
}

pub fn execute(
    command: RegistryCommand,
    output: &mut impl Write,
    platforms: &[&dyn CliPlatform],
) -> anyhow::Result<()> {
    let providers = &super::super::validators(platforms)[..];
    let secrets = credential_store::detect();
    match command {
        RegistryCommand::Push {
            store,
            local_tag,
            target,
            options,
            allow_move,
            dry_run,
        } => {
            let (config, default_host) = load(&options)?;
            let store = Store::open_existing(store)?;
            let target = Reference::parse(target.as_deref().unwrap_or(&local_tag), &default_host)?;
            if target.host != default_host {
                conflicting_registry(&options, &target)?;
            }
            let entry = config.entry(&target.host);
            let plan =
                push::plan_with_catalogue(&store, &local_tag, target, entry.as_ref(), |m| {
                    super::restricted(&store, m, platforms)
                })?;
            if dry_run {
                writeln!(output, "target:{}", plan.target())?;
                writeln!(output, "image:{}", plan.image().digest)?;
                if let Some(scope) = plan.scope() {
                    writeln!(output, "scope:{scope}")?;
                }
                for entry in plan.closure() {
                    writeln!(output, "{entry}")?;
                }
                match plan.verdict() {
                    Ok(()) => writeln!(output, "guard:allowed")?,
                    Err(refusal) => {
                        writeln!(output, "guard:refused")?;
                        return Err(image_registry::Error::from(refusal.clone()).into());
                    }
                }
                return Ok(());
            }
            plan.verdict()
                .clone()
                .map_err(image_registry::Error::from)?;
            let client = client(&config, &plan.target().host, &secrets)?;
            let options = push::PushOptions {
                allow_move,
                ..push::PushOptions::default()
            };
            let mut progress = |d: &image_store::Descriptor, o: push::BlobOutcome| {
                let _ = writeln!(output, "{}\t{}", o.as_str(), d.digest);
            };
            let report = push::push(&store, &plan, providers, &client, &options, &mut progress)?;
            match report.tag {
                push::TagOutcome::Unchanged => writeln!(
                    output,
                    "unchanged\t{}\t{}",
                    plan.target(),
                    report.image.digest
                )?,
                push::TagOutcome::Moved { from } => writeln!(
                    output,
                    "moved\t{}\t{from} -> {}",
                    plan.target(),
                    report.image.digest
                )?,
                push::TagOutcome::Created | push::TagOutcome::Untagged => {
                    writeln!(output, "pushed\t{}\t{}", plan.target(), report.image.digest)?
                }
            }
        }
        RegistryCommand::Pull {
            store,
            source,
            local_tag,
            create,
            options,
        } => {
            let (config, default_host) = load(&options)?;
            let source = Reference::parse(&source, &default_host)?;
            if source.host != default_host {
                conflicting_registry(&options, &source)?;
            }
            let local_tag = match local_tag.or_else(|| source.local_tag()) {
                Some(tag) => tag,
                None => anyhow::bail!("pulling by digest requires --tag NAME:TAG"),
            };
            Store::validate_tag_name(&local_tag)?;
            let store = if create {
                Store::open(store)?
            } else {
                Store::open_existing(store)?
            };
            let client = client(&config, &source.host, &secrets)?;
            let report = pull::pull(&store, &client, &source, &local_tag, providers, &|_| {})?;
            for descriptor in &report.fetched {
                writeln!(output, "fetched\t{}", descriptor.digest)?;
            }
            for descriptor in &report.present {
                writeln!(output, "present\t{}", descriptor.digest)?;
            }
            // A set or package pulled by its release moves that alias too.
            if let Some(alias) = &report.alias {
                writeln!(output, "alias\t{alias}\t{}", report.image.digest)?;
            }
            let word = match report.outcome {
                pull::PullOutcome::Pulled => "pulled",
                pull::PullOutcome::Unchanged => "unchanged",
            };
            writeln!(
                output,
                "{word}\t{}\t{}",
                report.local_tag, report.image.digest
            )?;
        }
        RegistryCommand::RemoteList { target, options } => {
            let (config, default_host) = load(&options)?;
            let registry_only = !target.contains('/')
                && (target.contains('.') || target.contains(':') || target == "localhost");
            if registry_only {
                let host = target.to_ascii_lowercase();
                let client = client(&config, &host, &secrets)?;
                client.use_credentials_now()?;
                for name in pull::list_repositories(&client, 100)? {
                    writeln!(output, "{host}/{name}")?;
                }
            } else {
                let (host, repository) = Reference::parse_repository(&target, &default_host)?;
                let client = client(&config, &host, &secrets)?;
                for tag in pull::list_tags(&client, &repository, 100)? {
                    writeln!(output, "{host}/{repository}:{tag}")?;
                }
            }
        }
        RegistryCommand::Login { host, user, config } => {
            let path = match config {
                Some(path) => path,
                None => image_store::location::preferences::path()?,
            };
            log_in(&path, &host, &user, &*secrets, read_secret, output)?;
        }
        RegistryCommand::Set {
            host,
            private,
            namespaces,
            config,
        } => {
            let path = match config {
                Some(path) => path,
                None => image_store::location::preferences::path()?,
            };
            set_privacy(&path, &host, private, &namespaces, output)?;
        }
    }
    Ok(())
}

/// Write how private `host` is under the preferences lock, keeping its user
/// and every other key, then print what the entry says now: one `registry`
/// line, then a `namespace` line for each listed namespace.
fn set_privacy(
    path: &Path,
    host: &str,
    private: bool,
    namespaces: &[String],
    output: &mut impl Write,
) -> anyhow::Result<()> {
    let host = config::validate_registry_host(host)?;
    let namespaces = namespaces
        .iter()
        .map(|item| config::parse_namespace(item, false))
        .collect::<image_registry::Result<Vec<_>>>()?;
    let table = config::update_preferences(path, |table| {
        config::set_privacy(table, &host, private, namespaces)
    })?;
    let entry = RegistryConfig::from_table(&table)?
        .entry(&host)
        .context("the registry entry was not written")?;
    let whole = match (entry.namespaces.is_empty(), entry.private) {
        (false, _) => "per namespace",
        (true, true) => "private",
        (true, false) => "public",
    };
    writeln!(output, "registry\t{host}\t{whole}")?;
    for namespace in &entry.namespaces {
        let mark = if namespace.private {
            "private"
        } else {
            "public"
        };
        writeln!(output, "namespace\t{host}/{}/\t{mark}", namespace.name)?;
    }
    Ok(())
}

/// Check the target before asking for the secret, then let the registry
/// accept the login before the secret is stored and the user recorded
/// ([`accounts::log_in`]). A session-only store would forget the secret when
/// this command exits, so scripts use the environment variables instead.
fn log_in(
    path: &Path,
    host: &str,
    user: &str,
    store: &dyn CredentialStore,
    secret: impl FnOnce(&str, &str) -> anyhow::Result<Secret>,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    if !store.kind().persistent() {
        anyhow::bail!(
            "no credential store on this host; scripts and CI set {USER_ENV}, {PASSWORD_ENV} and {HOST_ENV}"
        );
    }
    let host = config::validate_registry_host(host)?;
    config::set_user(&mut config::read_preferences(path)?, &host, user)?;
    let secret = secret(&host, user)?;
    let outcome = accounts::log_in(path, &host, user, &secret, store, &|| true)?;
    writeln!(
        output,
        "stored\t{}\t{user}",
        credential_store::item_name(&host)
    )?;
    if outcome == accounts::LoginOutcome::Unverifiable {
        writeln!(
            output,
            "unverified\t{host}\tthe registry answers without login, so it could not confirm these credentials"
        )?;
    }
    Ok(())
}

/// The password or token: typed on the terminal without echo, or one line of
/// standard input when that is not a terminal. Either way it never appears in
/// the process list or the shell history.
fn read_secret(host: &str, user: &str) -> anyhow::Result<Secret> {
    use std::io::{BufRead, IsTerminal, Read};
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprint!("Password or token for {user} at {host}: ");
        let typed = terminal::read_password(&mut stdin.lock());
        eprintln!();
        return typed;
    }
    let mut line = String::new();
    stdin.lock().take(64 * 1024).read_line(&mut line)?;
    let secret = line.strip_suffix('\n').unwrap_or(&line);
    let secret = secret.strip_suffix('\r').unwrap_or(secret);
    if secret.is_empty() {
        anyhow::bail!("no password or token was given");
    }
    Ok(Secret::new(secret))
}

/// Password entry on a terminal. While the password is typed, the terminal
/// runs without canonical line editing, echo or signal keys (POSIX termios:
/// `ICANON`, `ECHO` and `ISIG` cleared, `VMIN` 1), so Ctrl+C arrives as a byte
/// that ends the prompt instead of killing the process, and the saved mode is
/// restored on every way out, including a panic.
mod terminal {
    use image_registry::credential_store::Secret;
    use std::io::Read;

    /// What one typed byte does to the entry.
    #[derive(Debug, PartialEq, Eq)]
    pub(super) enum Step {
        More,
        Done,
        Interrupted,
    }

    #[derive(Default)]
    pub(super) struct Entry {
        bytes: Vec<u8>,
    }

    impl Entry {
        pub fn feed(&mut self, byte: u8) -> Step {
            match byte {
                // Enter as carriage return or line feed, and Ctrl+D, end the entry.
                b'\r' | b'\n' | 0x04 => Step::Done,
                // Ctrl+C.
                0x03 => Step::Interrupted,
                // Delete and backspace remove the last character with all its
                // UTF-8 continuation bytes.
                0x7f | 0x08 => {
                    while let Some(last) = self.bytes.pop() {
                        if last & 0xc0 != 0x80 {
                            break;
                        }
                    }
                    Step::More
                }
                // Ctrl+U discards everything typed so far.
                0x15 => {
                    self.bytes.clear();
                    Step::More
                }
                byte => {
                    self.bytes.push(byte);
                    Step::More
                }
            }
        }

        fn finish(self) -> anyhow::Result<Secret> {
            let text = String::from_utf8(self.bytes)
                .map_err(|_| anyhow::anyhow!("the password is not valid UTF-8"))?;
            if text.is_empty() {
                anyhow::bail!("no password or token was given");
            }
            Ok(Secret::new(text))
        }
    }

    /// Feed `input` byte by byte to an [`Entry`] until it is done.
    pub(super) fn read_entry(input: &mut impl Read) -> anyhow::Result<Secret> {
        let mut entry = Entry::default();
        let mut byte = [0u8; 1];
        loop {
            match input.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => match entry.feed(byte[0]) {
                    Step::More => {}
                    Step::Done => break,
                    Step::Interrupted => anyhow::bail!("login cancelled at the password prompt"),
                },
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        entry.finish()
    }

    #[cfg(unix)]
    pub(super) fn read_password(input: &mut impl Read) -> anyhow::Result<Secret> {
        let _raw = RawMode::enter(libc::STDIN_FILENO)?;
        read_entry(input)
    }

    #[cfg(not(unix))]
    pub(super) fn read_password(_: &mut impl Read) -> anyhow::Result<Secret> {
        anyhow::bail!("cannot hide typed input here; pipe the password on standard input instead")
    }

    /// The terminal at `fd` without canonical mode, echo and signal keys until
    /// dropped, when its saved mode returns.
    #[cfg(unix)]
    pub(super) struct RawMode {
        fd: libc::c_int,
        saved: libc::termios,
    }

    #[cfg(unix)]
    impl RawMode {
        pub fn enter(fd: libc::c_int) -> anyhow::Result<Self> {
            let failed = |error: std::io::Error| {
                anyhow::anyhow!(
                    "cannot turn off terminal echo ({error}); pipe the password on standard input instead"
                )
            };
            let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
            // tcgetattr fills the whole structure when it succeeds.
            if unsafe { libc::tcgetattr(fd, saved.as_mut_ptr()) } != 0 {
                return Err(failed(std::io::Error::last_os_error()));
            }
            let saved = unsafe { saved.assume_init() };
            let mut raw = saved;
            raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            // `raw` is the terminal's own settings with three flags cleared.
            if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
                return Err(failed(std::io::Error::last_os_error()));
            }
            Ok(Self { fd, saved })
        }
    }

    #[cfg(unix)]
    impl Drop for RawMode {
        fn drop(&mut self) {
            // The saved structure came from tcgetattr on the same terminal.
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_registry::{
        credential_store::{SessionStore, StoreKind},
        test_support::{MockRegistry, Policy},
    };

    /// A store holding `toy/rom:1`, a toy machine image every blob of which
    /// it marks open, among them one ROM; returns the ROM's digest.
    fn open_toy_image(store: &Store) -> image_store::Digest {
        use image_store::{license, media_type, BlobSource, ConfigValidator, Descriptor, Manifest};
        struct ToyImages;
        impl ConfigValidator for ToyImages {
            fn platform(&self) -> &str {
                "toy"
            }
            fn validate(
                &self,
                _: &toml::Table,
                descriptors: &[Descriptor],
                _: &dyn BlobSource,
            ) -> image_store::Result<Vec<image_store::Digest>> {
                Ok(descriptors
                    .iter()
                    .filter(|d| d.media_type == media_type::FIRMWARE)
                    .map(|d| d.digest.clone())
                    .collect())
            }
        }
        let open = |media: &str, bytes: &[u8]| {
            let blob = store.import(bytes, None).unwrap();
            let mut descriptor = Descriptor::new(media, blob.digest, blob.size);
            descriptor.annotations.extend([
                (license::REDISTRIBUTION.into(), license::OPEN.into()),
                (license::LICENSE.into(), "MIT".into()),
            ]);
            descriptor
        };
        let config = open(media_type::CONFIG, b"schema = 1\nplatform = \"toy\"\n");
        let rom = open(media_type::FIRMWARE, b"synthetic rom");
        let metadata = image_store::metadata::Metadata::new("toy")
            .encode(&[])
            .unwrap();
        let metadata = open(media_type::METADATA, &metadata);
        let manifest = Manifest::new("toy", config, vec![rom.clone(), metadata]).unwrap();
        let image = store.import_manifest(&manifest).unwrap();
        store
            .update_tag("toy/rom:1", &image, None, &ToyImages)
            .unwrap();
        rom.digest
    }

    /// The push guard of `image push` adds each platform's restrictions to
    /// the store's marks: a ROM the image marks open but a platform restricts
    /// is user-supplied by the catalogue, so a dry run to a private registry
    /// lists it so and one to the official namespace is refused. Dry runs
    /// plan only; nothing is sent.
    #[test]
    fn a_push_keeps_what_a_platform_restricts_user_supplied() {
        use crate::tests::Toy;
        let temp =
            Dir(std::env::temp_dir().join(format!("cli-push-restricted-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&temp.0);
        let store = Store::open(temp.0.join("store")).unwrap();
        let rom = open_toy_image(&store);
        let config = temp.0.join("preferences.toml");
        std::fs::write(
            &config,
            "[[registries]]\nhost = \"registry.example\"\nprivate = true\n",
        )
        .unwrap();
        let push = |target: &str, platforms: &[&dyn CliPlatform]| {
            let args: Vec<OsString> = [
                "toy/rom:1".into(),
                target.into(),
                "--store".into(),
                temp.0.join("store").into_os_string(),
                "--dry-run".into(),
                "--config".into(),
                config.clone().into_os_string(),
            ]
            .into();
            let mut output = Vec::new();
            let result = execute(parse("push", &args).unwrap(), &mut output, platforms);
            (result.is_ok(), String::from_utf8(output).unwrap())
        };
        let rom_line = |output: &str| {
            output
                .lines()
                .find(|line| line.contains(&rom.to_string()))
                .unwrap()
                .to_owned()
        };
        let restricting = Toy {
            restricted: vec![rom.clone()],
        };
        let (ok, output) = push("registry.example/team/rom:1", &[&restricting]);
        assert!(ok, "{output}");
        assert!(
            rom_line(&output).ends_with("\tuser-supplied\tfirmware catalogue"),
            "{output}"
        );
        assert!(output.ends_with("guard:allowed\n"), "{output}");
        let (ok, output) = push("registry.example/unemu/rom:1", &[&restricting]);
        assert!(!ok);
        assert!(output.ends_with("guard:refused\n"), "{output}");
        // Every platform is asked, not the first alone.
        let (ok, output) = push(
            "registry.example/unemu/rom:1",
            &[&Toy::default(), &restricting],
        );
        assert!(!ok);
        assert!(output.ends_with("guard:refused\n"), "{output}");
        // Without the restriction the same image may go anywhere.
        let (ok, output) = push("registry.example/unemu/rom:1", &[&Toy::default()]);
        assert!(ok, "{output}");
        assert!(
            rom_line(&output).ends_with("\topen\tthis image"),
            "{output}"
        );
        assert!(output.ends_with("guard:allowed\n"), "{output}");
    }

    const SECRET: &str = "synthetic-secret";

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A persistent store as on a desktop host, kept in memory so the test
    /// never reaches the real Keychain.
    #[derive(Default)]
    struct Persistent(SessionStore);
    impl CredentialStore for Persistent {
        fn kind(&self) -> StoreKind {
            StoreKind::Keychain
        }
        fn read(&self, host: &str, user: &str) -> image_registry::Result<Option<Secret>> {
            self.0.read(host, user)
        }
        fn store(&self, host: &str, user: &str, secret: &Secret) -> image_registry::Result<()> {
            self.0.store(host, user, secret)
        }
        fn delete(&self, host: &str, user: &str) -> image_registry::Result<()> {
            self.0.delete(host, user)
        }
    }

    #[test]
    fn a_login_is_checked_by_the_registry_and_keeps_a_save_made_during_the_prompt() {
        let dir =
            Dir(std::env::temp_dir().join(format!("cli-registry-login-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let path = dir.0.join("preferences.toml");
        let registry = MockRegistry::start(Policy {
            user: Some(("ci".into(), SECRET.into())),
            ..Policy::default()
        });
        let host = registry.host();
        std::fs::write(
            &path,
            format!(
                "rom_dirs = []\nreduced_motion = false\n[[registries]]\nhost = \"{host}\"\nprivate = true\nplain_http = true\n"
            ),
        )
        .unwrap();
        let store = Persistent::default();
        let mut output = Vec::new();
        log_in(
            &path,
            &host,
            "ci",
            &store,
            |_, _| {
                // The desktop shell saves while the password prompt is open.
                config::update_preferences(&path, |table| {
                    table.insert("reduced_motion".into(), true.into());
                    table.insert("rom_dirs".into(), vec!["/roms"].into());
                    Ok(())
                })?;
                Ok(Secret::new(SECRET))
            },
            &mut output,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert_eq!(output, format!("stored\tunemu-registry:{host}\tci\n"));
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains(SECRET) && !output.contains(SECRET));
        let table = config::read_preferences(&path).unwrap();
        assert_eq!(table["reduced_motion"].as_bool(), Some(true));
        assert_eq!(table["rom_dirs"][0].as_str(), Some("/roms"));
        let entry = RegistryConfig::from_table(&table)
            .unwrap()
            .entry(&host)
            .unwrap();
        assert_eq!(entry.user.as_deref(), Some("ci"));
        assert!(entry.private);
        assert_eq!(store.read(&host, "ci").unwrap(), Some(Secret::new(SECRET)));

        let before = std::fs::read(&path).unwrap();
        let refused = log_in(
            &path,
            &host,
            "ci",
            &store,
            |_, _| Ok(Secret::new("wrong-secret")),
            &mut Vec::new(),
        );
        let message = format!("{:#}", refused.unwrap_err());
        assert!(message.contains("rejected the credentials"), "{message}");
        assert!(!message.contains("wrong-secret"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(store.read(&host, "ci").unwrap(), Some(Secret::new(SECRET)));
    }

    #[test]
    fn the_password_entry_edits_ends_and_cancels_like_a_terminal_line() {
        use terminal::{Entry, Step};
        let feed = |bytes: &[u8]| {
            let mut input = bytes;
            terminal::read_entry(&mut input)
        };
        assert_eq!(feed(b"se\x7fecret\r").unwrap(), Secret::new("secret"));
        // Backspace removes a whole two-byte character.
        assert_eq!(feed(b"p\xc5\x82\x08x\n").unwrap(), Secret::new("px"));
        assert_eq!(feed(b"wrong\x15right\r").unwrap(), Secret::new("right"));
        assert_eq!(feed(b"ends-at-eof").unwrap(), Secret::new("ends-at-eof"));
        assert_eq!(feed(b"ctrl-d\x04rest").unwrap(), Secret::new("ctrl-d"));
        let cancelled = feed(b"abc\x03def\r").unwrap_err();
        assert!(format!("{cancelled:#}").contains("cancelled"));
        assert!(feed(b"\r").is_err());
        let mut entry = Entry::default();
        assert_eq!(entry.feed(b'a'), Step::More);
        assert_eq!(entry.feed(0x03), Step::Interrupted);
    }

    /// A pseudo-terminal pair (POSIX `posix_openpt`, `grantpt`, `unlockpt`,
    /// `ptsname`), neither of them this process's controlling terminal.
    #[cfg(unix)]
    fn pty() -> (std::fs::File, std::fs::File) {
        use std::os::fd::FromRawFd;
        let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
        assert!(master >= 0, "{}", std::io::Error::last_os_error());
        // The descriptor is a fresh master; ptsname's buffer is copied at once.
        let slave = unsafe {
            assert_eq!(libc::grantpt(master), 0);
            assert_eq!(libc::unlockpt(master), 0);
            let name = std::ffi::CStr::from_ptr(libc::ptsname(master)).to_owned();
            libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY)
        };
        assert!(slave >= 0, "{}", std::io::Error::last_os_error());
        // Both descriptors are open and owned by nothing else.
        unsafe {
            (
                std::fs::File::from_raw_fd(master),
                std::fs::File::from_raw_fd(slave),
            )
        }
    }

    #[cfg(unix)]
    #[test]
    fn raw_mode_hides_typing_and_restores_the_terminal_on_every_exit() {
        use std::os::fd::AsRawFd;
        let (mut master, slave) = pty();
        let fd = slave.as_raw_fd();
        let lflag = || {
            let mut mode = std::mem::MaybeUninit::<libc::termios>::uninit();
            // tcgetattr fills the structure on success, checked here.
            assert_eq!(unsafe { libc::tcgetattr(fd, mode.as_mut_ptr()) }, 0);
            // PENDIN is the kernel's own note that input awaits re-reading
            // after a mode change, not a setting the prompt restores.
            unsafe { mode.assume_init() }.c_lflag & !libc::PENDIN
        };
        let hidden = libc::ECHO | libc::ICANON | libc::ISIG;
        let before = lflag();
        assert_eq!(before & hidden, hidden, "a new terminal echoes");
        {
            let _raw = terminal::RawMode::enter(fd).unwrap();
            assert_eq!(lflag() & hidden, 0);
            master.write_all(b"se\x7fecret\r").unwrap();
            assert_eq!(
                terminal::read_entry(&mut &slave).unwrap(),
                Secret::new("secret")
            );
        }
        assert_eq!(lflag(), before);
        let cancelled = {
            let _raw = terminal::RawMode::enter(fd).unwrap();
            master.write_all(b"abc\x03").unwrap();
            terminal::read_entry(&mut &slave)
        };
        assert!(cancelled.is_err());
        assert_eq!(lflag(), before, "Ctrl+C restores the mode");
        let panicked = std::panic::catch_unwind(|| {
            let _raw = terminal::RawMode::enter(fd).unwrap();
            panic!("injected failure while the password is typed");
        });
        assert!(panicked.is_err());
        assert_eq!(lflag(), before, "a panic restores the mode");
    }

    #[test]
    fn invalid_logins_fail_before_the_password_prompt() {
        let dir =
            Dir(std::env::temp_dir()
                .join(format!("cli-registry-login-invalid-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let path = dir.0.join("preferences.toml");
        let prompt = |_: &str, _: &str| -> anyhow::Result<Secret> {
            panic!("an invalid login must fail before the prompt")
        };
        for (host, user) in [("registry.example", "a:b"), ("no-dot-host", "me")] {
            let refused = log_in(
                &path,
                host,
                user,
                &Persistent::default(),
                prompt,
                &mut Vec::new(),
            );
            assert!(refused.is_err(), "{host} {user}");
        }
        let refused = log_in(
            &path,
            "registry.example",
            "me",
            &SessionStore::default(),
            prompt,
            &mut Vec::new(),
        );
        assert!(
            format!("{:#}", refused.unwrap_err()).contains(USER_ENV),
            "a session-only store points to the environment variables"
        );
        assert!(!path.exists());
    }
}
