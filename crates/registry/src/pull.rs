// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pull an image into the local store; machine-images §§3.1 and 5.1 and OCI
//! Distribution Specification 1.1, "Pulling manifests" and "Pulling blobs".
//!
//! The manifest is fetched once (by tag or digest) and its exact bytes are
//! hashed. The provider's payload types bound every descriptor before any blob
//! is fetched. Missing blobs download in parallel, at most
//! [`PARALLEL_DOWNLOADS`] at a time, each streamed through the store's import,
//! which hashes the bytes in staging and publishes nothing unless the SHA-256
//! matches. A download cut off mid-blob continues with a `Range` request from
//! the bytes already staged, up to [`DOWNLOAD_RETRIES`] times; the staged
//! file and the running hash are kept, so the digest still covers every byte.
//! A registry that answers the range with the whole blob restarts the blob
//! from zero. The local tag is written by compare-and-swap only after the
//! store has verified the complete closure, and it records the registry it
//! came from ([`image_store::PULL_REGISTRY`]). Compressed payload types are not
//! supported yet, so only the stored bytes' digest is checked.
//!
//! A caller can cancel a pull ([`pull_cancellable`]): the check runs before
//! each blob, while a blob downloads and before the local tag is written, so
//! a cancelled pull never sets the tag. Blobs already verified stay in the
//! store, and the next pull fetches only the rest.

use crate::{client::BlobBody, Client, Error, Reference, Result};
use image_store::{media_type, ConfigValidator, Descriptor, Digest, Manifest, Store};
use std::{
    collections::BTreeSet,
    io::{self, Read},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
};

/// Concurrent blob downloads per pull.
pub const PARALLEL_DOWNLOADS: usize = 4;
/// Further requests per blob after an interrupted download.
pub const DOWNLOAD_RETRIES: u32 = 3;

/// What a running pull reports. Downloads run in parallel, so reports of
/// different blobs interleave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullProgress<'a> {
    /// `have` bytes of the blob are staged, counting earlier attempts.
    Receiving { blob: &'a Descriptor, have: u64 },
    /// The blob is verified in the store.
    Verified(&'a Descriptor),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PullOutcome {
    Pulled,
    /// The local tag already named this image.
    Unchanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullReport {
    pub local_tag: String,
    pub image: Descriptor,
    pub fetched: Vec<Descriptor>,
    pub present: Vec<Descriptor>,
    pub outcome: PullOutcome,
}

/// Pull `source` and tag it locally as `local_tag`, which is usually
/// [`Reference::local_tag`]: short from the family default registry,
/// host-qualified from any other.
pub fn pull(
    store: &Store,
    client: &Client,
    source: &Reference,
    local_tag: &str,
    providers: &[&dyn ConfigValidator],
    progress: &(dyn Fn(&Descriptor) + Sync),
) -> Result<PullReport> {
    pull_cancellable(
        store,
        client,
        source,
        local_tag,
        providers,
        &|event| {
            if let PullProgress::Verified(descriptor) = event {
                progress(descriptor);
            }
        },
        &|| false,
    )
}

/// [`pull`] with progress per blob and per received bytes, ending with
/// [`Error::Cancelled`] once `cancelled` answers true: it is asked before each
/// blob, while a blob downloads and before the local tag is written. Both
/// callbacks are called from the download threads too.
pub fn pull_cancellable(
    store: &Store,
    client: &Client,
    source: &Reference,
    local_tag: &str,
    providers: &[&dyn ConfigValidator],
    progress: &(dyn Fn(PullProgress<'_>) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<PullReport> {
    Store::validate_tag_name(local_tag)?;
    if source.host != client.host() {
        return Err(Error::Config("client and source registry differ".into()));
    }
    let repository = source.repository.as_str();
    let fetched = client
        .manifest(repository, &source.manifest_reference())?
        .ok_or_else(|| Error::NotFound(source.to_string()))?;
    if fetched
        .media_type
        .as_deref()
        .is_some_and(|t| t != media_type::MANIFEST)
    {
        return Err(Error::Protocol(format!(
            "unsupported manifest media type {}",
            fetched.media_type.unwrap_or_default()
        )));
    }
    let envelope = Manifest::decode_for_summary(&fetched.bytes)?;
    let validator = crate::select_provider(providers, &envelope)?;
    let manifest = Manifest::decode_with_types(&fetched.bytes, validator.payload_types())?;
    if manifest.artifact_type() != validator.artifact_type()
        || manifest.platform()? != validator.platform()
    {
        return Err(Error::Protocol(
            "manifest artifact type or platform does not match the provider".into(),
        ));
    }
    let image = Descriptor::new(
        media_type::MANIFEST,
        fetched.digest.clone(),
        fetched.bytes.len() as u64,
    );
    match store.tag_descriptor(local_tag) {
        Ok(current) if current.digest == image.digest => {
            return Ok(PullReport {
                local_tag: local_tag.into(),
                image,
                fetched: Vec::new(),
                present: Vec::new(),
                outcome: PullOutcome::Unchanged,
            })
        }
        Ok(current) => {
            return Err(Error::LocalTagConflict {
                tag: local_tag.into(),
                current: current.digest,
                new: image.digest,
            })
        }
        Err(image_store::Error::TagNotFound(_)) => {}
        Err(error) => return Err(error.into()),
    }
    let mut seen = BTreeSet::new();
    let (mut missing, mut present) = (Vec::new(), Vec::new());
    for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
        if !seen.insert(descriptor.digest.clone()) {
            continue;
        }
        match store.open_blob(&descriptor.digest, descriptor.size) {
            Ok(_) => present.push(descriptor.clone()),
            Err(image_store::Error::MissingBlob { .. }) => missing.push(descriptor.clone()),
            Err(error) => return Err(error.into()),
        }
    }
    // A download that failed because the cancel broke its connection is
    // reported as the cancel it is.
    let downloaded = download_all(store, client, repository, &missing, progress, cancelled);
    if cancelled() {
        return Err(Error::Cancelled);
    }
    downloaded?;
    store.import(fetched.bytes.as_slice(), Some(&image.digest))?;
    store.update_pulled_tag(local_tag, &image, None, validator, &source.host)?;
    Ok(PullReport {
        local_tag: local_tag.into(),
        image,
        fetched: missing,
        present,
        outcome: PullOutcome::Pulled,
    })
}

fn download_all(
    store: &Store,
    client: &Client,
    repository: &str,
    blobs: &[Descriptor],
    progress: &(dyn Fn(PullProgress<'_>) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<()> {
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let failure = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL_DOWNLOADS.min(blobs.len()) {
            scope.spawn(|| {
                while !stop.load(Ordering::SeqCst) {
                    if cancelled() {
                        stop.store(true, Ordering::SeqCst);
                        failure
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_or_insert(Error::Cancelled);
                        break;
                    }
                    let Some(descriptor) = blobs.get(next.fetch_add(1, Ordering::SeqCst)) else {
                        break;
                    };
                    match download(store, client, repository, descriptor, progress, cancelled) {
                        Ok(()) => progress(PullProgress::Verified(descriptor)),
                        Err(error) => {
                            stop.store(true, Ordering::SeqCst);
                            failure
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .get_or_insert(error);
                        }
                    }
                }
            });
        }
    });
    match failure.into_inner().unwrap_or_else(|e| e.into_inner()) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// One byte more than declared is read, so an oversized body fails the size
/// check instead of being silently truncated to a matching prefix.
fn download(
    store: &Store,
    client: &Client,
    repository: &str,
    descriptor: &Descriptor,
    progress: &(dyn Fn(PullProgress<'_>) + Sync),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<()> {
    let mut retries = DOWNLOAD_RETRIES;
    let mut body = None;
    loop {
        let mut source = ResumingBlob {
            client,
            repository,
            digest: &descriptor.digest,
            size: descriptor.size,
            reader: body.take(),
            have: 0,
            retries: &mut retries,
            failure: None,
            restart: None,
            blob: descriptor,
            progress,
            cancelled,
        };
        let imported = store.import(
            (&mut source).take(descriptor.size.saturating_add(1)),
            Some(&descriptor.digest),
        );
        let blob = match (imported, source.failure.take(), source.restart.take()) {
            (Ok(blob), _, _) => blob,
            (Err(_), _, Some(whole)) => {
                body = Some(whole);
                continue;
            }
            (Err(_), Some(failure), None) => return Err(failure),
            (Err(image_store::Error::DigestMismatch { expected, actual }), None, None) => {
                return Err(Error::DigestMismatch { expected, actual })
            }
            (Err(error), None, None) => return Err(error.into()),
        };
        if blob.size != descriptor.size {
            return Err(image_store::Error::SizeMismatch {
                expected: descriptor.size,
                actual: blob.size,
            }
            .into());
        }
        return Ok(());
    }
}

/// A blob body that reconnects after an interruption and continues at the
/// number of bytes already handed to the store's import. Registry failures are
/// kept in `failure`, so they are reported as such rather than as store I/O.
struct ResumingBlob<'a> {
    client: &'a Client,
    repository: &'a str,
    digest: &'a Digest,
    size: u64,
    reader: Option<Box<dyn Read + Send>>,
    have: u64,
    retries: &'a mut u32,
    failure: Option<Error>,
    /// The whole blob again, after the registry ignored a range request.
    restart: Option<Box<dyn Read + Send>>,
    blob: &'a Descriptor,
    progress: &'a (dyn Fn(PullProgress<'_>) + Sync),
    cancelled: &'a (dyn Fn() -> bool + Sync),
}

impl ResumingBlob<'_> {
    fn fail(&mut self, error: Error) -> io::Error {
        let message = error.to_string();
        self.failure = Some(error);
        io::Error::other(message)
    }
}

impl Read for ResumingBlob<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        // The staged file is dropped unpublished, so a cancelled blob leaves nothing.
        if (self.cancelled)() {
            return Err(self.fail(Error::Cancelled));
        }
        let mut interrupted = None;
        loop {
            if let Some(reader) = self.reader.as_mut() {
                match reader.read(buffer) {
                    Ok(0) if self.have < self.size && !buffer.is_empty() => {
                        interrupted = Some(Error::Transport("blob download ended early".into()));
                    }
                    Ok(count) => {
                        self.have += count as u64;
                        (self.progress)(PullProgress::Receiving {
                            blob: self.blob,
                            have: self.have,
                        });
                        return Ok(count);
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        interrupted =
                            Some(Error::Transport(crate::client::redact(&error.to_string())));
                    }
                }
                self.reader = None;
                if self.have >= self.size {
                    // Every declared byte arrived; the digest decides.
                    return Ok(0);
                }
            }
            let first = interrupted.is_none() && self.have == 0;
            if !first {
                if *self.retries == 0 {
                    let error = interrupted
                        .take()
                        .unwrap_or_else(|| Error::Transport("blob download failed".into()));
                    return Err(self.fail(error));
                }
                *self.retries -= 1;
            }
            match self
                .client
                .blob_reader_from(self.repository, self.digest, self.have, self.size)
            {
                Ok(BlobBody::Partial(reader)) => self.reader = Some(reader),
                Ok(BlobBody::Whole(reader)) if self.have == 0 => self.reader = Some(reader),
                Ok(BlobBody::Whole(reader)) => {
                    self.restart = Some(reader);
                    return Err(io::Error::other(
                        "registry ignored the range request; restarting the blob",
                    ));
                }
                Err(error @ Error::Transport(_)) => interrupted = Some(error),
                Err(error) => return Err(self.fail(error)),
            }
        }
    }
}

/// All tags of a repository, following `Link` pages of `page_size` names.
pub fn list_tags(client: &Client, repository: &str, page_size: usize) -> Result<Vec<String>> {
    collect(|next| client.tags_page(repository, page_size, next))
}

/// All repositories the registry lists for this client.
pub fn list_repositories(client: &Client, page_size: usize) -> Result<Vec<String>> {
    collect(|next| client.catalog_page(page_size, next))
}

fn collect(
    mut fetch: impl FnMut(Option<&str>) -> Result<crate::client::Page>,
) -> Result<Vec<String>> {
    const MAX_PAGES: usize = 10_000;
    let mut names = Vec::new();
    let mut next: Option<String> = None;
    let mut visited = BTreeSet::new();
    for _ in 0..MAX_PAGES {
        let page = fetch(next.as_deref())?;
        names.extend(page.names);
        match page.next {
            Some(url) if visited.insert(url.clone()) => next = Some(url),
            Some(_) => return Err(Error::Protocol("registry paging repeats a page".into())),
            None => return Ok(names),
        }
    }
    Err(Error::Protocol("registry paging does not end".into()))
}
