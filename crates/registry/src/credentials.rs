// SPDX-License-Identifier: AGPL-3.0-or-later
//! Registry secrets from the environment or the host credential store.
//!
//! Secrets never enter preferences, the image store, logs or error messages.
//! On macOS the Keychain is reached only through the system `security` tool as
//! a subprocess: an item created by that tool lists it in the item's access
//! list, so later reads do not prompt, whereas the Keychain API called from a
//! rebuilt, unsigned binary would prompt again after every build. Items use the
//! service name `unemu-registry:<host>` and the user name as account.

use crate::{Error, Result, DEFAULT_REGISTRY};
use std::fmt;

/// Environment variables for CI and tests; both must be set to take effect.
pub const USER_ENV: &str = "UNEMU_REGISTRY_USER";
pub const PASSWORD_ENV: &str = "UNEMU_REGISTRY_PASSWORD";
/// The one registry host the environment pair belongs to; when unset, the
/// pair belongs to the family default registry only.
pub const HOST_ENV: &str = "UNEMU_REGISTRY_HOST";

/// A user name and secret. `Debug` never prints the secret.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub user: String,
    password: String,
}

impl Credentials {
    pub fn new(user: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            user: user.into(),
            password: password.into(),
        }
    }

    /// RFC 7617 §2: `Basic base64(user ":" password)`.
    pub(crate) fn basic_header(&self) -> String {
        format!(
            "Basic {}",
            base64(format!("{}:{}", self.user, self.password).as_bytes())
        )
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Supplies credentials for a registry host after it has asked for them.
pub trait CredentialSource: Send + Sync {
    fn credentials(&self, host: &str) -> Result<Option<Credentials>>;
}

/// Anonymous access only.
pub struct Anonymous;
impl CredentialSource for Anonymous {
    fn credentials(&self, _: &str) -> Result<Option<Credentials>> {
        Ok(None)
    }
}

impl CredentialSource for Credentials {
    fn credentials(&self, _: &str) -> Result<Option<Credentials>> {
        Ok(Some(self.clone()))
    }
}

/// The environment's user name and secret, bound to a single registry host so
/// that a challenge from any other host never receives them. `Debug` never
/// prints the secret.
#[derive(Clone, Default)]
pub struct EnvCredentials {
    host: String,
    user: Option<String>,
    password: Option<String>,
}

impl EnvCredentials {
    /// Read [`USER_ENV`], [`PASSWORD_ENV`] and [`HOST_ENV`].
    pub fn from_env() -> Self {
        let var = |name| std::env::var(name).ok().filter(|v| !v.is_empty());
        Self {
            host: var(HOST_ENV)
                .map(|h| h.to_ascii_lowercase())
                .unwrap_or_else(|| DEFAULT_REGISTRY.into()),
            user: var(USER_ENV),
            password: var(PASSWORD_ENV),
        }
    }

    pub fn new(host: &str, user: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            host: host.to_ascii_lowercase(),
            user: Some(user.into()),
            password: Some(password.into()),
        }
    }

    fn user_for(&self, host: &str) -> Option<&str> {
        (self.host == host)
            .then_some(self.user.as_deref())
            .flatten()
    }

    fn credentials_for(&self, host: &str) -> Option<Credentials> {
        match (self.user_for(host), &self.password) {
            (Some(user), Some(password)) => Some(Credentials::new(user, password.clone())),
            _ => None,
        }
    }
}

impl fmt::Debug for EnvCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvCredentials")
            .field("host", &self.host)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Environment first (for its own host only), then the host credential store
/// for the configured user.
pub struct HostCredentials {
    /// The user name from the registry entry, if one is configured.
    pub configured_user: Option<String>,
    pub env: EnvCredentials,
}

impl CredentialSource for HostCredentials {
    fn credentials(&self, host: &str) -> Result<Option<Credentials>> {
        if let Some(credentials) = self.env.credentials_for(host) {
            return Ok(Some(credentials));
        }
        let user = self
            .env
            .user_for(host)
            .map(str::to_owned)
            .or_else(|| self.configured_user.clone());
        match user {
            Some(user) => Ok(keychain::read(host, &user)?.map(|p| Credentials::new(user, p))),
            None => Ok(None),
        }
    }
}

/// The macOS Keychain through `/usr/bin/security`; other hosts have no store yet.
pub mod keychain {
    use super::*;

    /// Keychain service name of a registry host's item.
    pub fn service(host: &str) -> String {
        format!("unemu-registry:{host}")
    }

    /// Read a stored secret. A missing item is `None`; the secret never reaches
    /// logs because the tool's diagnostics are discarded.
    pub fn read(host: &str, user: &str) -> Result<Option<String>> {
        if !cfg!(target_os = "macos") {
            return Ok(None);
        }
        let output = std::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                &service(host),
                "-a",
                user,
                "-w",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .map_err(|e| Error::Credentials(format!("cannot run the security tool: {e}")))?;
        if !output.status.success() {
            return Ok(None);
        }
        let mut secret = String::from_utf8(output.stdout)
            .map_err(|_| Error::Credentials("stored secret is not UTF-8".into()))?;
        if secret.ends_with('\n') {
            secret.pop();
        }
        Ok((!secret.is_empty()).then_some(secret))
    }

    /// Create or update the item. With `-w` last, the tool itself prompts on the
    /// terminal, so the secret never passes through this process or its arguments.
    pub fn store_interactive(host: &str, user: &str) -> Result<()> {
        if !cfg!(target_os = "macos") {
            return Err(Error::Credentials(format!(
                "no credential store on this host; set {USER_ENV} and {PASSWORD_ENV}"
            )));
        }
        let status = std::process::Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-U",
                "-s",
                &service(host),
                "-a",
                user,
                "-w",
            ])
            .status()
            .map_err(|e| Error::Credentials(format!("cannot run the security tool: {e}")))?;
        if !status.success() {
            return Err(Error::Credentials(format!(
                "the security tool could not store the secret ({status})"
            )));
        }
        Ok(())
    }
}

/// RFC 4648 §4 base64 with padding.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_4648_test_vectors() {
        for (input, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), encoded);
        }
        assert_eq!(
            Credentials::new("Aladdin", "open sesame").basic_header(),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }

    #[test]
    fn debug_output_never_contains_the_secret() {
        let text = format!("{:?}", Credentials::new("ci", "not-printed"));
        assert!(text.contains("ci") && !text.contains("not-printed"));
        let text = format!(
            "{:?}",
            EnvCredentials::new("a.example", "ci", "not-printed")
        );
        assert!(!text.contains("not-printed"), "{text}");
    }

    #[test]
    fn the_environment_pair_answers_only_its_own_host() {
        let source = HostCredentials {
            configured_user: None,
            env: EnvCredentials::new("a.example:5000", "ci", "synthetic-secret"),
        };
        assert_eq!(
            source.credentials("a.example:5000").unwrap(),
            Some(Credentials::new("ci", "synthetic-secret"))
        );
        for other in ["b.example", "a.example", "a.example:5001", DEFAULT_REGISTRY] {
            assert_eq!(source.credentials(other).unwrap(), None, "{other}");
        }
        let unbound = EnvCredentials {
            host: DEFAULT_REGISTRY.into(),
            ..EnvCredentials::new("unused.example", "ci", "synthetic-secret")
        };
        assert!(unbound.credentials_for(DEFAULT_REGISTRY).is_some());
        assert!(unbound.credentials_for("b.example").is_none());
    }
}
