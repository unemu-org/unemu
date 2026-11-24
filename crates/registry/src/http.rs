// SPDX-License-Identifier: AGPL-3.0-or-later
//! Blocking HTTP transport: ureq over rustls with the ring provider.
//!
//! Request bodies carry an explicit `Content-Length` and stream from memory or
//! from a file range, so a blob is never held whole in memory. Server
//! certificates are verified against the operating system's trust store.
//! Redirects keep `Authorization` only for the same host over HTTPS, and
//! environment proxies are ignored so that every request goes where the
//! registry entry says.

use std::{
    fs::File,
    io::{self, Read},
    sync::{Arc, OnceLock},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
    Head,
    Post,
    Patch,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Patch => "PATCH",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

/// A replayable request body: each attempt reads it afresh.
pub(crate) enum Body<'a> {
    Empty,
    Bytes(&'a [u8]),
    File {
        file: &'a File,
        offset: u64,
        len: u64,
    },
}

impl Body<'_> {
    pub fn len(&self) -> u64 {
        match self {
            Self::Empty => 0,
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::File { len, .. } => *len,
        }
    }

    fn reader(&self) -> Box<dyn Read + '_> {
        match self {
            Self::Empty => Box::new(io::empty()),
            Self::Bytes(bytes) => Box::new(*bytes),
            Self::File { file, offset, len } => Box::new(FileRange {
                file,
                position: *offset,
                end: offset + len,
            }),
        }
    }
}

/// Positional reads, so the shared file cursor is never moved.
struct FileRange<'a> {
    file: &'a File,
    position: u64,
    end: u64,
}

impl Read for FileRange<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = self.end - self.position;
        if remaining == 0 {
            return Ok(0);
        }
        let want = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        #[cfg(unix)]
        let count =
            std::os::unix::fs::FileExt::read_at(self.file, &mut buffer[..want], self.position)?;
        #[cfg(windows)]
        let count = std::os::windows::fs::FileExt::seek_read(
            self.file,
            &mut buffer[..want],
            self.position,
        )?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "blob shorter than its descriptor",
            ));
        }
        self.position += count as u64;
        Ok(count)
    }
}

pub(crate) struct Response {
    pub status: u16,
    /// The URL this response answers; relative URLs in it resolve against this.
    pub url: String,
    headers: Vec<(String, String)>,
    body: Box<dyn Read + Send>,
}

impl Response {
    #[cfg(test)]
    pub(crate) fn manifest_bytes(url: &str, bytes: Vec<u8>) -> Self {
        Self {
            status: 200,
            url: url.into(),
            headers: Vec::new(),
            body: Box::new(io::Cursor::new(bytes)),
        }
    }

    #[cfg(test)]
    pub(crate) fn manifest_head(url: &str, digest: &crate::Digest) -> Self {
        let mut response = Self::manifest_bytes(url, Vec::new());
        response
            .headers
            .push(("docker-content-digest".into(), digest.to_string()));
        response
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_length(&self) -> Option<u64> {
        self.header("content-length")?.trim().parse().ok()
    }

    /// Read the whole body, refusing more than `limit` bytes.
    pub fn bytes(self, limit: u64) -> crate::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.body
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| crate::Error::Transport(e.to_string()))?;
        if bytes.len() as u64 > limit {
            return Err(crate::Error::Protocol(format!(
                "response exceeds {limit} bytes"
            )));
        }
        Ok(bytes)
    }

    pub fn into_reader(self) -> Box<dyn Read + Send> {
        self.body
    }
}

pub(crate) trait Transport: Send + Sync {
    fn send(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, String)],
        body: &Body<'_>,
    ) -> io::Result<Response>;
}

/// How long a transport waits to connect and for each read or write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Timeouts {
    pub connect: Duration,
    pub io: Duration,
}

impl Timeouts {
    /// Transfers: registries may take minutes to verify a multi-gigabyte blob
    /// before answering.
    pub const TRANSFER: Self = Self {
        connect: Duration::from_secs(30),
        io: Duration::from_secs(600),
    };
    /// Logins and connection tests, whose answers are small and immediate;
    /// a person is waiting for them.
    pub const PROBE: Self = Self {
        connect: Duration::from_secs(10),
        io: Duration::from_secs(15),
    };
}

/// `tls`: HTTPS only; otherwise plain HTTP only (the client restricts that to loopback).
pub(crate) fn transport(tls: bool, timeouts: Timeouts) -> crate::Result<Box<dyn Transport>> {
    let mut config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .https_only(tls)
        .proxy(None)
        .max_redirects(0)
        .user_agent(concat!("unemu-image-registry/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(timeouts.connect))
        .timeout_per_read(Some(timeouts.io))
        .timeout_per_write(Some(timeouts.io));
    if tls {
        let roots = native_roots().map_err(crate::Error::Config)?;
        config = config.tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::Rustls)
                .root_certs(ureq::tls::RootCerts::Specific(roots))
                .unversioned_rustls_crypto_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .build(),
        );
    }
    Ok(Box::new(Ureq {
        agent: ureq::Agent::new_with_config(config.build()),
    }))
}

type Roots = Arc<Vec<ureq::tls::Certificate<'static>>>;

fn native_roots() -> Result<Roots, String> {
    static ROOTS: OnceLock<Result<Roots, String>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let certificates: Vec<_> = rustls_native_certs::load_native_certs()
                .certs
                .into_iter()
                .map(|der| ureq::tls::Certificate::from_der(der.as_ref()).to_owned())
                .collect();
            if certificates.is_empty() {
                return Err("no trusted root certificates in the system store".into());
            }
            Ok(Arc::new(certificates))
        })
        .clone()
}

struct Ureq {
    agent: ureq::Agent,
}

impl Transport for Ureq {
    fn send(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, String)],
        body: &Body<'_>,
    ) -> io::Result<Response> {
        let mut request = ureq::http::Request::builder()
            .method(method.as_str())
            .uri(url);
        let mut has_length = false;
        for (name, value) in headers {
            has_length |= name.eq_ignore_ascii_case("content-length");
            request = request.header(*name, value.as_str());
        }
        let length = body.len();
        if !has_length
            && (length > 0 || matches!(method, Method::Post | Method::Put | Method::Patch))
        {
            request = request.header("Content-Length", length.to_string());
        }
        let response = if length == 0 {
            self.agent.run(
                request
                    .body(ureq::SendBody::none())
                    .map_err(io::Error::other)?,
            )
        } else {
            let mut reader = body.reader();
            let send = ureq::SendBody::from_reader(&mut reader);
            self.agent
                .run(request.body(send).map_err(io::Error::other)?)
        }
        .map_err(|error| match error {
            ureq::Error::Io(error) => error,
            other => io::Error::other(other.to_string()),
        })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        Ok(Response {
            status,
            url: url.to_owned(),
            headers,
            body: Box::new(response.into_body().into_reader()),
        })
    }
}
