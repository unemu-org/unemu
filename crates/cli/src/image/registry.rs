// SPDX-License-Identifier: AGPL-3.0-or-later
//! Registry push, pull, listing and login commands; machine-images §5.1.
//! The registries list comes from the shared preferences file (or `--config`);
//! secrets come from the environment or the host credential store only.

use anyhow::Context;
use image_registry::{
    config::{self, RegistryConfig, DEFAULT_REGISTRY_ENV},
    credentials::{keychain, Anonymous, EnvCredentials, HostCredentials},
    pull, push, Client, Reference,
};
use image_store::{firmware, media_set, ConfigValidator, Store};
use pc_image::firmware::{catalogue, PcFirmwareRules};
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
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
    let (mut allow_move, mut dry_run, mut create) = (false, false, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let flag = match arg.to_str() {
            Some("--move") if action == "push" && !allow_move => &mut allow_move,
            Some("--dry-run") if action == "push" && !dry_run => &mut dry_run,
            Some("--create") if action == "pull" && !create => &mut create,
            Some(text) if text.starts_with("--") => {
                let value = args
                    .next()
                    .filter(|s| !s.is_empty() && !s.as_encoded_bytes().starts_with(b"--"))
                    .ok_or("missing registry option value")?;
                let text_value = value.to_str().ok_or("invalid registry option value")?;
                match text {
                    "--store" if action != "remote" && action != "login" && store.is_none() => {
                        store = Some(PathBuf::from(value))
                    }
                    "--registry" if action != "login" && common.registry.is_none() => {
                        common.registry = Some(text_value.to_owned())
                    }
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
        _ => Err("invalid registry command"),
    }
}

/// Platform data directory of the shared preferences file.
fn preferences_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let root = if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Application Support"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".config")))
    };
    root.map(|r| r.join("unemupc98/preferences.toml"))
        .context("platform config directory is unavailable")
}

fn read_preferences(path: &Path) -> anyhow::Result<toml::Table> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse()
            .with_context(|| format!("invalid preferences {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn load(options: &Common) -> anyhow::Result<(RegistryConfig, String)> {
    let path = match &options.config {
        Some(path) => path.clone(),
        None => preferences_path()?,
    };
    let config = RegistryConfig::from_table(&read_preferences(&path)?)?;
    let env = std::env::var(DEFAULT_REGISTRY_ENV).ok();
    let host = config.default_host(options.registry.as_deref(), env.as_deref())?;
    Ok((config, host))
}

/// Secrets are offered only to configured registries (and the family
/// default), so a pull from an arbitrary host can never receive them. The
/// environment pair is further bound to its one host.
fn client(config: &RegistryConfig, host: &str) -> anyhow::Result<Client> {
    Ok(match config.entry(host) {
        Some(entry) => {
            let credentials = HostCredentials {
                configured_user: entry.user.clone(),
                env: EnvCredentials::from_env(),
            };
            Client::new(&entry, Box::new(credentials))?
        }
        None => Client::new(&config.entry_or_public(host), Box::new(Anonymous))?,
    })
}

fn conflicting_registry(options: &Common, reference: &Reference) -> anyhow::Result<()> {
    if let Some(flag) = &options.registry {
        if !flag.eq_ignore_ascii_case(&reference.host) {
            anyhow::bail!("--registry {flag} conflicts with the host in {}", reference);
        }
    }
    Ok(())
}

const MEDIA_SETS: media_set::Validator<'static> =
    media_set::Validator(&pc_image::media_set::PcMediaFormats);

pub fn execute(command: RegistryCommand, output: &mut impl Write) -> anyhow::Result<()> {
    let firmware_rules = PcFirmwareRules(catalogue());
    let firmware = firmware::Validator(&firmware_rules);
    let providers: [&dyn ConfigValidator; 3] = [&pc_image::PcProvider, &MEDIA_SETS, &firmware];
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
            let plan = push::plan(&store, &local_tag, target, entry.as_ref())?;
            if dry_run {
                writeln!(output, "target:{}", plan.target())?;
                writeln!(output, "image:{}", plan.image().digest)?;
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
            let client = client(&config, &plan.target().host)?;
            let options = push::PushOptions {
                allow_move,
                ..push::PushOptions::default()
            };
            let mut progress = |d: &image_store::Descriptor, o: push::BlobOutcome| {
                let _ = writeln!(output, "{}\t{}", o.as_str(), d.digest);
            };
            let report = push::push(&store, &plan, &providers, &client, &options, &mut progress)?;
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
            let client = client(&config, &source.host)?;
            let report = pull::pull(&store, &client, &source, &local_tag, &providers, &|_| {})?;
            for descriptor in &report.fetched {
                writeln!(output, "fetched\t{}", descriptor.digest)?;
            }
            for descriptor in &report.present {
                writeln!(output, "present\t{}", descriptor.digest)?;
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
                let client = client(&config, &host)?;
                client.use_credentials_now()?;
                for name in pull::list_repositories(&client, 100)? {
                    writeln!(output, "{host}/{name}")?;
                }
            } else {
                let (host, repository) = Reference::parse_repository(&target, &default_host)?;
                let client = client(&config, &host)?;
                for tag in pull::list_tags(&client, &repository, 100)? {
                    writeln!(output, "{host}/{repository}:{tag}")?;
                }
            }
        }
        RegistryCommand::Login { host, user, config } => {
            let path = match config {
                Some(path) => path,
                None => preferences_path()?,
            };
            let host = host.to_ascii_lowercase();
            record_login(&path, &host, &user, || {
                keychain::store_interactive(&host, &user).map_err(Into::into)
            })?;
            writeln!(output, "stored\t{}\t{user}", keychain::service(&host))?;
        }
    }
    Ok(())
}

/// Check the preferences, let `store_secret` run (it may wait on a password
/// prompt), then record the user under the preferences lock in a fresh read
/// of the file, so a desktop-shell save made meanwhile is kept.
fn record_login(
    path: &Path,
    host: &str,
    user: &str,
    store_secret: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    config::set_user(&mut read_preferences(path)?, host, user)?;
    store_secret()?;
    update_preferences(path, |table| Ok(config::set_user(table, host, user)?))
}

/// Read-modify-write of the shared preferences file under an advisory lock on
/// the sibling `<file>.lock`, which the desktop shell takes for its saves too.
fn update_preferences(
    path: &Path,
    change: impl FnOnce(&mut toml::Table) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut name = path
        .file_name()
        .context("preferences path has no file name")?
        .to_owned();
    name.push(".lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(parent.join(name))
        .with_context(|| format!("cannot lock {}", path.display()))?;
    lock.lock()
        .with_context(|| format!("cannot lock {}", path.display()))?;
    let mut table = read_preferences(path)?;
    change(&mut table)?;
    write_preferences(path, &table)
}

/// Replace the preferences file atomically; the caller holds the lock.
fn write_preferences(path: &Path, table: &toml::Table) -> anyhow::Result<()> {
    let text = toml::to_string(table)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".preferences-{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_login_keeps_a_preferences_save_made_during_the_password_prompt() {
        let dir =
            Dir(std::env::temp_dir().join(format!("cli-registry-login-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let path = dir.0.join("preferences.toml");
        std::fs::write(&path, "rom_dirs = []\nreduced_motion = false\n").unwrap();
        record_login(&path, "registry.example", "me", || {
            // The desktop shell saves while the password prompt is open.
            std::fs::write(&path, "rom_dirs = [\"/roms\"]\nreduced_motion = true\n")?;
            Ok(())
        })
        .unwrap();
        let table = read_preferences(&path).unwrap();
        assert_eq!(table["reduced_motion"].as_bool(), Some(true));
        assert_eq!(table["rom_dirs"][0].as_str(), Some("/roms"));
        let config = RegistryConfig::from_table(&table).unwrap();
        assert_eq!(
            config.entry("registry.example").unwrap().user.as_deref(),
            Some("me")
        );

        let refused = record_login(&path, "registry.example", "a:b", || {
            panic!("an invalid user must fail before the prompt")
        });
        assert!(refused.is_err());
    }
}
