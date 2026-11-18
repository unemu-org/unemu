// SPDX-License-Identifier: AGPL-3.0-or-later
//! One file downloaded over HTTPS, for the inputs a recipe build fetches
//! with the user's consent (`backlog/docs/design/recipes.md` §6.13): `GET`
//! only, redirects followed only to `https` URLs, at most a few of them, and
//! never more bytes handed on than the size the caller pins. The caller
//! hashes what it receives; nothing here trusts the server's word on size or
//! content. The transport is the registry client's: rustls with the
//! operating system's trust store, no environment proxy.
//!
//! **Time limits.** Connecting may take 30 s and each read 60 s; besides,
//! a whole download, redirects included, must finish within
//! [`deadline`]: 120 s plus one second per 32 KiB of the pinned size, a
//! floor of 32 KiB/s on average, so a server that trickles bytes cannot
//! hold a build for ever. The download stops with an error when it is
//! passed.

use crate::http::{Body, Method, Timeouts, Transport};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Connect and per-read limits of a download.
const TIMEOUTS: Timeouts = Timeouts {
    connect: Duration::from_secs(30),
    io: Duration::from_secs(60),
};

/// The slowest average rate a download may have, in bytes per second.
pub const MIN_RATE: u64 = 32 * 1024;

/// The time a whole download of `limit` bytes may take: 120 s, plus one
/// second per [`MIN_RATE`] bytes.
pub fn deadline(limit: u64) -> Duration {
    Duration::from_secs(120 + limit / MIN_RATE)
}

/// How many redirects a download follows.
const MAX_REDIRECTS: usize = 5;

/// Downloads over HTTPS.
pub struct Downloader {
    transport: Box<dyn Transport>,
    /// Test routing: requests to `https://HOST` go to `http://LOOPBACK`.
    #[cfg(any(test, feature = "test-support"))]
    route: Option<(String, String)>,
}

impl Downloader {
    /// A downloader that reaches HTTPS URLs only.
    pub fn new() -> crate::Result<Self> {
        Ok(Self {
            transport: crate::http::transport(true, TIMEOUTS)?,
            #[cfg(any(test, feature = "test-support"))]
            route: None,
        })
    }

    /// A downloader whose requests to `https://{host}` reach the loopback
    /// server at `loopback` over plain HTTP instead, so a test can fetch
    /// the URLs a recipe states without the network.
    #[cfg(any(test, feature = "test-support"))]
    pub fn routed(host: &str, loopback: &str) -> Self {
        Self {
            transport: crate::http::transport(false, TIMEOUTS).expect("plain transport"),
            route: Some((format!("https://{host}"), format!("http://{loopback}"))),
        }
    }

    fn send(&self, url: &str) -> std::io::Result<crate::http::Response> {
        #[cfg(any(test, feature = "test-support"))]
        if let Some((from, to)) = &self.route {
            if let Some(rest) = url.strip_prefix(from.as_str()) {
                return self
                    .transport
                    .send(Method::Get, &format!("{to}{rest}"), &[], &Body::Empty);
            }
        }
        self.transport.send(Method::Get, url, &[], &Body::Empty)
    }

    /// Download `url` into `sink`, refusing anything but `https`, a status
    /// other than 200 after redirects, and more than `limit` bytes. A
    /// shorter body is handed on as it is: the caller compares the size.
    pub fn fetch(&self, url: &str, limit: u64, sink: &mut dyn Write) -> Result<(), String> {
        self.fetch_within(url, limit, sink, deadline(limit))
    }

    /// [`Self::fetch`] that fails once `within` has passed.
    fn fetch_within(
        &self,
        url: &str,
        limit: u64,
        sink: &mut dyn Write,
        within: Duration,
    ) -> Result<(), String> {
        let started = Instant::now();
        let late = || {
            (started.elapsed() > within).then(|| {
                format!(
                    "the download took longer than the {} s allowed for {limit} bytes",
                    within.as_secs()
                )
            })
        };
        let mut url = url.to_owned();
        for _ in 0..=MAX_REDIRECTS {
            if let Some(error) = late() {
                return Err(error);
            }
            if !url.starts_with("https://") {
                return Err(format!("{}: only https URLs are fetched", strip(&url)));
            }
            let response = self
                .send(&url)
                .map_err(|e| crate::client::redact(&e.to_string()))?;
            match response.status {
                200 => {
                    if response
                        .content_length()
                        .is_some_and(|length| length > limit)
                    {
                        return Err(format!(
                            "the server announces {} bytes, more than the pinned {limit}",
                            response.content_length().unwrap_or_default()
                        ));
                    }
                    let mut body = response.into_reader().take(limit.saturating_add(1));
                    let mut buffer = vec![0u8; 64 * 1024];
                    let mut total = 0u64;
                    loop {
                        let n = body.read(&mut buffer).map_err(|e| e.to_string())?;
                        if n == 0 {
                            return Ok(());
                        }
                        total += n as u64;
                        if total > limit {
                            return Err(format!("more than the pinned {limit} bytes"));
                        }
                        sink.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
                        if let Some(error) = late() {
                            return Err(error);
                        }
                    }
                }
                301 | 302 | 303 | 307 | 308 => {
                    let location = response
                        .header("location")
                        .ok_or_else(|| format!("{}: a redirect without a location", strip(&url)))?;
                    url = crate::client::resolve(&url, location)
                        .ok_or_else(|| format!("{}: an invalid redirect", strip(&url)))?;
                }
                status => return Err(format!("{}: the server answered {status}", strip(&url))),
            }
        }
        Err(format!("more than {MAX_REDIRECTS} redirects"))
    }
}

/// A URL without its query, which may carry a signature.
fn strip(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    /// A loopback HTTP server that answers each request with the next of
    /// `responses` (status line and headers, then the body), then stops.
    fn serve(responses: Vec<(String, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for (head, body) in responses {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let mut stream = stream;
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        address
    }

    fn ok(body: &[u8]) -> (String, Vec<u8>) {
        (
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ),
            body.to_vec(),
        )
    }

    #[test]
    fn a_download_follows_https_redirects_and_stops_at_the_limit() {
        let address = serve(vec![
            (
                "HTTP/1.1 302 Found\r\nLocation: https://example.org/b\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
                Vec::new(),
            ),
            ok(b"the bytes"),
        ]);
        let downloader = Downloader::routed("example.org", &address);
        let mut out = Vec::new();
        downloader
            .fetch("https://example.org/a", 9, &mut out)
            .unwrap();
        assert_eq!(out, b"the bytes");

        let address = serve(vec![ok(b"too many bytes")]);
        let downloader = Downloader::routed("example.org", &address);
        let error = downloader
            .fetch("https://example.org/a", 4, &mut Vec::new())
            .unwrap_err();
        assert!(error.contains("more than the pinned 4"), "{error}");
    }

    #[test]
    fn a_download_that_passes_its_deadline_stops() {
        assert_eq!(deadline(0), Duration::from_secs(120));
        assert_eq!(deadline(737_280), Duration::from_secs(142));
        // A server that sends a byte every 150 ms: a read after the first
        // finds the deadline passed.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let mut stream = stream;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n");
            for byte in b"slow" {
                let _ = stream.write_all(&[*byte]);
                let _ = stream.flush();
                std::thread::sleep(Duration::from_millis(150));
            }
        });
        let downloader = Downloader::routed("example.org", &address);
        let error = downloader
            .fetch_within(
                "https://example.org/a",
                4,
                &mut Vec::new(),
                Duration::from_millis(100),
            )
            .unwrap_err();
        assert!(error.contains("took longer than"), "{error}");
    }

    #[test]
    fn plain_http_and_redirects_to_it_are_refused() {
        let downloader = Downloader::routed("example.org", "127.0.0.1:9");
        let error = downloader
            .fetch("http://example.org/a", 4, &mut Vec::new())
            .unwrap_err();
        assert!(error.contains("only https URLs"), "{error}");
        let address = serve(vec![(
            "HTTP/1.1 301 Moved\r\nLocation: http://example.org/b\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
            Vec::new(),
        )]);
        let downloader = Downloader::routed("example.org", &address);
        let error = downloader
            .fetch("https://example.org/a", 4, &mut Vec::new())
            .unwrap_err();
        assert!(error.contains("only https URLs"), "{error}");
        let address = serve(vec![(
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
            Vec::new(),
        )]);
        let downloader = Downloader::routed("example.org", &address);
        let error = downloader
            .fetch("https://example.org/a", 4, &mut Vec::new())
            .unwrap_err();
        assert!(error.contains("answered 404"), "{error}");
    }
}
