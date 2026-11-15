// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where registry passwords and tokens are kept: one credential-store backend
//! per operating system, chosen at run time; machine-images §5.1.
//!
//! [`detect`] picks the macOS Keychain when the system `security` tool is
//! present. Without a system store the [`SessionStore`] keeps secrets in
//! memory until the process exits, and scripts and CI pass credentials through
//! the `UNEMU_REGISTRY_*` environment variables instead
//! ([`crate::credentials`]). The Secret Service of Linux desktops and the
//! Windows Credential Manager are further [`CredentialStore`] backends for
//! [`detect`] to choose; they are not implemented yet.
//!
//! Items are keyed by registry host and user name, under the item name
//! `unemu-registry:<host>` ([`item_name`]). No backend writes a secret to a
//! file, a log, the preferences or an error message, and `Debug` output of
//! every type here leaves secrets out.

use crate::{Error, Result};
use std::{
    collections::BTreeMap,
    fmt,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

/// A password or access token. `Debug` never prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// The kind of store a backend is, for the user interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKind {
    /// The macOS Keychain, reached through the system `security` tool.
    Keychain,
    /// No system store: secrets live in memory until the process exits.
    Session,
}

impl StoreKind {
    /// Short name, as in "secret in the macOS Keychain".
    pub fn name(self) -> &'static str {
        match self {
            Self::Keychain => "macOS Keychain",
            Self::Session => "memory of this session only",
        }
    }

    /// Whether a secret survives the process, so another program sees it.
    pub fn persistent(self) -> bool {
        self == Self::Keychain
    }
}

/// A per-host secret store. A secret stored for one host and user is never
/// returned for another host or user.
pub trait CredentialStore: Send + Sync {
    fn kind(&self) -> StoreKind;

    fn read(&self, host: &str, user: &str) -> Result<Option<Secret>>;

    /// Whether [`CredentialStore::store`] would accept this secret, checked
    /// before a login reaches the registry so a refusal costs no request.
    fn check(&self, _host: &str, _user: &str, _secret: &Secret) -> Result<()> {
        Ok(())
    }

    /// Whether a secret is stored, without reading it where the backend can.
    fn contains(&self, host: &str, user: &str) -> Result<bool> {
        Ok(self.read(host, user)?.is_some())
    }

    /// Create or replace the item of `host` and `user`.
    fn store(&self, host: &str, user: &str, secret: &Secret) -> Result<()>;

    /// Remove the item; removing an absent item succeeds.
    fn delete(&self, host: &str, user: &str) -> Result<()>;
}

/// Name of a registry host's item in every backend.
pub fn item_name(host: &str) -> String {
    format!("unemu-registry:{host}")
}

/// The system `security` tool. A Keychain item it creates lists the tool in
/// its access list, so later reads through it never prompt; the Keychain API
/// called from a rebuilt, unsigned emulator binary would prompt after every
/// build.
pub const SECURITY_TOOL: &str = "/usr/bin/security";

/// The store of this operating system, chosen at run time: the macOS Keychain
/// when the `security` tool exists, the session store everywhere else.
pub fn detect() -> Arc<dyn CredentialStore> {
    if cfg!(target_os = "macos") && Path::new(SECURITY_TOOL).is_file() {
        Arc::new(KeychainStore::new())
    } else {
        Arc::new(SessionStore::default())
    }
}

/// Secrets in memory for the life of this process: the store used when the
/// operating system offers none. Nothing is written to disk.
#[derive(Default)]
pub struct SessionStore {
    items: Mutex<BTreeMap<(String, String), Secret>>,
}

impl SessionStore {
    fn items(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), Secret>> {
        self.items.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl fmt::Debug for SessionStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionStore")
            .field("items", &self.items().keys().collect::<Vec<_>>())
            .finish()
    }
}

impl CredentialStore for SessionStore {
    fn kind(&self) -> StoreKind {
        StoreKind::Session
    }

    fn read(&self, host: &str, user: &str) -> Result<Option<Secret>> {
        Ok(self.items().get(&(host.into(), user.into())).cloned())
    }

    fn store(&self, host: &str, user: &str, secret: &Secret) -> Result<()> {
        self.items()
            .insert((host.into(), user.into()), secret.clone());
        Ok(())
    }

    fn delete(&self, host: &str, user: &str) -> Result<()> {
        self.items().remove(&(host.to_owned(), user.to_owned()));
        Ok(())
    }
}

/// The macOS Keychain through the `security` tool, run as a subprocess.
///
/// The secret never appears in the tool's arguments, where the process list
/// would show it: it is read back on the tool's standard output and written
/// as a command line on the standard input of `security -i`. The tool's
/// diagnostics are discarded.
#[derive(Clone, Debug, Default)]
pub struct KeychainStore {
    /// One keychain file instead of the user's search list.
    keychain: Option<PathBuf>,
}

/// `errSecItemNotFound` as an exit status of the `security` tool.
const NOT_FOUND: i32 = 44;

/// `security -i` reads commands in lines of about 4 KiB and would run the
/// rest of a longer line as another command, so a command must stay shorter.
const MAX_COMMAND_LINE: usize = 4000;

/// The longest password or token the Keychain backend stores. Hex-encoded, it
/// leaves room in one `security -i` command line for the longest host and
/// user names.
pub const MAX_KEYCHAIN_SECRET: usize = 1536;

impl KeychainStore {
    /// The user's keychain search list, where new items go to the default keychain.
    pub fn new() -> Self {
        Self::default()
    }

    /// Only the keychain file at `path`, for an opt-in test with a throwaway keychain.
    pub fn with_keychain(path: impl Into<PathBuf>) -> Self {
        Self {
            keychain: Some(path.into()),
        }
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(SECURITY_TOOL);
        command.args(arguments);
        if let Some(path) = &self.keychain {
            command.arg(path);
        }
        command.stdin(Stdio::null()).stderr(Stdio::null());
        command
    }

    fn status_of(&self, arguments: &[&str]) -> Result<Option<i32>> {
        let status = self
            .command(arguments)
            .stdout(Stdio::null())
            .status()
            .map_err(tool_error)?;
        Ok(status.code())
    }

    /// The `security -i` command line that stores `secret`, hex-encoded.
    fn add_line(&self, host: &str, user: &str, secret: &Secret) -> Result<String> {
        // `find-generic-password -w` prints any other password as hexadecimal,
        // so a stored secret would not read back as itself.
        if secret.is_empty() || !secret.expose().bytes().all(|b| (0x20..0x7f).contains(&b)) {
            return Err(Error::Credentials(
                "the Keychain stores passwords and tokens of printable ASCII characters only"
                    .into(),
            ));
        }
        if secret.expose().len() > MAX_KEYCHAIN_SECRET {
            return Err(Error::Credentials(format!(
                "the Keychain stores passwords and tokens of at most {MAX_KEYCHAIN_SECRET} characters"
            )));
        }
        let hex: String = secret
            .expose()
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect();
        let mut line = format!(
            "add-generic-password -U -s {} -a {} -X {hex}",
            quoted(&item_name(host))?,
            quoted(user)?
        );
        if let Some(path) = &self.keychain {
            line.push(' ');
            line.push_str(&quoted(&path.to_string_lossy())?);
        }
        line.push('\n');
        if line.len() > MAX_COMMAND_LINE {
            return Err(Error::Credentials(
                "the password or token is too long for the Keychain tool".into(),
            ));
        }
        Ok(line)
    }
}

fn tool_error(error: std::io::Error) -> Error {
    Error::Credentials(format!("cannot run the security tool: {error}"))
}

fn refused(action: &str, code: Option<i32>) -> Error {
    Error::Credentials(match code {
        Some(code) => format!("the security tool could not {action} (exit status {code})"),
        None => format!("the security tool could not {action}"),
    })
}

/// A word of a `security -i` command line: double quotes, with backslash
/// escapes for quotes and backslashes. Control characters would end the line.
fn quoted(word: &str) -> Result<String> {
    if word.chars().any(char::is_control) {
        return Err(Error::Credentials(
            "keychain item names cannot contain control characters".into(),
        ));
    }
    let mut out = String::with_capacity(word.len() + 2);
    out.push('"');
    for c in word.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    Ok(out)
}

impl CredentialStore for KeychainStore {
    fn kind(&self) -> StoreKind {
        StoreKind::Keychain
    }

    fn read(&self, host: &str, user: &str) -> Result<Option<Secret>> {
        let output = self
            .command(&[
                "find-generic-password",
                "-s",
                &item_name(host),
                "-a",
                user,
                "-w",
            ])
            .output()
            .map_err(tool_error)?;
        match output.status.code() {
            Some(0) => {}
            Some(NOT_FOUND) => return Ok(None),
            code => return Err(refused("read the Keychain", code)),
        }
        let mut secret = String::from_utf8(output.stdout)
            .map_err(|_| Error::Credentials("stored secret is not UTF-8".into()))?;
        if secret.ends_with('\n') {
            secret.pop();
        }
        Ok((!secret.is_empty()).then(|| Secret::new(secret)))
    }

    fn check(&self, host: &str, user: &str, secret: &Secret) -> Result<()> {
        self.add_line(host, user, secret).map(|_| ())
    }

    fn contains(&self, host: &str, user: &str) -> Result<bool> {
        // Without `-w` the tool prints only the item's attributes.
        match self.status_of(&["find-generic-password", "-s", &item_name(host), "-a", user])? {
            Some(0) => Ok(true),
            Some(NOT_FOUND) => Ok(false),
            code => Err(refused("search the Keychain", code)),
        }
    }

    fn store(&self, host: &str, user: &str, secret: &Secret) -> Result<()> {
        let line = self.add_line(host, user, secret)?;
        let mut child = Command::new(SECURITY_TOOL)
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(tool_error)?;
        let written = child
            .stdin
            .take()
            .map(|mut input| input.write_all(line.as_bytes()));
        let status = child.wait().map_err(tool_error)?;
        match written {
            Some(Ok(())) if status.success() => Ok(()),
            _ => Err(refused("store the secret", status.code())),
        }
    }

    fn delete(&self, host: &str, user: &str) -> Result<()> {
        match self.status_of(&[
            "delete-generic-password",
            "-s",
            &item_name(host),
            "-a",
            user,
        ])? {
            Some(0 | NOT_FOUND) => Ok(()),
            code => Err(refused("delete the Keychain item", code)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_store_keeps_one_secret_per_host_and_user() {
        let store = SessionStore::default();
        assert_eq!(store.kind(), StoreKind::Session);
        assert!(!store.kind().persistent());
        let secret = Secret::new("synthetic-secret");
        store.store("a.example", "me", &secret).unwrap();
        store
            .store("b.example:5000", "me", &Secret::new("other-secret"))
            .unwrap();
        assert_eq!(store.read("a.example", "me").unwrap(), Some(secret.clone()));
        assert!(store.contains("a.example", "me").unwrap());
        for (host, user) in [
            ("a.example", "you"),
            ("a.example:5000", "me"),
            ("b.example", "me"),
            ("A.example", "me"),
        ] {
            assert_eq!(store.read(host, user).unwrap(), None, "{host} {user}");
            assert!(!store.contains(host, user).unwrap());
        }
        store
            .store("a.example", "me", &Secret::new("replaced"))
            .unwrap();
        assert_eq!(
            store.read("a.example", "me").unwrap(),
            Some(Secret::new("replaced"))
        );
        store.delete("a.example", "me").unwrap();
        store.delete("a.example", "me").unwrap();
        assert_eq!(store.read("a.example", "me").unwrap(), None);
        assert_eq!(
            store.read("b.example:5000", "me").unwrap(),
            Some(Secret::new("other-secret"))
        );
    }

    #[test]
    fn debug_output_of_secrets_and_stores_leaves_the_secret_out() {
        let store = SessionStore::default();
        store
            .store("a.example", "me", &Secret::new("not-printed"))
            .unwrap();
        for text in [
            format!("{:?}", Secret::new("not-printed")),
            format!("{store:?}"),
            format!("{:?}", store.read("a.example", "me").unwrap()),
        ] {
            assert!(!text.contains("not-printed"), "{text}");
        }
        assert!(format!("{store:?}").contains("a.example"));
    }

    #[test]
    fn the_keychain_command_line_carries_the_secret_hex_encoded_on_standard_input() {
        let store = KeychainStore::with_keychain("/synthetic/test.keychain-db");
        let line = store
            .add_line("nas.example:5000", "me \"x\"", &Secret::new("s3cret pass"))
            .unwrap();
        assert_eq!(
            line,
            "add-generic-password -U -s \"unemu-registry:nas.example:5000\" -a \"me \\\"x\\\"\" \
             -X 7333637265742070617373 \"/synthetic/test.keychain-db\"\n"
        );
        assert!(!line.contains("s3cret"));
        let fits = Secret::new("x".repeat(MAX_KEYCHAIN_SECRET));
        assert!(store.check("a.example", "me", &fits).is_ok());
        // The longest host and user names still fit one command line.
        let host = format!("{}.example:65535", "h".repeat(240));
        assert!(store.check(&host, &"\"".repeat(128), &fits).is_ok());
        let long = "x".repeat(MAX_KEYCHAIN_SECRET + 1);
        for refused in ["", "non-ascii-\u{142}", "tab\there", long.as_str()] {
            let secret = Secret::new(refused);
            assert!(
                store.check("a.example", "me", &secret).is_err(),
                "{refused}"
            );
        }
        assert!(store
            .add_line("a.example", "line\nbreak", &Secret::new("x"))
            .is_err());
    }

    /// Exercises the real `security` tool against a throwaway keychain file in
    /// the temporary directory, never the user's own keychains. Opt-in because
    /// creating a keychain briefly touches the user's keychain search list.
    #[test]
    #[ignore = "macOS Keychain: set UNEMU_KEYCHAIN_TEST=1 to run against a throwaway keychain"]
    fn keychain_backend_round_trip_in_a_throwaway_keychain() {
        if !cfg!(target_os = "macos") || std::env::var("UNEMU_KEYCHAIN_TEST").as_deref() != Ok("1")
        {
            eprintln!("skipped: UNEMU_KEYCHAIN_TEST is not 1");
            return;
        }
        let path = std::env::temp_dir().join(format!(
            "unemu-registry-test-{}.keychain-db",
            std::process::id()
        ));
        eprintln!(
            "throwaway keychain {}; if this test is interrupted, remove it with \
             `security delete-keychain <that path>`",
            path.display()
        );
        let tool = |args: &[&std::ffi::OsStr]| {
            Command::new(SECURITY_TOOL)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
        };
        assert!(tool(&[
            "create-keychain".as_ref(),
            "-p".as_ref(),
            "synthetic-keychain-password".as_ref(),
            path.as_os_str(),
        ])
        .success());
        let result = std::panic::catch_unwind(|| {
            let store = KeychainStore::with_keychain(&path);
            let host = "keychain-test.invalid:5000";
            assert!(!store.contains(host, "ci").unwrap());
            assert_eq!(store.read(host, "ci").unwrap(), None);
            store
                .store(host, "ci", &Secret::new("synthetic \"secret\" \\ 1"))
                .unwrap();
            assert!(store.contains(host, "ci").unwrap());
            assert!(!store.contains(host, "other").unwrap());
            assert_eq!(
                store.read(host, "ci").unwrap(),
                Some(Secret::new("synthetic \"secret\" \\ 1"))
            );
            store.store(host, "ci", &Secret::new("replaced")).unwrap();
            assert_eq!(
                store.read(host, "ci").unwrap(),
                Some(Secret::new("replaced"))
            );
            store.delete(host, "ci").unwrap();
            store.delete(host, "ci").unwrap();
            assert_eq!(store.read(host, "ci").unwrap(), None);
            // Quoting reaches the tool intact for an account name with a
            // space, a quote and a backslash.
            let user = "ci \"x\" \\ y";
            store.store(host, user, &Secret::new("synthetic")).unwrap();
            assert!(store.contains(host, user).unwrap());
            assert!(!store.contains(host, "ci").unwrap());
            assert_eq!(
                store.read(host, user).unwrap(),
                Some(Secret::new("synthetic"))
            );
            store.delete(host, user).unwrap();
            assert!(!store.contains(host, user).unwrap());
        });
        tool(&["delete-keychain".as_ref(), path.as_os_str()]);
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
