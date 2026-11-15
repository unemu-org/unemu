// SPDX-License-Identifier: AGPL-3.0-or-later
//! In-process OCI distribution registry for tests, over plain HTTP on loopback.
//!
//! Models the OCI Distribution Specification 1.1 endpoints this client uses:
//! `/v2/`, blob `HEAD`/`GET`, upload sessions (`POST`, `PATCH` with
//! `Content-Range`, status `GET`, `PUT ?digest=`), cross-repository mount,
//! manifest `HEAD`/`GET`/`PUT`/`DELETE`, `tags/list` and `_catalog` with `n`,
//! `last` and `Link` paging, or a registry without a catalog. Basic
//! authentication follows RFC 7617; a policy can answer with Bearer
//! challenges instead. Blob downloads honour
//! `Range: bytes=<start>-` (RFC 9110 §14) unless told to ignore or misreport
//! it, and can redirect to another server. Fault injection drops connections
//! mid-upload and mid-download, forgets sessions and corrupts blobs, and one
//! request can be held until the test releases it. Empty
//! upload sessions report `Range: 0--1`, as zot does. A request log lets tests
//! assert ordering. Never used outside tests.

use crate::{config::RegistryEntry, credentials::base64};
use image_store::Digest;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread::JoinHandle,
};

/// Who may do what. Namespaces are first path components; `*` matches all.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    pub anonymous_read: Vec<String>,
    pub anonymous_write: Vec<String>,
    /// Basic credentials that may read and write every repository.
    pub user: Option<(String, String)>,
    /// A misconfigured registry whose catalog lists every repository anonymously.
    pub anonymous_catalog: bool,
    /// Challenge with a Bearer token realm, as Docker Hub, GHCR or Forgejo do.
    pub bearer_challenge: bool,
    /// A registry without the optional catalog endpoint, which answers it `404`.
    pub no_catalog: bool,
}

/// Cut the next `GET` of `digest` after sending `keep` bytes of its body.
/// Several faults for one digest cut successive downloads.
#[derive(Clone, Debug)]
pub struct BlobFault {
    pub digest: Digest,
    pub keep: usize,
}

/// How blob downloads answer a `Range` request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RangeSupport {
    #[default]
    Honour,
    /// Answer `200` with the whole blob.
    Ignore,
    /// Answer `206` with a `Content-Range` one byte off.
    Misreport,
}

/// Drop the connection of the `nth` PATCH (1-based) after keeping `keep` bytes.
#[derive(Clone, Copy, Debug)]
pub struct PatchFault {
    pub nth: usize,
    pub keep: usize,
    /// Forget the whole session, as zot does when a client dies mid-PATCH.
    pub drop_session: bool,
}

#[derive(Default)]
struct State {
    policy: Policy,
    blobs: HashMap<String, Vec<u8>>,
    repo_blobs: BTreeSet<(String, String)>,
    manifests: HashMap<(String, String), (String, Vec<u8>)>,
    tags: BTreeMap<(String, String), String>,
    uploads: HashMap<String, (String, Vec<u8>)>,
    next_upload: u64,
    patches: usize,
    faults: Vec<PatchFault>,
    corrupt: BTreeSet<String>,
    log: Vec<String>,
    authorized: Vec<String>,
    challenges: usize,
    blob_faults: Vec<BlobFault>,
    ranges: Vec<String>,
    range_support: RangeSupport,
    redirect_blobs: Option<(String, String)>,
    relative_locations: bool,
    ambiguous_empty_range: bool,
    /// The next request whose `METHOD path` starts with this waits.
    hold: Option<String>,
    /// A held request is waiting for [`MockRegistry::release`].
    holding: bool,
}

/// A running mock registry; dropping it stops the server.
pub struct MockRegistry {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MockRegistry {
    pub fn start(policy: Policy) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local address");
        let state = Arc::new(Mutex::new(State {
            policy,
            ..State::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (state, stop) = (state.clone(), stop.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Ok(stream) = stream {
                        let state = state.clone();
                        std::thread::spawn(move || {
                            let _ = serve(stream, &state);
                        });
                    }
                }
            })
        };
        Self {
            addr,
            state,
            stop,
            thread: Some(thread),
        }
    }

    /// `127.0.0.1:<port>`.
    pub fn host(&self) -> String {
        self.addr.to_string()
    }

    /// A registry entry for this server with the plain-HTTP loopback opt-in.
    pub fn entry(&self, private: bool) -> RegistryEntry {
        RegistryEntry {
            host: self.host(),
            private,
            user: None,
            plain_http: true,
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `METHOD path` of every request so far, without query strings.
    pub fn log(&self) -> Vec<String> {
        self.state().log.clone()
    }

    pub fn clear_log(&self) {
        self.state().log.clear();
    }

    pub fn inject_patch_fault(&self, fault: PatchFault) {
        self.state().faults.push(fault);
    }

    pub fn inject_blob_fault(&self, fault: BlobFault) {
        self.state().blob_faults.push(fault);
    }

    pub fn set_range_support(&self, support: RangeSupport) {
        self.state().range_support = support;
    }

    /// Redirect every blob `GET` to the same path under `base`, such as
    /// `http://127.0.0.1:<port>`, followed by `query` (empty, or `?...` as an
    /// object-storage signature would be).
    pub fn redirect_blobs_to(&self, base: impl Into<String>, query: &str) {
        self.state().redirect_blobs = Some((base.into(), query.into()));
    }

    /// Return upload `Location` values relative to the request path.
    pub fn use_relative_locations(&self) {
        self.state().relative_locations = true;
    }

    /// Report an empty upload session as `0-0`, as distribution/registry
    /// does, which reads the same as a session holding one byte.
    pub fn report_empty_sessions_as_one_byte(&self) {
        self.state().ambiguous_empty_range = true;
    }

    /// Hold the next request whose `METHOD path` (as [`MockRegistry::log`]
    /// shows it) starts with `prefix`, before it is answered or logged, until
    /// [`MockRegistry::release`]. Later requests are not held.
    pub fn hold_next(&self, prefix: &str) {
        self.state().hold = Some(prefix.into());
    }

    /// A request is held now.
    pub fn held(&self) -> bool {
        self.state().holding
    }

    /// Let the held request continue.
    pub fn release(&self) {
        self.state().holding = false;
    }

    /// `METHOD path` of every request that carried an `Authorization` header.
    pub fn authorized_log(&self) -> Vec<String> {
        self.state().authorized.clone()
    }

    /// `Range` header values of blob downloads, in order.
    pub fn range_requests(&self) -> Vec<String> {
        self.state().ranges.clone()
    }

    /// Serve this blob with its first byte flipped.
    pub fn corrupt_blob(&self, digest: &Digest) {
        self.state().corrupt.insert(digest.to_string());
    }

    pub fn has_blob(&self, repository: &str, digest: &Digest) -> bool {
        self.state()
            .repo_blobs
            .contains(&(repository.into(), digest.to_string()))
    }

    /// Store a blob in a repository directly, as if pushed earlier.
    pub fn seed_blob(&self, repository: &str, bytes: &[u8]) -> Digest {
        let digest = Digest::of(bytes);
        let mut state = self.state();
        state.blobs.insert(digest.to_string(), bytes.to_vec());
        state
            .repo_blobs
            .insert((repository.into(), digest.to_string()));
        digest
    }

    /// Point a tag at a manifest, as another client would.
    pub fn seed_manifest(&self, repository: &str, tag: &str, bytes: &[u8]) -> Digest {
        let digest = Digest::of(bytes);
        let mut state = self.state();
        state.manifests.insert(
            (repository.into(), digest.to_string()),
            (image_store::media_type::MANIFEST.into(), bytes.to_vec()),
        );
        state
            .tags
            .insert((repository.into(), tag.into()), digest.to_string());
        digest
    }

    /// Store `bytes` under a digest they do not hash to, tagged `tag`.
    pub fn forge_manifest(&self, repository: &str, tag: &str, claimed: &Digest, bytes: &[u8]) {
        let mut state = self.state();
        state.manifests.insert(
            (repository.into(), claimed.to_string()),
            (image_store::media_type::MANIFEST.into(), bytes.to_vec()),
        );
        state
            .tags
            .insert((repository.into(), tag.into()), claimed.to_string());
    }

    /// Number of `401` challenges sent so far.
    pub fn challenges(&self) -> usize {
        self.state().challenges
    }

    pub fn tag_digest(&self, repository: &str, tag: &str) -> Option<Digest> {
        self.state()
            .tags
            .get(&(repository.into(), tag.into()))
            .and_then(|d| d.parse().ok())
    }

    /// Open upload sessions with the number of bytes each holds.
    pub fn sessions(&self) -> Vec<usize> {
        self.state()
            .uploads
            .values()
            .map(|(_, b)| b.len())
            .collect()
    }

    /// Create empty repositories so that catalog paging has entries.
    pub fn seed_repositories(&self, names: &[&str]) {
        for name in names {
            self.seed_blob(name, name.as_bytes());
        }
    }
}

impl Drop for MockRegistry {
    fn drop(&mut self) {
        self.release();
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Request {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn query(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
    fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }
    fn error(status: u16, code: &str) -> Self {
        Self::new(status)
            .header("Content-Type", "application/json")
            .body(format!(r#"{{"errors":[{{"code":"{code}","message":"{code}"}}]}}"#).into_bytes())
    }
}

enum Outcome {
    Reply(Response),
    /// Send the head and only the first `keep` body bytes, then close.
    Truncate(Response, usize),
    Disconnect,
}

fn serve(stream: TcpStream, state: &Mutex<State>) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let Some(request) = read_request(&mut reader)? else {
        return Ok(());
    };
    let head = request.method == "HEAD";
    let line = format!("{} {}", request.method, request.path);
    let lock = || state.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut state = lock();
        if state.hold.as_deref().is_some_and(|p| line.starts_with(p)) {
            state.hold = None;
            state.holding = true;
            drop(state);
            while lock().holding {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
    let outcome = {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .log
            .push(format!("{} {}", request.method, request.path));
        if request.header("authorization").is_some() {
            state
                .authorized
                .push(format!("{} {}", request.method, request.path));
        }
        let outcome = route(&mut state, &request);
        if matches!(&outcome, Outcome::Reply(r) if r.status == 401) {
            state.challenges += 1;
        }
        outcome
    };
    let (response, keep) = match outcome {
        Outcome::Reply(response) => (response, None),
        Outcome::Truncate(response, keep) => (response, Some(keep)),
        Outcome::Disconnect => return stream.shutdown(std::net::Shutdown::Both),
    };
    let mut out = stream;
    write!(
        out,
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        reason(response.status),
        response.body.len()
    )?;
    for (name, value) in &response.headers {
        write!(out, "{name}: {value}\r\n")?;
    }
    out.write_all(b"\r\n")?;
    if !head {
        let keep = keep.unwrap_or(response.body.len()).min(response.body.len());
        out.write_all(&response.body[..keep])?;
    }
    out.flush()?;
    if keep.is_some() {
        out.shutdown(std::net::Shutdown::Both)?;
    }
    Ok(())
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        206 => "Partial Content",
        307 => "Temporary Redirect",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        _ => "Status",
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Ok(None);
    };
    let (method, target) = (method.to_owned(), target.to_owned());
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    let find = |name: &str| {
        headers
            .iter()
            .find(|(n, _): &&(String, String)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let mut body = Vec::new();
    if let Some(length) = find("content-length").and_then(|v| v.parse::<usize>().ok()) {
        body.resize(length, 0);
        reader.read_exact(&mut body)?;
    } else if find("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        loop {
            line.clear();
            reader.read_line(&mut line)?;
            let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or("0"), 16)
                .map_err(io::Error::other)?;
            if size == 0 {
                line.clear();
                reader.read_line(&mut line)?;
                break;
            }
            let start = body.len();
            body.resize(start + size, 0);
            reader.read_exact(&mut body[start..])?;
            line.clear();
            reader.read_line(&mut line)?;
        }
    }
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_owned(), query.to_owned()),
        None => (target, String::new()),
    };
    let query = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(k), decode(v))
        })
        .collect();
    Ok(Some(Request {
        method,
        path: decode(&path),
        query,
        headers,
        body,
    }))
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn in_namespace(list: &[String], repository: &str) -> bool {
    let namespace = repository.split('/').next().unwrap_or_default();
    list.iter().any(|n| n == "*" || n == namespace)
}

fn authenticated(state: &State, request: &Request) -> Option<bool> {
    let header = request.header("authorization")?;
    Some(state.policy.user.as_ref().is_some_and(|(user, password)| {
        header == format!("Basic {}", base64(format!("{user}:{password}").as_bytes()))
    }))
}

fn challenge(state: &State) -> Response {
    let header = if state.policy.bearer_challenge {
        r#"Bearer realm="http://127.0.0.1/token",service="mock",scope="repository:any:pull""#
    } else {
        r#"Basic realm="mock""#
    };
    Response::error(401, "UNAUTHORIZED").header("WWW-Authenticate", header)
}

fn allowed(state: &State, request: &Request, repository: &str, write: bool) -> Option<Response> {
    match authenticated(state, request) {
        Some(true) => None,
        Some(false) => Some(challenge(state)),
        None => {
            let list = if write {
                &state.policy.anonymous_write
            } else {
                &state.policy.anonymous_read
            };
            (!in_namespace(list, repository)).then(|| challenge(state))
        }
    }
}

fn route(state: &mut State, request: &Request) -> Outcome {
    let Some(rest) = request.path.strip_prefix("/v2/") else {
        return Outcome::Reply(Response::error(404, "NOT_FOUND"));
    };
    if rest.is_empty() {
        let open = authenticated(state, request) == Some(true)
            || (authenticated(state, request).is_none() && !state.policy.anonymous_read.is_empty());
        return Outcome::Reply(if open {
            Response::new(200)
        } else {
            challenge(state)
        });
    }
    if rest == "_catalog" {
        return Outcome::Reply(if state.policy.no_catalog {
            Response::error(404, "UNSUPPORTED")
        } else {
            catalog(state, request)
        });
    }
    for (marker, kind) in [
        ("/blobs/uploads", Kind::Upload),
        ("/blobs/", Kind::Blob),
        ("/manifests/", Kind::Manifest),
        ("/tags/list", Kind::Tags),
    ] {
        if let Some(position) = rest.rfind(marker) {
            let repository = &rest[..position];
            let tail = rest[position + marker.len()..].trim_start_matches('/');
            let write = !matches!(request.method.as_str(), "GET" | "HEAD");
            if let Some(denied) = allowed(state, request, repository, write) {
                return Outcome::Reply(denied);
            }
            return match kind {
                Kind::Upload => upload(state, request, repository, tail),
                Kind::Blob => blob(state, request, repository, tail),
                Kind::Manifest => Outcome::Reply(manifest(state, request, repository, tail)),
                Kind::Tags => Outcome::Reply(tags(state, request, repository)),
            };
        }
    }
    Outcome::Reply(Response::error(404, "NOT_FOUND"))
}

enum Kind {
    Upload,
    Blob,
    Manifest,
    Tags,
}

fn repository_exists(state: &State, repository: &str) -> bool {
    state.repo_blobs.iter().any(|(r, _)| r == repository)
        || state.tags.keys().any(|(r, _)| r == repository)
        || state.manifests.keys().any(|(r, _)| r == repository)
}

fn blob(state: &mut State, request: &Request, repository: &str, digest: &str) -> Outcome {
    if !state
        .repo_blobs
        .contains(&(repository.into(), digest.into()))
    {
        return Outcome::Reply(Response::error(404, "BLOB_UNKNOWN"));
    }
    let mut bytes = state.blobs[digest].clone();
    if state.corrupt.contains(digest) {
        if let Some(first) = bytes.first_mut() {
            *first ^= 0xff;
        }
    }
    match request.method.as_str() {
        "HEAD" => Outcome::Reply(
            Response::new(200)
                .header("Docker-Content-Digest", digest)
                .header("Content-Type", "application/octet-stream")
                .body(bytes),
        ),
        "GET" => {
            if let Some((base, query)) = &state.redirect_blobs {
                return Outcome::Reply(Response::new(307).header(
                    "Location",
                    format!("{base}/v2/{repository}/blobs/{digest}{query}"),
                ));
            }
            let range = request.header("range").map(str::to_owned);
            let start = range
                .as_deref()
                .filter(|_| state.range_support != RangeSupport::Ignore)
                .and_then(|r| r.strip_prefix("bytes="))
                .and_then(|r| r.strip_suffix('-'))
                .and_then(|r| r.parse::<usize>().ok())
                .filter(|start| *start < bytes.len());
            state.ranges.extend(range);
            let total = bytes.len();
            let response = match start {
                Some(start) => {
                    let shown = match state.range_support {
                        RangeSupport::Misreport => start + 1,
                        _ => start,
                    };
                    Response::new(206)
                        .header(
                            "Content-Range",
                            format!("bytes {shown}-{}/{total}", total - 1),
                        )
                        .body(bytes[start..].to_vec())
                }
                None => Response::new(200).body(bytes),
            }
            .header("Docker-Content-Digest", digest)
            .header("Content-Type", "application/octet-stream");
            match state
                .blob_faults
                .iter()
                .position(|f| f.digest.to_string() == digest)
            {
                Some(i) => {
                    let fault = state.blob_faults.remove(i);
                    Outcome::Truncate(response, fault.keep)
                }
                None => Outcome::Reply(response),
            }
        }
        _ => Outcome::Reply(Response::error(405, "UNSUPPORTED")),
    }
}

fn upload(state: &mut State, request: &Request, repository: &str, id: &str) -> Outcome {
    let relative = state.relative_locations;
    let location = |id: &str| {
        if !relative {
            format!("/v2/{repository}/blobs/uploads/{id}")
        } else if request.path.ends_with("/uploads/") {
            format!("{id}?state=0")
        } else {
            format!("./{id}?state={}", request.body.len())
        }
    };
    let ambiguous = state.ambiguous_empty_range;
    let range = move |len: usize| match len {
        0 if !ambiguous => "0--1".to_owned(),
        len => format!("0-{}", len.saturating_sub(1)),
    };
    if id.is_empty() {
        if request.method != "POST" {
            return Outcome::Reply(Response::error(405, "UNSUPPORTED"));
        }
        if let (Some(digest), Some(from)) = (request.query("mount"), request.query("from")) {
            let readable = allowed(state, request, from, false).is_none();
            if readable && state.repo_blobs.contains(&(from.into(), digest.into())) {
                state.repo_blobs.insert((repository.into(), digest.into()));
                return Outcome::Reply(
                    Response::new(201)
                        .header("Location", format!("/v2/{repository}/blobs/{digest}"))
                        .header("Docker-Content-Digest", digest),
                );
            }
        }
        state.next_upload += 1;
        let id = format!("session-{}", state.next_upload);
        state
            .uploads
            .insert(id.clone(), (repository.into(), Vec::new()));
        return Outcome::Reply(
            Response::new(202)
                .header("Location", location(&id))
                .header("Range", "0-0")
                .header("Docker-Upload-UUID", id),
        );
    }
    let Some((owner, data)) = state.uploads.get(id).cloned() else {
        return Outcome::Reply(Response::error(404, "BLOB_UPLOAD_UNKNOWN"));
    };
    if owner != repository {
        return Outcome::Reply(Response::error(404, "BLOB_UPLOAD_UNKNOWN"));
    }
    match request.method.as_str() {
        "GET" => Outcome::Reply(
            Response::new(204)
                .header("Location", location(id))
                .header("Range", range(data.len())),
        ),
        "PATCH" => {
            if let Some(content_range) = request.header("content-range") {
                let start = content_range
                    .split('-')
                    .next()
                    .and_then(|s| s.parse::<usize>().ok());
                if start != Some(data.len()) {
                    return Outcome::Reply(
                        Response::error(416, "BLOB_UPLOAD_INVALID")
                            .header("Location", location(id))
                            .header("Range", range(data.len())),
                    );
                }
            }
            state.patches += 1;
            let fault = state
                .faults
                .iter()
                .position(|f| f.nth == state.patches)
                .map(|i| state.faults.remove(i));
            let session = state.uploads.get_mut(id).expect("checked above");
            if let Some(fault) = fault {
                let keep = fault.keep.min(request.body.len());
                session.1.extend_from_slice(&request.body[..keep]);
                if fault.drop_session {
                    state.uploads.remove(id);
                }
                return Outcome::Disconnect;
            }
            session.1.extend_from_slice(&request.body);
            let len = session.1.len();
            Outcome::Reply(
                Response::new(202)
                    .header("Location", location(id))
                    .header("Range", range(len)),
            )
        }
        "PUT" => {
            let mut data = data;
            data.extend_from_slice(&request.body);
            let actual = Digest::of(&data).to_string();
            if request.query("digest") != Some(actual.as_str()) {
                return Outcome::Reply(Response::error(400, "DIGEST_INVALID"));
            }
            state.uploads.remove(id);
            state.blobs.insert(actual.clone(), data);
            state.repo_blobs.insert((repository.into(), actual.clone()));
            Outcome::Reply(
                Response::new(201)
                    .header("Location", format!("/v2/{repository}/blobs/{actual}"))
                    .header("Docker-Content-Digest", actual),
            )
        }
        "DELETE" => {
            state.uploads.remove(id);
            Outcome::Reply(Response::new(204))
        }
        _ => Outcome::Reply(Response::error(405, "UNSUPPORTED")),
    }
}

fn manifest(state: &mut State, request: &Request, repository: &str, reference: &str) -> Response {
    let key = |digest: &str| (repository.to_owned(), digest.to_owned());
    let resolved = if reference.starts_with("sha256:") {
        Some(reference.to_owned())
    } else {
        state.tags.get(&key(reference)).cloned()
    };
    match request.method.as_str() {
        "HEAD" | "GET" => match resolved.and_then(|d| {
            state
                .manifests
                .get(&key(&d))
                .map(|m| (d.clone(), m.clone()))
        }) {
            Some((digest, (media_type, bytes))) => Response::new(200)
                .header("Content-Type", media_type)
                .header("Docker-Content-Digest", digest)
                .body(bytes),
            None => Response::error(404, "MANIFEST_UNKNOWN"),
        },
        "PUT" => {
            let digest = Digest::of(&request.body).to_string();
            if reference.starts_with("sha256:") && reference != digest {
                return Response::error(400, "DIGEST_INVALID");
            }
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&request.body) {
                let referenced = std::iter::once(&value["config"])
                    .chain(value["layers"].as_array().into_iter().flatten())
                    .filter_map(|d| d["digest"].as_str());
                for blob in referenced {
                    if !state.repo_blobs.contains(&key(blob)) {
                        return Response::error(400, "MANIFEST_BLOB_UNKNOWN");
                    }
                }
            } else {
                return Response::error(400, "MANIFEST_INVALID");
            }
            let media_type = request
                .header("content-type")
                .unwrap_or_default()
                .to_owned();
            state
                .manifests
                .insert(key(&digest), (media_type, request.body.clone()));
            if !reference.starts_with("sha256:") {
                state.tags.insert(key(reference), digest.clone());
            }
            Response::new(201)
                .header("Location", format!("/v2/{repository}/manifests/{digest}"))
                .header("Docker-Content-Digest", digest)
        }
        "DELETE" => {
            let Some(digest) = resolved else {
                return Response::error(404, "MANIFEST_UNKNOWN");
            };
            if state.manifests.remove(&key(&digest)).is_none() {
                return Response::error(404, "MANIFEST_UNKNOWN");
            }
            state
                .tags
                .retain(|(r, _), d| !(r == repository && *d == digest));
            Response::new(202)
        }
        _ => Response::error(405, "UNSUPPORTED"),
    }
}

fn page(
    items: Vec<String>,
    request: &Request,
    path: &str,
    field: &str,
    name: Option<&str>,
) -> Response {
    let last = request.query("last");
    let mut items: Vec<_> = items
        .into_iter()
        .filter(|i| last.is_none_or(|last| i.as_str() > last))
        .collect();
    let n = request
        .query("n")
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(usize::MAX);
    let more = items.len() > n;
    items.truncate(n);
    let mut body = serde_json::Map::new();
    if let Some(name) = name {
        body.insert("name".into(), name.into());
    }
    body.insert(field.into(), items.clone().into());
    let mut response = Response::new(200)
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(&body).expect("encode page"));
    if more {
        let last = items.last().cloned().unwrap_or_default();
        response = response.header("Link", format!("<{path}?n={n}&last={last}>; rel=\"next\""));
    }
    response
}

fn tags(state: &State, request: &Request, repository: &str) -> Response {
    if !repository_exists(state, repository) {
        return Response::error(404, "NAME_UNKNOWN");
    }
    let tags = state
        .tags
        .keys()
        .filter(|(r, _)| r == repository)
        .map(|(_, t)| t.clone())
        .collect();
    page(
        tags,
        request,
        &format!("/v2/{repository}/tags/list"),
        "tags",
        Some(repository),
    )
}

fn catalog(state: &State, request: &Request) -> Response {
    let everything = authenticated(state, request) == Some(true) || state.policy.anonymous_catalog;
    let repositories: BTreeSet<String> = state
        .repo_blobs
        .iter()
        .map(|(r, _)| r)
        .chain(state.tags.keys().map(|(r, _)| r))
        .filter(|r| everything || in_namespace(&state.policy.anonymous_read, r))
        .cloned()
        .collect();
    page(
        repositories.into_iter().collect(),
        request,
        "/v2/_catalog",
        "repositories",
        None,
    )
}
