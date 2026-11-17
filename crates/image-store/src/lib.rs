// SPDX-License-Identifier: AGPL-3.0-or-later
//! Platform-neutral immutable machine artifacts and local tags.
//!
//! Implements OCI Image Layout 1.0.0 and OCI Image Manifest / Content Descriptors
//! 1.1: SHA-256 names exact stored bytes, not a decoded representation. See
//! `backlog/docs/design/machine-images.md` §§1–3 and §5; machines, the named
//! local instances of images, are §3.2. Platform providers own
//! config validation and ordered graph traversal; this crate owns no emulator,
//! registry, disk reconstruction, or GC.
//!
//! Store roots must be local filesystems with advisory locks, atomic publication
//! and directory fsync. Linux and macOS enforce locality; other hosts currently
//! return an unsupported-filesystem error. Power-loss durability relies on the
//! filesystem honoring fsync. Content digests never include local lock metadata.

mod artifact;
pub mod codec;
pub mod compat;
mod filesystem;
pub mod firmware;
pub mod library;
pub mod license;
pub mod location;
pub mod machine;
pub mod media_set;
pub mod metadata;
pub mod platform;
mod store;
pub mod summary;
pub mod workspace;

pub use artifact::{
    media_type, Descriptor, Digest, Manifest, PayloadType, MAX_OBJECTS, MAX_SMALL_BLOB,
};
#[cfg(feature = "test-support")]
pub use store::verification_passes;
pub use store::{
    Blob, BlobSource, ConfigValidator, ContentReader, ResolvedImage, Store, Tag, TagPage,
    MADE_HERE, PULL_REGISTRY,
};

/// Failures at the storage and untrusted artifact boundaries.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The operating system could not complete an operation.
    #[error("image store I/O: {0}")]
    Io(#[from] std::io::Error),
    /// A document is malformed, unsupported or violates a format invariant.
    #[error("invalid image: {0}")]
    Invalid(String),
    /// Missing stored bytes, with enough context to offer a repair action.
    #[error("missing {media_type} blob: {digest}")]
    MissingBlob { digest: Digest, media_type: String },
    /// Exact stored bytes did not match their content address.
    #[error("blob digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: Digest, actual: Digest },
    /// A blob does not have the length declared by its descriptor.
    #[error("blob size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    /// An update used an outdated tag value. The current value permits retry.
    #[error("tag conflict; current digest: {current:?}")]
    Conflict { current: Option<Digest> },
    /// A local edit raced another writer; rerun the edit against current data.
    #[error("local library changed during edit")]
    LibraryConflict,
    /// No such local tag exists.
    #[error("tag not found: {0}")]
    TagNotFound(String),
    /// The host cannot provide the required local storage guarantees.
    #[error("unsupported store filesystem: {0}")]
    UnsupportedFilesystem(String),
    /// The artifact declares a newer release than this build reads; nothing
    /// of it was decoded (machine-images §5.6). `required` is `None` when the
    /// declared value does not parse.
    #[error("{reference} {}", compat::needs_newer(.required, .level))]
    NeedsNewer {
        reference: String,
        required: Option<compat::AppVersion>,
        level: compat::AppVersion,
    },
}

/// Storage operations return typed errors without changing an existing blob.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
