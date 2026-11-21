// SPDX-License-Identifier: AGPL-3.0-or-later
//! OCI Distribution Specification 1.1 client operations; machine-images §5.1.
//!
//! Requests start anonymous. A `401` with an RFC 7617 `Basic` challenge
//! (distribution-spec, "Authentication") is answered with configured
//! credentials, which are then sent with every later request to that host.
//! Credentials never leave the host unencrypted: they are sent over TLS, or
//! over plain HTTP only to a loopback test registry. Bearer-token realms are
//! parsed but not yet implemented; [`Challenge`] is the extension point.
//!
//! URLs returned by the registry (upload `Location`, paging `Link`, redirect
//! `Location`) are resolved against the request URL (RFC 3986 §5.2) and must
//! use HTTPS, or plain HTTP on a loopback test registry. Redirects are followed
//! here rather than by the HTTP library, so that `Authorization` is sent only
//! to the registry's own origin: a change of scheme, host or port drops it, as
//! blob downloads redirected to object storage expect. Error messages never
//! carry a URL's query string, which may hold a presigned signature.

use crate::{
    config::{is_loopback, RegistryEntry},
    credentials::CredentialSource,
    http::{Body, Method, Response, Timeouts, Transport},
    reference::validate_host,
    Error, Result,
};
use image_store::{media_type, Descriptor, Digest, MAX_SMALL_BLOB};
use std::{fs::File, io::Read, sync::Mutex};

/// A parsed `WWW-Authenticate` challenge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Challenge {
    Basic {
        realm: Option<String>,
    },
    Bearer {
        realm: String,
        service: Option<String>,
        scope: Option<String>,
    },
    Other(String),
}

impl Challenge {
    /// RFC 7235 §4.1 challenge with quoted or token parameter values.
    pub fn parse(header: &str) -> Self {
        let header = header.trim();
        let (scheme, params) = header.split_once(' ').unwrap_or((header, ""));
        let mut values = std::collections::BTreeMap::new();
        let mut rest = params.trim();
        while let Some((key, after)) = rest.split_once('=') {
            let key = key
                .trim()
                .trim_start_matches(',')
                .trim()
                .to_ascii_lowercase();
            let after = after.trim_start();
            let (value, tail) = if let Some(quoted) = after.strip_prefix('"') {
                let mut value = String::new();
                let mut chars = quoted.char_indices();
                let mut end = quoted.len();
                while let Some((i, c)) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some((_, escaped)) = chars.next() {
                                value.push(escaped);
                            }
                        }
                        '"' => {
                            end = i + 1;
                            break;
                        }
                        c => value.push(c),
                    }
                }
                (value, &quoted[end..])
            } else {
                let end = after.find(',').unwrap_or(after.len());
                (after[..end].trim().to_owned(), &after[end..])
            };
            values.insert(key, value);
            rest = tail.trim_start().trim_start_matches(',').trim_start();
        }
        match scheme.to_ascii_lowercase().as_str() {
            "basic" => Self::Basic {
                realm: values.remove("realm"),
            },
            "bearer" => match values.remove("realm") {
                Some(realm) => Self::Bearer {
                    realm,
                    service: values.remove("service"),
                    scope: values.remove("scope"),
                },
                None => Self::Other("Bearer".into()),
            },
            other => Self::Other(other.into()),
        }
    }
}

/// Outcome of opening an upload session.
#[derive(Debug, PartialEq, Eq)]
pub enum Started {
    /// The registry linked an existing blob from another repository.
    Mounted,
    /// A session URL for `PATCH` and the final `PUT`.
    Session(String),
}

/// Outcome of one `PATCH`.
#[derive(Debug)]
pub enum Chunk {
    /// The registry holds `offset` bytes; continue at the returned location.
    Accepted { location: String, offset: u64 },
    /// The session is gone; a new upload must start from zero.
    SessionLost,
    /// The registry refused the chunk's start offset although it was the one
    /// the registry reported. Its offset reports cannot be trusted for this
    /// session (some registries report an empty session and a one-byte
    /// session alike), so the upload starts again in a new session.
    RangeRejected,
    /// The chunk was not accepted; ask the registry for its offset.
    Interrupted(Error),
}

/// One page of names and the URL of the next page, if any.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Page {
    pub names: Vec<String>,
    pub next: Option<String>,
}

/// What the API check `GET /v2/` answers without credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiAccess {
    /// `200`: the registry serves requests without login.
    Anonymous,
    /// `401` with a Basic challenge: requests need a login.
    BasicLogin,
    /// `401` with a Bearer challenge: a token service, not implemented here.
    TokenService,
    /// `401` with another challenge scheme, or none.
    Unsupported(String),
}

/// Result of an anonymous access probe.
#[derive(Debug, PartialEq, Eq)]
pub enum AnonymousAccess {
    Denied,
    Readable,
    /// The registry answered with a token challenge. Token services send that
    /// for public repositories too, so the answer shows nothing about privacy.
    Inconclusive,
}

/// A blob body opened at an offset.
pub enum BlobBody {
    /// `206` whose `Content-Range` starts at the requested offset.
    Partial(Box<dyn Read + Send>),
    /// `200`: the registry ignored the range and sends the whole blob.
    Whole(Box<dyn Read + Send>),
}

/// A manifest as fetched, before any decoding.
#[derive(Debug)]
pub struct FetchedManifest {
    pub bytes: Vec<u8>,
    pub media_type: Option<String>,
    pub digest: Digest,
}

/// A client for one registry host. Safe to share between transfer threads.
pub struct Client {
    host: String,
    base: String,
    origin: Origin,
    tls: bool,
    confidential: bool,
    transport: Box<dyn Transport>,
    credentials: Box<dyn CredentialSource>,
    authorization: Mutex<Option<String>>,
    /// The challenge of the last `401` this registry answered.
    challenge: Mutex<Option<Challenge>>,
}

/// The OCI image manifest, the only manifest type this client decodes.
const ACCEPT_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
/// Every manifest and index type a tag may name: OCI image manifest and image
/// index (OCI Image Format 1.1, "Media Types"), Docker Image Manifest V2
/// Schema 2 and its manifest list. A registry may answer `404` for a tag whose
/// manifest type the `Accept` header leaves out, or substitute one platform's
/// manifest for a list, so asking which digest a tag names accepts them all.
pub(crate) const ACCEPT_ANY_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json, \
     application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json, \
     application/vnd.docker.distribution.manifest.list.v2+json";
const MAX_REDIRECTS: usize = 5;

impl Client {
    /// Connect policy: TLS for every host; plain HTTP only with the entry's
    /// loopback opt-in. Nothing is sent until the first request.
    pub fn new(entry: &RegistryEntry, credentials: Box<dyn CredentialSource>) -> Result<Self> {
        Self::with_timeouts(entry, credentials, Timeouts::TRANSFER)
    }

    /// A client for logins and connection tests: the same policy with short
    /// timeouts (10 s to connect, 15 s per read), so an unreachable registry
    /// answers a waiting person quickly.
    pub fn for_probes(
        entry: &RegistryEntry,
        credentials: Box<dyn CredentialSource>,
    ) -> Result<Self> {
        Self::with_timeouts(entry, credentials, Timeouts::PROBE)
    }

    fn with_timeouts(
        entry: &RegistryEntry,
        credentials: Box<dyn CredentialSource>,
        timeouts: Timeouts,
    ) -> Result<Self> {
        validate_host(&entry.host)?;
        let loopback = is_loopback(&entry.host);
        if entry.plain_http && !loopback {
            return Err(Error::Config(format!(
                "plain HTTP is only allowed for a loopback registry, not {}",
                entry.host
            )));
        }
        let scheme = if entry.plain_http { "http" } else { "https" };
        let base = format!("{scheme}://{}", entry.host);
        Ok(Self {
            host: entry.host.clone(),
            origin: Origin::of(&base)
                .ok_or_else(|| Error::Config(format!("invalid registry host {}", entry.host)))?,
            base,
            tls: !entry.plain_http,
            confidential: !entry.plain_http || loopback,
            transport: crate::http::transport(!entry.plain_http, timeouts)?,
            credentials,
            authorization: Mutex::new(None),
            challenge: Mutex::new(None),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test_transport(host: &str, transport: Box<dyn Transport>) -> Self {
        let base = format!("https://{host}");
        Self {
            host: host.into(),
            origin: Origin::of(&base).unwrap(),
            base,
            tls: true,
            confidential: true,
            transport,
            credentials: Box::new(crate::credentials::Anonymous),
            authorization: Mutex::new(None),
            challenge: Mutex::new(None),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// A path on this registry, or an absolute URL this client may use.
    fn url(&self, target: &str) -> Result<String> {
        if target.starts_with('/') {
            return Ok(format!("{}{target}", self.base));
        }
        self.permitted(target)?;
        Ok(target.to_owned())
    }

    /// Resolve a URL the registry returned against the request it answered.
    fn locate(&self, request: &str, value: &str) -> Result<String> {
        let url = resolve(request, value)
            .ok_or_else(|| Error::Protocol("registry returned an invalid URL".into()))?;
        self.permitted(&url)?;
        Ok(url)
    }

    /// HTTPS anywhere for a TLS registry; a loopback test registry stays on
    /// plain HTTP and loopback, because its transport has no TLS settings.
    fn permitted(&self, url: &str) -> Result<()> {
        let allowed = Origin::of(url).is_some_and(|origin| {
            if self.tls {
                origin.scheme == "https"
            } else {
                origin.scheme == "http" && is_loopback(&origin.authority)
            }
        });
        if !allowed {
            return Err(Error::Protocol(format!(
                "registry pointed to {}; only HTTPS, or plain HTTP on loopback, is used",
                without_query(url)
            )));
        }
        Ok(())
    }

    fn same_origin(&self, url: &str) -> bool {
        Origin::of(url).as_ref() == Some(&self.origin)
    }

    /// The challenge of the last `401` this registry answered, so that a
    /// request ending in [`Error::AuthRequired`] or [`Error::Unauthorized`]
    /// can say which login the registry asks for (RFC 7235 §4.1).
    pub fn last_challenge(&self) -> Option<Challenge> {
        self.challenge
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn current_authorization(&self) -> Option<String> {
        self.authorization
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Send with the established authorization, answering one challenge.
    pub(crate) fn send(
        &self,
        method: Method,
        target: &str,
        headers: &[(&str, String)],
        body: &Body<'_>,
    ) -> Result<Response> {
        let url = self.url(target)?;
        let mut sent = self.current_authorization();
        loop {
            let response = self.exchange(method, &url, headers, body, sent.as_deref())?;
            if response.status != 401 || !self.same_origin(&response.url) {
                return Ok(response);
            }
            let challenge = response
                .header("www-authenticate")
                .map(Challenge::parse)
                .unwrap_or(Challenge::Other("none".into()));
            *self.challenge.lock().unwrap_or_else(|e| e.into_inner()) = Some(challenge.clone());
            if sent.is_some() {
                return Err(Error::Unauthorized {
                    host: self.host.clone(),
                });
            }
            sent = Some(self.authorize(&challenge)?);
        }
    }

    /// One request, following redirects. `authorization` goes only to this
    /// registry's origin; `303` turns the request into a body-less `GET`.
    fn exchange(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, String)],
        body: &Body<'_>,
        authorization: Option<&str>,
    ) -> Result<Response> {
        let (mut method, mut url) = (method, url.to_owned());
        let mut headers = headers.to_vec();
        let mut body = body;
        for _ in 0..=MAX_REDIRECTS {
            let mut all = headers.clone();
            if let Some(value) = authorization.filter(|_| self.same_origin(&url)) {
                all.push(("Authorization", value.to_owned()));
            }
            let response = self
                .transport
                .send(method, &url, &all, body)
                .map_err(|e| Error::Transport(redact(&e.to_string())))?;
            if !matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                return Ok(response);
            }
            let location = response
                .header("location")
                .ok_or_else(|| Error::Protocol("redirect without Location".into()))?;
            url = self.locate(&response.url, location)?;
            if response.status == 303 && method != Method::Head {
                method = Method::Get;
                body = &Body::Empty;
                headers.retain(|(name, _)| name.eq_ignore_ascii_case("accept"));
            }
        }
        Err(Error::Protocol("registry redirected too many times".into()))
    }

    fn authorize(&self, challenge: &Challenge) -> Result<String> {
        let mut established = self.authorization.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(value) = established.as_ref() {
            return Ok(value.clone());
        }
        match challenge {
            Challenge::Basic { .. } => {}
            Challenge::Bearer { .. } => {
                return Err(Error::UnsupportedAuth(
                    "bearer token service; only Basic authentication is implemented".into(),
                ))
            }
            Challenge::Other(scheme) => {
                return Err(Error::UnsupportedAuth(format!("scheme {scheme}")))
            }
        }
        if !self.confidential {
            return Err(Error::Config(
                "credentials are only sent over TLS or to a loopback registry".into(),
            ));
        }
        let credentials =
            self.credentials
                .credentials(&self.host)?
                .ok_or_else(|| Error::AuthRequired {
                    host: self.host.clone(),
                })?;
        let value = credentials.basic_header();
        *established = Some(value.clone());
        Ok(value)
    }

    /// Send Basic credentials from the next request on, when the source has
    /// them. Registries answer an anonymous catalog request with the anonymous
    /// view rather than a challenge, so a full listing cannot wait for a `401`.
    pub fn use_credentials_now(&self) -> Result<bool> {
        let mut established = self.authorization.lock().unwrap_or_else(|e| e.into_inner());
        if established.is_some() {
            return Ok(true);
        }
        if !self.confidential {
            return Ok(false);
        }
        match self.credentials.credentials(&self.host)? {
            Some(credentials) => {
                *established = Some(credentials.basic_header());
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn status_error(&self, method: Method, target: &str, response: Response) -> Error {
        let status = response.status;
        let code = response
            .bytes(64 * 1024)
            .ok()
            .and_then(|body| serde_json::from_slice::<serde_json::Value>(&body).ok())
            .and_then(|v| v["errors"][0]["code"].as_str().map(str::to_owned));
        let path = without_query(target);
        let path = path.strip_prefix(&self.base).unwrap_or(path).to_owned();
        match status {
            403 => Error::Forbidden {
                host: self.host.clone(),
                path,
            },
            _ => Error::Status {
                status,
                method: method.as_str(),
                path,
                code,
            },
        }
    }

    /// `HEAD` a blob. A present blob must have the descriptor's length.
    pub fn blob_exists(&self, repository: &str, descriptor: &Descriptor) -> Result<bool> {
        let target = format!("/v2/{repository}/blobs/{}", descriptor.digest);
        let response = self.send(Method::Head, &target, &[], &Body::Empty)?;
        match response.status {
            200 => match response.content_length() {
                Some(size) if size != descriptor.size => Err(Error::Protocol(format!(
                    "registry blob {} has {size} bytes, expected {}",
                    descriptor.digest, descriptor.size
                ))),
                _ => Ok(true),
            },
            404 => Ok(false),
            _ => Err(self.status_error(Method::Head, &target, response)),
        }
    }

    /// Stream a blob's bytes. The caller verifies length and digest.
    pub fn blob_reader(&self, repository: &str, digest: &Digest) -> Result<Box<dyn Read + Send>> {
        let target = format!("/v2/{repository}/blobs/{digest}");
        let response = self.send(Method::Get, &target, &[], &Body::Empty)?;
        match response.status {
            200 => Ok(response.into_reader()),
            _ => Err(self.status_error(Method::Get, &target, response)),
        }
    }

    /// Read a small blob, such as a config record, into memory: at most
    /// [`MAX_SMALL_BLOB`] bytes, exactly the descriptor's length and digest.
    /// Nothing reaches the store.
    pub fn small_blob(&self, repository: &str, descriptor: &Descriptor) -> Result<Vec<u8>> {
        if descriptor.size > MAX_SMALL_BLOB {
            return Err(Error::Protocol(format!(
                "blob {} has {} bytes, more than a small record",
                descriptor.digest, descriptor.size
            )));
        }
        let target = format!("/v2/{repository}/blobs/{}", descriptor.digest);
        let response = self.send(Method::Get, &target, &[], &Body::Empty)?;
        if response.status != 200 {
            return Err(self.status_error(Method::Get, &target, response));
        }
        let bytes = response.bytes(descriptor.size)?;
        if bytes.len() as u64 != descriptor.size {
            return Err(Error::Protocol(format!(
                "registry blob {} has {} bytes, expected {}",
                descriptor.digest,
                bytes.len(),
                descriptor.size
            )));
        }
        let actual = Digest::of(&bytes);
        if actual != descriptor.digest {
            return Err(Error::DigestMismatch {
                expected: descriptor.digest.clone(),
                actual,
            });
        }
        Ok(bytes)
    }

    /// Continue a blob download at `offset` with `Range: bytes=<offset>-`
    /// (RFC 9110 §14). A `206` must carry a `Content-Range` from `offset` to
    /// the last byte of a `size`-byte blob; a `200` sends the whole blob again.
    pub fn blob_reader_from(
        &self,
        repository: &str,
        digest: &Digest,
        offset: u64,
        size: u64,
    ) -> Result<BlobBody> {
        if offset == 0 {
            return self.blob_reader(repository, digest).map(BlobBody::Whole);
        }
        let target = format!("/v2/{repository}/blobs/{digest}");
        let range = [("Range", format!("bytes={offset}-"))];
        let response = self.send(Method::Get, &target, &range, &Body::Empty)?;
        match response.status {
            200 => Ok(BlobBody::Whole(response.into_reader())),
            206 if content_range_matches(response.header("content-range"), offset, size) => {
                Ok(BlobBody::Partial(response.into_reader()))
            }
            206 => Err(Error::Protocol(format!(
                "registry answered a range request for {digest} with another range"
            ))),
            _ => Err(self.status_error(Method::Get, &target, response)),
        }
    }

    /// The digest a manifest reference names, or `None` when absent. Any OCI
    /// or Docker manifest or index type counts, so a tag that names an image
    /// index or a Docker manifest list is present: push decides from this
    /// whether a tag is new.
    pub fn manifest_digest(&self, repository: &str, reference: &str) -> Result<Option<Digest>> {
        let target = format!("/v2/{repository}/manifests/{reference}");
        let accept = [("Accept", ACCEPT_ANY_MANIFEST.to_owned())];
        let response = self.send(Method::Head, &target, &accept, &Body::Empty)?;
        match response.status {
            200 => match response.header("docker-content-digest") {
                Some(value) => value
                    .parse()
                    .map(Some)
                    .map_err(|_| Error::Protocol("invalid Docker-Content-Digest".into())),
                None => Ok(self
                    .fetch_manifest(repository, reference, ACCEPT_ANY_MANIFEST)?
                    .map(|m| m.digest)),
            },
            404 => Ok(None),
            _ => Err(self.status_error(Method::Head, &target, response)),
        }
    }

    /// Fetch an OCI image manifest of at most 1 MiB and hash its exact bytes.
    /// A digest reference or a `Docker-Content-Digest` header must match those
    /// bytes. Only the OCI image manifest type is accepted, so a reference that
    /// names another manifest or index type may be `None`.
    pub fn manifest(&self, repository: &str, reference: &str) -> Result<Option<FetchedManifest>> {
        self.fetch_manifest(repository, reference, ACCEPT_MANIFEST)
    }

    fn fetch_manifest(
        &self,
        repository: &str,
        reference: &str,
        accept: &str,
    ) -> Result<Option<FetchedManifest>> {
        let target = format!("/v2/{repository}/manifests/{reference}");
        let accept = [("Accept", accept.to_owned())];
        let response = self.send(Method::Get, &target, &accept, &Body::Empty)?;
        match response.status {
            200 => {}
            404 => return Ok(None),
            _ => return Err(self.status_error(Method::Get, &target, response)),
        }
        let media_type = response
            .header("content-type")
            .map(|v| v.split(';').next().unwrap_or(v).trim().to_owned());
        let claimed = response.header("docker-content-digest").map(str::to_owned);
        let bytes = response.bytes(MAX_SMALL_BLOB)?;
        let digest = Digest::of(&bytes);
        for expected in [Some(reference), claimed.as_deref()]
            .into_iter()
            .flatten()
            .filter(|r| r.starts_with("sha256:"))
        {
            let expected: Digest = expected
                .parse()
                .map_err(|_| Error::Protocol("invalid manifest digest".into()))?;
            if expected != digest {
                return Err(Error::DigestMismatch {
                    expected,
                    actual: digest,
                });
            }
        }
        Ok(Some(FetchedManifest {
            bytes,
            media_type,
            digest,
        }))
    }

    /// `PUT` a manifest under a tag or its digest; returns the registry's digest, if reported.
    pub fn put_manifest(&self, repository: &str, reference: &str, bytes: &[u8]) -> Result<()> {
        let target = format!("/v2/{repository}/manifests/{reference}");
        let headers = [("Content-Type", media_type::MANIFEST.to_owned())];
        let response = self.send(Method::Put, &target, &headers, &Body::Bytes(bytes))?;
        match response.status {
            201 => {
                let expected = Digest::of(bytes);
                if let Some(reported) = response.header("docker-content-digest") {
                    if reported != expected.to_string() {
                        return Err(Error::Protocol(format!(
                            "registry stored the manifest as {reported}, expected {expected}"
                        )));
                    }
                }
                Ok(())
            }
            _ => Err(self.status_error(Method::Put, &target, response)),
        }
    }

    /// Delete a manifest by digest, with its tags.
    pub fn delete_manifest(&self, repository: &str, digest: &Digest) -> Result<()> {
        let target = format!("/v2/{repository}/manifests/{digest}");
        let response = self.send(Method::Delete, &target, &[], &Body::Empty)?;
        match response.status {
            200 | 202 | 204 => Ok(()),
            _ => Err(self.status_error(Method::Delete, &target, response)),
        }
    }

    /// Delete a blob from a repository; registries may refuse blob deletion.
    pub fn delete_blob(&self, repository: &str, digest: &Digest) -> Result<()> {
        let target = format!("/v2/{repository}/blobs/{digest}");
        let response = self.send(Method::Delete, &target, &[], &Body::Empty)?;
        match response.status {
            200 | 202 | 204 => Ok(()),
            _ => Err(self.status_error(Method::Delete, &target, response)),
        }
    }

    /// Open an upload session, first asking to mount `digest` from `from`.
    pub fn start_upload(
        &self,
        repository: &str,
        mount: Option<(&Digest, &str)>,
    ) -> Result<Started> {
        let mut target = format!("/v2/{repository}/blobs/uploads/");
        if let Some((digest, from)) = mount {
            target.push_str(&format!("?mount={}&from={from}", query_digest(digest)));
        }
        let headers = [("Content-Length", "0".to_owned())];
        let response = self.send(Method::Post, &target, &headers, &Body::Empty)?;
        match response.status {
            201 if mount.is_some() => Ok(Started::Mounted),
            202 => {
                let location = response
                    .header("location")
                    .ok_or_else(|| Error::Protocol("upload session without Location".into()))?;
                Ok(Started::Session(self.locate(&response.url, location)?))
            }
            _ => Err(self.status_error(Method::Post, &target, response)),
        }
    }

    /// The number of bytes a session holds, or `None` when the session is gone.
    pub fn upload_offset(&self, location: &str) -> Result<Option<u64>> {
        let response = self.send(Method::Get, location, &[], &Body::Empty)?;
        match response.status {
            204 | 202 | 200 => Ok(Some(parse_range(response.header("range"))?.unwrap_or(0))),
            404 => Ok(None),
            _ => Err(self.status_error(Method::Get, location, response)),
        }
    }

    /// Send `len` bytes of `file` from `offset` as one chunk.
    pub fn upload_chunk(
        &self,
        location: &str,
        file: &File,
        offset: u64,
        len: u64,
    ) -> Result<Chunk> {
        let headers = [
            ("Content-Type", "application/octet-stream".to_owned()),
            ("Content-Range", format!("{offset}-{}", offset + len - 1)),
            ("Content-Length", len.to_string()),
        ];
        let body = Body::File { file, offset, len };
        let response = match self.send(Method::Patch, location, &headers, &body) {
            Ok(response) => response,
            Err(error @ Error::Transport(_)) => return Ok(Chunk::Interrupted(error)),
            Err(error) => return Err(error),
        };
        match response.status {
            202 => {
                let next = match response.header("location") {
                    Some(next) => self.locate(&response.url, next)?,
                    None => location.to_owned(),
                };
                let offset = parse_range(response.header("range"))?.unwrap_or(offset + len);
                Ok(Chunk::Accepted {
                    location: next,
                    offset,
                })
            }
            404 => Ok(Chunk::SessionLost),
            416 => Ok(Chunk::RangeRejected),
            status if status >= 500 => Ok(Chunk::Interrupted(self.status_error(
                Method::Patch,
                location,
                response,
            ))),
            _ => Err(self.status_error(Method::Patch, location, response)),
        }
    }

    /// Close a session with `PUT ?digest=`, carrying any final bytes.
    pub(crate) fn finish_upload(
        &self,
        location: &str,
        digest: &Digest,
        body: &Body<'_>,
    ) -> Result<()> {
        let separator = if location.contains('?') { '&' } else { '?' };
        let target = format!("{location}{separator}digest={}", query_digest(digest));
        let mut headers = vec![("Content-Length", body.len().to_string())];
        if body.len() > 0 {
            headers.push(("Content-Type", "application/octet-stream".to_owned()));
        }
        let response = self.send(Method::Put, &target, &headers, body)?;
        match response.status {
            201 | 204 => Ok(()),
            _ => Err(self.status_error(Method::Put, &target, response)),
        }
    }

    /// One `tags/list` page; `next` is a URL from an earlier page's `Link`.
    pub fn tags_page(&self, repository: &str, n: usize, next: Option<&str>) -> Result<Page> {
        let first = format!("/v2/{repository}/tags/list?n={n}");
        self.page(next.unwrap_or(&first), "tags")
    }

    /// One `_catalog` page; `next` is a URL from an earlier page's `Link`.
    pub fn catalog_page(&self, n: usize, next: Option<&str>) -> Result<Page> {
        let first = format!("/v2/_catalog?n={n}");
        self.page(next.unwrap_or(&first), "repositories")
    }

    fn page(&self, target: &str, field: &str) -> Result<Page> {
        let response = self.send(Method::Get, target, &[], &Body::Empty)?;
        if response.status != 200 {
            return Err(self.status_error(Method::Get, target, response));
        }
        let next = response
            .header("link")
            .and_then(next_link)
            .map(|link| self.locate(&response.url, &link))
            .transpose()?;
        let body = response.bytes(16 * MAX_SMALL_BLOB)?;
        let value: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| Error::Protocol(e.to_string()))?;
        let names = match &value[field] {
            serde_json::Value::Null => Vec::new(),
            serde_json::Value::Array(items) => items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| Error::Protocol(format!("{field} must be strings")))
                })
                .collect::<Result<_>>()?,
            _ => return Err(Error::Protocol(format!("missing {field} list"))),
        };
        Ok(Page { names, next })
    }

    /// distribution-spec "Determining Support" (`GET /v2/`) without any
    /// credentials, as a first contact with the registry.
    pub fn probe_api(&self) -> Result<ApiAccess> {
        let response = self.exchange(Method::Get, &self.url("/v2/")?, &[], &Body::Empty, None)?;
        match response.status {
            200 => Ok(ApiAccess::Anonymous),
            401 => Ok(
                match response.header("www-authenticate").map(Challenge::parse) {
                    Some(Challenge::Basic { .. }) => ApiAccess::BasicLogin,
                    Some(Challenge::Bearer { .. }) => ApiAccess::TokenService,
                    Some(Challenge::Other(scheme)) => ApiAccess::Unsupported(scheme),
                    None => ApiAccess::Unsupported("none".into()),
                },
            ),
            _ => Err(self.status_error(Method::Get, "/v2/", response)),
        }
    }

    /// The same check with the source's Basic credentials sent from the first
    /// request. `Ok(false)` when the source has none; credentials the registry
    /// refuses are [`Error::Unauthorized`].
    pub fn check_login(&self) -> Result<bool> {
        if !self.use_credentials_now()? {
            return Ok(false);
        }
        let response = self.send(Method::Get, "/v2/", &[], &Body::Empty)?;
        match response.status {
            200 => Ok(true),
            _ => Err(self.status_error(Method::Get, "/v2/", response)),
        }
    }

    /// Probe anonymous reads of `repository` and of its namespace in the
    /// catalog, without the established credentials. `403`, or `401` with a
    /// Basic challenge, denies; `200` or `404` mean the repository is
    /// anonymously visible. A `401` with any other challenge is inconclusive:
    /// token services answer anonymous requests for public repositories that
    /// way too. Any other answer is an error, so a restricted push fails closed.
    pub fn anonymous_access(&self, repository: &str) -> Result<AnonymousAccess> {
        let target = format!("/v2/{repository}/tags/list?n=1");
        let response = self.exchange(Method::Get, &self.url(&target)?, &[], &Body::Empty, None)?;
        match response.status {
            403 => {}
            401 => match response.header("www-authenticate").map(Challenge::parse) {
                Some(Challenge::Basic { .. }) => {}
                _ => return Ok(AnonymousAccess::Inconclusive),
            },
            200 | 404 => return Ok(AnonymousAccess::Readable),
            _ => return Err(self.status_error(Method::Get, &target, response)),
        }
        let namespace = repository.split('/').next().unwrap_or(repository);
        let target = format!("/v2/_catalog?n=1&last={namespace}/");
        let response = self.exchange(Method::Get, &self.url(&target)?, &[], &Body::Empty, None)?;
        if response.status == 200 {
            let body = response.bytes(MAX_SMALL_BLOB)?;
            let visible = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["repositories"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|v| v.as_str())
                .any(|name| name.starts_with(&format!("{namespace}/")));
            if visible {
                return Ok(AnonymousAccess::Readable);
            }
        }
        Ok(AnonymousAccess::Denied)
    }
}

fn query_digest(digest: &Digest) -> String {
    digest.to_string().replace(':', "%3A")
}

/// Distribution-spec upload `Range: 0-<last>` (inclusive), so `0-0` is a
/// session holding one byte. zot reports an empty session as `0--1`.
fn parse_range(header: Option<&str>) -> Result<Option<u64>> {
    let Some(value) = header else {
        return Ok(None);
    };
    let value = value.trim().trim_start_matches("bytes=");
    if value == "0--1" {
        return Ok(Some(0));
    }
    let (start, end) = value
        .split_once('-')
        .ok_or_else(|| Error::Protocol("invalid upload Range".into()))?;
    let end: u64 = end
        .trim()
        .parse()
        .map_err(|_| Error::Protocol("invalid upload Range".into()))?;
    if start.trim() != "0" {
        return Err(Error::Protocol("upload Range must start at 0".into()));
    }
    end.checked_add(1)
        .map(Some)
        .ok_or_else(|| Error::Protocol("invalid upload Range".into()))
}

/// RFC 9110 §14.4 `Content-Range: bytes <first>-<last>/<complete>` covering
/// exactly `offset..size`.
fn content_range_matches(header: Option<&str>, offset: u64, size: u64) -> bool {
    let Some(range) = header.and_then(|h| h.trim().strip_prefix("bytes ")) else {
        return false;
    };
    let Some((span, complete)) = range.split_once('/') else {
        return false;
    };
    let Some((first, last)) = span.split_once('-') else {
        return false;
    };
    let number = |text: &str| text.trim().parse::<u64>().ok();
    number(first) == Some(offset)
        && size
            .checked_sub(1)
            .is_some_and(|end| number(last) == Some(end))
        && (complete.trim() == "*" || number(complete) == Some(size))
}

/// Scheme, authority with an explicit port, compared case-insensitively.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Origin {
    scheme: String,
    authority: String,
}

impl Origin {
    fn of(url: &str) -> Option<Self> {
        let (scheme, rest) = url.split_once("://")?;
        let scheme = scheme.to_ascii_lowercase();
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = rest[..end].to_ascii_lowercase();
        if authority.is_empty() || authority.contains('@') {
            return None;
        }
        let default_port = match scheme.as_str() {
            "https" => "443",
            "http" => "80",
            _ => return None,
        };
        let has_port = match authority.strip_prefix('[') {
            Some(inner) => inner
                .split_once(']')
                .is_some_and(|(_, after)| after.starts_with(':')),
            None => authority.contains(':'),
        };
        let authority = if has_port {
            authority
        } else {
            format!("{authority}:{default_port}")
        };
        Some(Self { scheme, authority })
    }
}

/// RFC 3986 §5.2 reference resolution against an absolute base URL.
pub(crate) fn resolve(base: &str, reference: &str) -> Option<String> {
    let reference = reference.trim();
    let reference = reference.split('#').next().unwrap_or_default();
    let (scheme, rest) = base.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let base_path = rest[end..].split(['?', '#']).next().unwrap_or_default();
    let has_scheme = reference.find(':').is_some_and(|colon| {
        let candidate = &reference[..colon];
        !candidate.is_empty()
            && candidate.starts_with(|c: char| c.is_ascii_alphabetic())
            && candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    let (path, query) = match reference.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (reference, None),
    };
    let with_query = |prefix: String, query: Option<&str>| match query {
        Some(query) => format!("{prefix}?{query}"),
        None => prefix,
    };
    if has_scheme || reference.starts_with("//") {
        let absolute = if has_scheme {
            reference.to_owned()
        } else {
            format!("{scheme}:{reference}")
        };
        let (target_scheme, rest) = absolute.split_once("://")?;
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (target_authority, tail) = rest.split_at(end);
        let (path, query) = match tail.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (tail, None),
        };
        let path = if path.is_empty() {
            "/".to_owned()
        } else {
            remove_dot_segments(path)
        };
        return Some(with_query(
            format!("{target_scheme}://{target_authority}{path}"),
            query,
        ));
    }
    let origin = format!("{scheme}://{authority}");
    let base_query = rest[end..]
        .split('#')
        .next()
        .and_then(|t| t.split_once('?'))
        .map(|(_, q)| q);
    if path.is_empty() {
        let path = if base_path.is_empty() { "/" } else { base_path };
        return Some(with_query(format!("{origin}{path}"), query.or(base_query)));
    }
    let merged = if path.starts_with('/') {
        path.to_owned()
    } else {
        let directory = base_path.rsplit_once('/').map_or("", |(dir, _)| dir);
        format!("{directory}/{path}")
    };
    Some(with_query(
        format!("{origin}{}", remove_dot_segments(&merged)),
        query,
    ))
}

/// RFC 3986 §5.2.4 for an absolute path.
fn remove_dot_segments(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').collect();
    let last = segments.len() - 1;
    let mut output: Vec<&str> = Vec::new();
    for (i, segment) in segments.iter().enumerate() {
        match *segment {
            "." | ".." => {
                if *segment == ".." && output.len() > 1 {
                    output.pop();
                }
                if i == last {
                    output.push("");
                }
            }
            other => output.push(other),
        }
    }
    let joined = output.join("/");
    if joined.starts_with('/') {
        joined
    } else {
        format!("/{joined}")
    }
}

/// A URL without its query string or fragment.
fn without_query(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or_default()
}

/// Remove the query string of every URL in `text`. Presigned object-storage
/// URLs carry their signature there, and HTTP library errors quote URLs.
pub(crate) fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = ["http://", "https://"]
        .iter()
        .filter_map(|scheme| rest.find(scheme))
        .min()
    {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | ','))
            .unwrap_or(tail.len());
        out.push_str(without_query(&tail[..end]));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// RFC 8288 `Link: <url>; rel="next"`.
fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|link| {
        let (url, params) = link.trim().split_once(';')?;
        let url = url.trim().strip_prefix('<')?.strip_suffix('>')?;
        params
            .split(';')
            .any(|p| {
                let p = p.trim().replace(' ', "");
                p == "rel=\"next\"" || p == "rel=next"
            })
            .then(|| url.to_owned())
    })
}

/// Sends a TLS registry's requests to a loopback test registry over plain
/// HTTP instead, so a test can pull "from" a registry name such as the family
/// default without the network.
#[cfg(test)]
struct Routed {
    from: String,
    to: String,
    inner: Box<dyn Transport>,
}

#[cfg(test)]
impl Transport for Routed {
    fn send(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, String)],
        body: &Body<'_>,
    ) -> std::io::Result<Response> {
        let url = match url.strip_prefix(&self.from) {
            Some(rest) => format!("{}{rest}", self.to),
            None => url.to_owned(),
        };
        self.inner.send(method, &url, headers, body)
    }
}

#[cfg(test)]
impl Client {
    /// An anonymous client for the TLS registry `host` whose requests reach
    /// the loopback registry at `loopback`.
    pub(crate) fn routed(host: &str, loopback: &str) -> Self {
        let base = format!("https://{host}");
        Self {
            host: host.into(),
            origin: Origin::of(&base).expect("valid registry host"),
            transport: Box::new(Routed {
                from: base.clone(),
                to: format!("http://{loopback}"),
                inner: crate::http::transport(false, Timeouts::TRANSFER).expect("plain transport"),
            }),
            base,
            tls: true,
            confidential: true,
            credentials: Box::new(crate::credentials::Anonymous),
            authorization: Mutex::new(None),
            challenge: Mutex::new(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenges_name_their_scheme_and_parameters() {
        assert_eq!(
            Challenge::parse(r#"Basic realm="registry.unemu.dev""#),
            Challenge::Basic {
                realm: Some("registry.unemu.dev".into())
            }
        );
        assert_eq!(
            Challenge::parse(
                r#"Bearer realm="https://auth.example/token",service="registry.example",scope="repository:a/b:pull,push""#
            ),
            Challenge::Bearer {
                realm: "https://auth.example/token".into(),
                service: Some("registry.example".into()),
                scope: Some("repository:a/b:pull,push".into()),
            }
        );
        assert_eq!(
            Challenge::parse("Negotiate"),
            Challenge::Other("negotiate".into())
        );
    }

    #[test]
    fn upload_ranges_are_inclusive_and_links_name_the_next_page() {
        assert_eq!(parse_range(Some("0-99")).unwrap(), Some(100));
        assert_eq!(parse_range(Some("0-0")).unwrap(), Some(1), "one byte held");
        assert_eq!(parse_range(Some("bytes=0-0")).unwrap(), Some(1));
        assert_eq!(parse_range(Some("0--1")).unwrap(), Some(0), "zot: empty");
        assert_eq!(parse_range(None).unwrap(), None);
        assert!(parse_range(Some("5-9")).is_err());
        assert!(parse_range(Some("0--2")).is_err());
        assert_eq!(
            next_link(r#"</v2/_catalog?n=2&last=b>; rel="next""#).as_deref(),
            Some("/v2/_catalog?n=2&last=b")
        );
        assert_eq!(next_link(r#"</v2/x>; rel="prev""#), None);
    }

    #[test]
    fn content_ranges_must_cover_exactly_the_rest_of_the_blob() {
        assert!(content_range_matches(Some("bytes 100-199/200"), 100, 200));
        assert!(content_range_matches(Some("bytes 100-199/*"), 100, 200));
        for wrong in [
            None,
            Some("bytes 101-199/200"),
            Some("bytes 100-198/200"),
            Some("bytes 100-199/300"),
            Some("100-199/200"),
            Some("bytes 100-199"),
        ] {
            assert!(!content_range_matches(wrong, 100, 200), "{wrong:?}");
        }
    }

    #[test]
    fn references_resolve_against_the_request_url() {
        let base = "https://r.example/v2/a/blobs/uploads/s1?state=x";
        for (reference, expected) in [
            (
                "/v2/a/blobs/uploads/s2",
                "https://r.example/v2/a/blobs/uploads/s2",
            ),
            (
                "s2?state=y",
                "https://r.example/v2/a/blobs/uploads/s2?state=y",
            ),
            ("./s2", "https://r.example/v2/a/blobs/uploads/s2"),
            (
                "../uploads/s3#part",
                "https://r.example/v2/a/blobs/uploads/s3",
            ),
            ("../../../../../x", "https://r.example/x"),
            (
                "?state=z",
                "https://r.example/v2/a/blobs/uploads/s1?state=z",
            ),
            ("", "https://r.example/v2/a/blobs/uploads/s1?state=x"),
            ("//cdn.example/b?sig=1", "https://cdn.example/b?sig=1"),
            (
                "https://s3.example:8443/b/./c/../d",
                "https://s3.example:8443/b/d",
            ),
            ("http://h.example", "http://h.example/"),
        ] {
            assert_eq!(
                resolve(base, reference).as_deref(),
                Some(expected),
                "{reference}"
            );
        }
        assert_eq!(
            resolve("http://127.0.0.1:5000", "v2/x").as_deref(),
            Some("http://127.0.0.1:5000/v2/x")
        );
    }

    #[test]
    fn origins_compare_scheme_host_and_port() {
        assert_eq!(
            Origin::of("https://R.example/x"),
            Origin::of("https://r.example:443/y")
        );
        assert_ne!(
            Origin::of("https://r.example"),
            Origin::of("https://r.example:5000")
        );
        assert_ne!(
            Origin::of("https://r.example"),
            Origin::of("http://r.example")
        );
        assert_eq!(Origin::of("http://[::1]/"), Origin::of("http://[::1]:80/"));
        assert_ne!(
            Origin::of("http://[::1]:80/"),
            Origin::of("http://[::1]:81/")
        );
        assert!(Origin::of("https://user@r.example/").is_none());
        assert!(Origin::of("ftp://r.example/").is_none());
    }

    struct Unused;
    impl Transport for Unused {
        fn send(
            &self,
            _: Method,
            _: &str,
            _: &[(&str, String)],
            _: &Body<'_>,
        ) -> std::io::Result<Response> {
            Err(std::io::Error::other("no network in this test"))
        }
    }

    fn offline(host: &str, tls: bool) -> Client {
        let base = format!("{}://{host}", if tls { "https" } else { "http" });
        Client {
            host: host.into(),
            origin: Origin::of(&base).unwrap(),
            base,
            tls,
            confidential: true,
            transport: Box::new(Unused),
            credentials: Box::new(crate::credentials::Anonymous),
            authorization: Mutex::new(None),
            challenge: Mutex::new(None),
        }
    }

    #[test]
    fn registry_urls_must_use_https_or_stay_on_loopback() {
        let tls = offline("r.example", true);
        assert!(tls.permitted("https://s3.example/b?sig=1").is_ok());
        assert!(tls.permitted("http://127.0.0.1:5000/x").is_err());
        let error = tls
            .permitted("http://s3.example/b?X-Amz-Signature=secret")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("http://s3.example/b") && !error.contains("secret"),
            "{error}"
        );
        let plain = offline("127.0.0.1:5000", false);
        assert!(plain.permitted("http://127.0.0.1:5001/x").is_ok());
        assert!(plain.permitted("http://localhost/x").is_ok());
        assert!(plain.permitted("http://10.0.0.1/x").is_err());
        assert!(plain.permitted("https://r.example/x").is_err());
        assert!(tls.same_origin("https://r.example:443/v2/"));
        assert!(!tls.same_origin("https://r.example:8443/v2/"));
    }

    #[test]
    fn presigned_queries_never_reach_error_text() {
        assert_eq!(
            redact("configured for https only: http://s3.example/b/o?X-Amz-Signature=abc&X-Amz-Credential=k"),
            "configured for https only: http://s3.example/b/o"
        );
        assert_eq!(
            redact("GET https://a.example/x?sig=1, then <http://b.example/y?t=2> failed"),
            "GET https://a.example/x, then <http://b.example/y> failed"
        );
        assert_eq!(redact("no address here"), "no address here");
    }
}
