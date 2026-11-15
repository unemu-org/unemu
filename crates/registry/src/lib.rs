// SPDX-License-Identifier: AGPL-3.0-or-later
//! Platform-neutral push and pull of machine images to OCI registries.
//!
//! Implements the client side of the OCI Distribution Specification 1.1
//! ("Pull", "Push", "Content discovery") for the artifacts of
//! [`image_store`], following `backlog/docs/design/machine-images.md` §5.1:
//!
//! - references resolve Docker-style against a default registry
//!   ([`mod@reference`]);
//! - a registries list without secrets comes from preferences ([`config`]),
//!   secrets from the environment or the host credential store
//!   ([`credentials`]), one store backend per operating system chosen at run
//!   time ([`credential_store`]);
//! - logging in, logging out and testing a connection are shared by the
//!   command line and the desktop shell, so a login in one is visible in the
//!   other ([`accounts`]);
//! - TLS is mandatory except for a loopback test registry; authentication is
//!   anonymous first, then RFC 7617 Basic ([`client`]);
//! - push uploads only missing blobs (monolithic when small, resumable
//!   chunked sessions when large), streams from the store, and publishes the
//!   manifest last under the tag rules ([`push`]);
//! - the push guard refuses user-supplied content outside private registries
//!   and anything but open content in the official namespace ([`guard`]);
//! - pull fetches the manifest, then the closure in parallel, verifies every
//!   SHA-256 through the store's streaming import, and sets the local tag by
//!   compare-and-swap only after the closure is complete ([`pull`]).
//!
//! The crate knows no emulator: platform providers supply the
//! [`image_store::ConfigValidator`] that checks a closure before it is tagged.

pub mod accounts;
pub mod client;
pub mod config;
pub mod credential_store;
pub mod credentials;
pub mod guard;
mod http;
pub mod pull;
pub mod push;
pub mod reference;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;

pub use client::Client;
pub use config::{RegistryConfig, RegistryEntry};
pub use reference::{Reference, DEFAULT_REGISTRY};

use image_store::{ConfigValidator, Digest, Manifest};

/// The provider whose artifact type and platform match `manifest`.
pub fn select_provider<'a>(
    providers: &[&'a dyn ConfigValidator],
    manifest: &Manifest,
) -> Result<&'a dyn ConfigValidator> {
    let platform = manifest.platform()?;
    providers
        .iter()
        .copied()
        .find(|p| p.artifact_type() == manifest.artifact_type() && p.platform() == platform)
        .ok_or_else(|| {
            Error::Protocol(format!(
                "no provider for {} on platform {platform}",
                manifest.artifact_type()
            ))
        })
}

/// Registry transfer failures. Messages never contain secrets.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] image_store::Error),
    #[error("invalid reference: {0}")]
    Reference(String),
    #[error("registry configuration: {0}")]
    Config(String),
    #[error("push refused: {0}")]
    Refused(#[from] guard::Refusal),
    #[error("network: {0}")]
    Transport(String),
    #[error("registry answered {status} to {method} {path}{}", .code.as_ref().map(|c| format!(" ({c})")).unwrap_or_default())]
    Status {
        status: u16,
        method: &'static str,
        path: String,
        code: Option<String>,
    },
    #[error("{host} requires authentication; log in or set the registry credentials")]
    AuthRequired { host: String },
    #[error("{host} rejected the credentials")]
    Unauthorized { host: String },
    #[error("{host} denied access to {path}")]
    Forbidden { host: String, path: String },
    #[error("unsupported registry authentication: {0}")]
    UnsupportedAuth(String),
    #[error("credential store: {0}")]
    Credentials(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("version tag {tag} already names {current}; publish {new} under a new version")]
    ImmutableTag {
        tag: String,
        current: Digest,
        new: Digest,
    },
    #[error("tag {tag} names {current}; moving it to {new} needs confirmation (--move)")]
    TagMoveNeedsConfirmation {
        tag: String,
        current: Digest,
        new: Digest,
    },
    #[error("tag {tag} changed on the registry during the push (now {current:?})")]
    RemoteTagChanged {
        tag: String,
        current: Option<Digest>,
    },
    #[error("local tag {tag} already names {current}, not {new}; pull under another --tag")]
    LocalTagConflict {
        tag: String,
        current: Digest,
        new: Digest,
    },
    #[error("registry content does not match its digest: expected {expected}, got {actual}")]
    DigestMismatch { expected: Digest, actual: Digest },
    #[error("invalid registry response: {0}")]
    Protocol(String),
    #[error("cancelled")]
    Cancelled,
}

/// Registry operations return typed errors; local store state stays consistent.
pub type Result<T> = std::result::Result<T, Error>;
