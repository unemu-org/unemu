// SPDX-License-Identifier: AGPL-3.0-or-later
//! `unemu recipe build` (recipes.md §3.1, §6.13, §6.15): its output and exit
//! codes, and `--fetch`: a download happens only with consent, only for the
//! inputs the plan reports missing, after the list of what will be
//! downloaded is printed, and is kept in the store's download cache only
//! when its size and SHA-256 are the pinned ones. The downloads reach a
//! loopback HTTP server that stands for the `https` hosts a recipe names;
//! no test touches the network. Every test names its own preferences file
//! and store. Synthetic files only.

use cli_core::recipe::HttpsFetcher;
use image_registry::download::Downloader;
use recipe::build::{build_path, Outcome, Request, DOWNLOADS};
use recipe::library::Library;
use recipe::provider::Provider;
use recipe::store_index::StoreIndex;
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn upcli(args: &[OsString]) -> (u8, String, String) {
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run(args, &mut output, &mut errors);
    (
        code,
        String::from_utf8(output).unwrap(),
        String::from_utf8(errors).unwrap(),
    )
}

struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("recipe-build-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sha(bytes: &[u8]) -> String {
    image_store::Digest::of(bytes).hex().to_owned()
}

/// A recipe of two inputs with URLs: a 720K disk image taken unmodified
/// and a readme stored as an extra.
fn recipe(t: &Temp, readme: &[u8], data: &[u8]) -> PathBuf {
    let text = format!(
        "recipe = \"1.0\"\n\n[media]\nname = \"private/media/ibm-pc/fetched\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n\
         [title]\ntitle = \"Fetched\"\n\n\
         [input.readme]\nname = \"README.TXT\"\nsize = {}\nsha256 = \"{}\"\nurls = [\"https://example.org/dl/README.TXT\"]\n\n\
         [input.data]\nname = \"DATA.BIN\"\nsize = {}\nsha256 = \"{}\"\nurls = [\"https://example.org/dl/DATA.BIN\"]\n\n\
         [[medium]]\nid = \"disk-1\"\nlabel = \"Disk 1\"\nformat = \"720K\"\nfrom = \"data\"\n\n\
         [[extra]]\nid = \"readme\"\nfile_name = \"README.TXT\"\nkind = \"document\"\nfrom = \"readme\"\n",
        readme.len(),
        sha(readme),
        data.len(),
        sha(data),
    );
    let path = t.path("fetched.media.toml");
    fs::write(&path, text).unwrap();
    path
}

const DATA_SIZE: usize = 737_280;

fn data() -> Vec<u8> {
    (0..DATA_SIZE).map(|i| (i % 251) as u8).collect()
}

/// A loopback HTTP server answering `GET /dl/NAME` from `files`, logging
/// each request path, for as long as the test runs.
fn serve(files: Vec<(&'static str, Vec<u8>)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            let _ = reader.read_line(&mut first);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let path = first.split(' ').nth(1).unwrap_or_default().to_owned();
            seen.lock().unwrap().push(path.clone());
            let mut stream = stream;
            match files.iter().find(|(name, _)| format!("/dl/{name}") == path) {
                Some((_, body)) => {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body);
                }
                None => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        }
    });
    (address, log)
}

const PROVIDERS: [&dyn Provider; 1] = [&pc_image::recipe::PcRecipes];

/// Build with the libraries named and, with `fetch`, downloads through the
/// loopback server at `address`.
fn build(
    recipe: &Path,
    store: &image_store::Store,
    libraries: &[PathBuf],
    fetch: Option<&str>,
) -> (Outcome, Vec<String>) {
    let fetcher = fetch.map(|address| HttpsFetcher(Downloader::routed("example.org", address)));
    build_with_fetcher(
        recipe,
        store,
        libraries,
        fetcher.as_ref().map(|f| f as &dyn recipe::build::Fetcher),
    )
}

fn build_with_fetcher(
    recipe: &Path,
    store: &image_store::Store,
    libraries: &[PathBuf],
    fetcher: Option<&dyn recipe::build::Fetcher>,
) -> (Outcome, Vec<String>) {
    let index = StoreIndex::load(store).unwrap();
    let mut libraries: Vec<Library> = libraries
        .iter()
        .map(|dir| Library::scan(dir, None).unwrap())
        .collect();
    let mut lines = Vec::new();
    let outcome = build_path(
        Request {
            path: recipe,
            providers: recipe::Providers(&PROVIDERS),
            store,
            index: Some(&index),
            store_read: None,
            libraries: &mut libraries,
            fetcher,
            keep_as: None,
            find_outputs: true,
        },
        &mut |line| lines.push(line.to_owned()),
    )
    .unwrap();
    (outcome, lines)
}

fn tags(store: &image_store::Store) -> usize {
    store.list_tags(None, 100).unwrap().tags.len()
}

#[test]
fn fetch_downloads_only_the_missing_inputs_after_listing_them() {
    let t = Temp::new("fetch");
    let readme = b"Read me first.\r\n".to_vec();
    let data = data();
    let recipe = recipe(&t, &readme, &data);
    let lib = t.path("lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(lib.join("README.TXT"), &readme).unwrap();
    let (address, requests) = serve(vec![
        ("README.TXT", readme.clone()),
        ("DATA.BIN", data.clone()),
    ]);
    let store = image_store::Store::open(t.path("store")).unwrap();

    // Without consent nothing is downloaded and nothing is tagged.
    let (outcome, lines) = build(&recipe, &store, std::slice::from_ref(&lib), None);
    assert!(matches!(outcome, Outcome::Missing(_)), "{outcome:#?}");
    assert!(lines.is_empty());
    assert!(requests.lock().unwrap().is_empty());
    assert_eq!(tags(&store), 0);

    // With consent, the list comes first, and only the missing input is
    // requested; the readme lies in the library.
    let (outcome, lines) = build(&recipe, &store, std::slice::from_ref(&lib), Some(&address));
    let Outcome::Built(report) = outcome else {
        panic!("{outcome:#?}");
    };
    assert_eq!(
        lines,
        [
            "fetch: downloading 1 input (737280 bytes), as --fetch allows:".to_owned(),
            format!(
                "  data (DATA.BIN): 737280 bytes, sha256 {}, from https://example.org/dl/DATA.BIN",
                sha(&data)
            ),
            "  data: fetched from https://example.org/dl/DATA.BIN".to_owned(),
        ]
    );
    assert_eq!(*requests.lock().unwrap(), ["/dl/DATA.BIN"]);
    assert_eq!(report.reference, "private/media/ibm-pc/fetched:r1");
    // Kept in the download cache by its hash.
    let cached = store.root().join(DOWNLOADS).join(sha(&data));
    assert_eq!(fs::read(&cached).unwrap(), data);

    // A later build needs no network: the cache is a source of the plan.
    let again = image_store::Store::open(t.path("again")).unwrap();
    fs::create_dir_all(again.root().join(DOWNLOADS)).unwrap();
    fs::copy(&cached, again.root().join(DOWNLOADS).join(sha(&data))).unwrap();
    let (outcome, lines) = build(&recipe, &again, std::slice::from_ref(&lib), None);
    let Outcome::Built(second) = outcome else {
        panic!("{outcome:#?}");
    };
    assert!(lines.is_empty());
    assert_eq!(second.published.descriptor, report.published.descriptor);
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[test]
fn a_download_of_other_bytes_is_not_kept_and_nothing_is_tagged() {
    let t = Temp::new("wrong");
    let readme = b"Read me first.\r\n".to_vec();
    let data = data();
    let recipe = recipe(&t, &readme, &data);
    let lib = t.path("lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(lib.join("README.TXT"), &readme).unwrap();
    let mut other = data.clone();
    other[0] ^= 1;
    let (address, requests) = serve(vec![("DATA.BIN", other)]);
    let store = image_store::Store::open(t.path("store")).unwrap();
    let (outcome, lines) = build(&recipe, &store, &[lib], Some(&address));
    let Outcome::Failed { error, .. } = outcome else {
        panic!("{outcome:#?}");
    };
    assert!(
        error.starts_with(
            "input data (DATA.BIN) could not be fetched: https://example.org/dl/DATA.BIN: sha256 "
        ),
        "{error}"
    );
    assert!(
        lines[1].contains("from https://example.org/dl/DATA.BIN"),
        "{lines:?}"
    );
    assert_eq!(*requests.lock().unwrap(), ["/dl/DATA.BIN"]);
    let cache = store.root().join(DOWNLOADS);
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 0, "nothing kept");
    assert_eq!(tags(&store), 0);
}

/// The same exact-byte responses and request log as the local server,
/// without sockets, so builder and CLI tests also run in a network sandbox.
struct FakeFetcher {
    files: Vec<(&'static str, Vec<u8>)>,
    requests: Mutex<Vec<String>>,
}

impl FakeFetcher {
    fn new(files: Vec<(&'static str, Vec<u8>)>) -> Self {
        Self {
            files,
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl recipe::build::Fetcher for FakeFetcher {
    fn fetch(&self, url: &str, _: u64, sink: &mut dyn Write) -> Result<(), String> {
        self.requests.lock().unwrap().push(url.to_owned());
        let name = url
            .strip_prefix("https://example.org/dl/")
            .ok_or("unexpected URL")?;
        let (_, bytes) = self
            .files
            .iter()
            .find(|(file, _)| *file == name)
            .ok_or("not found")?;
        sink.write_all(bytes).map_err(|e| e.to_string())
    }
}

/// The archive itself and a file-set member need the same synthetic bytes.
/// The FAT step stores those bytes as a file without unpacking them.
fn file_set_recipe(t: &Temp, bytes: &[u8]) -> PathBuf {
    let path = t.path("package.media.toml");
    fs::write(
        &path,
        format!(
            r#"recipe = "1.0"
[media]
name = "private/media/ibm-pc/package"
platform = "ibm-pc"
revision = 1
[title]
title = "Package"
[input.zip]
name = "package.zip"
size = {size}
sha256 = "{hash}"
urls = ["https://example.org/dl/package.zip"]
[input.package]
name = "package"
files = [{{ path = "APPEND.ZIP", size = {size}, sha256 = "{hash}" }}]
[[step]]
id = "disk"
fat = "package"
format = "720K"
[[medium]]
id = "disk-1"
label = "Disk 1"
format = "720K"
from = "disk"
[[extra]]
id = "archive"
file_name = "package.zip"
kind = "other"
from = "zip"
"#,
            size = bytes.len(),
            hash = sha(bytes),
        ),
    )
    .unwrap();
    path
}

fn remove_readme_url(path: &Path) {
    let text = fs::read_to_string(path)
        .unwrap()
        .replace("urls = [\"https://example.org/dl/README.TXT\"]\n", "");
    fs::write(path, text).unwrap();
}

const PACKAGE_NO_URL: &str =
    "fetch: input package has no URL; will look for it by hash after downloads";
const README_NO_URL: &str =
    "fetch: input readme has no URL; will look for it by hash after downloads";

#[test]
fn fetch_resolves_a_url_less_file_set_by_hash_after_downloading() {
    let t = Temp::new("file-set-fetch");
    let bytes = b"Synthetic package bytes.";
    let recipe = file_set_recipe(&t, bytes);
    let fetcher = FakeFetcher::new(vec![("package.zip", bytes.to_vec())]);
    let store = image_store::Store::open(t.path("store")).unwrap();
    let (outcome, lines) = build_with_fetcher(&recipe, &store, &[], Some(&fetcher));
    let Outcome::Built(report) = outcome else {
        panic!("{outcome:#?}");
    };
    assert_eq!(lines[0], PACKAGE_NO_URL);
    assert_eq!(
        lines[1],
        "fetch: downloading 1 input (24 bytes), as --fetch allows:"
    );
    assert_eq!(
        *fetcher.requests.lock().unwrap(),
        ["https://example.org/dl/package.zip"]
    );
    assert_eq!(
        fs::read(store.root().join(DOWNLOADS).join(sha(bytes))).unwrap(),
        bytes
    );
    assert_eq!(tags(&store), 1);

    let lib = t.path("lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(lib.join("APPEND.ZIP"), bytes).unwrap();
    let local = image_store::Store::open(t.path("local-store")).unwrap();
    let (outcome, _) = build_with_fetcher(&recipe, &local, &[lib], None);
    let Outcome::Built(local_report) = outcome else {
        panic!("{outcome:#?}");
    };
    assert_eq!(
        report.published.descriptor,
        local_report.published.descriptor
    );
}

#[test]
fn fetch_downloads_available_inputs_and_reports_only_what_is_still_missing() {
    let t = Temp::new("partial-fetch");
    let data = data();
    let recipe = recipe(&t, b"Missing readme.", &data);
    remove_readme_url(&recipe);
    let fetcher = FakeFetcher::new(vec![("DATA.BIN", data.clone())]);
    let store = image_store::Store::open(t.path("store")).unwrap();
    let (outcome, lines) = build_with_fetcher(&recipe, &store, &[], Some(&fetcher));
    let Outcome::Missing(plan) = outcome else {
        panic!("{outcome:#?}")
    };
    let missing: Vec<_> = plan
        .inputs
        .iter()
        .filter(|i| matches!(i.status, recipe::plan::Status::Missing(_)))
        .map(|i| i.id.as_str())
        .collect();
    assert_eq!(missing, ["readme"]);
    assert_eq!(lines[0], README_NO_URL);
    assert!(
        lines[1].starts_with("fetch: downloading 1 input"),
        "{lines:?}"
    );
    assert_eq!(
        *fetcher.requests.lock().unwrap(),
        ["https://example.org/dl/DATA.BIN"]
    );
    assert_eq!(
        fs::read(store.root().join(DOWNLOADS).join(sha(&data))).unwrap(),
        data
    );
    assert_eq!(tags(&store), 0);
}

#[test]
fn fetch_downloads_nothing_when_all_missing_inputs_lack_urls() {
    let t = Temp::new("all-no-url");
    let recipe = recipe(&t, b"Missing readme.", &data());
    remove_readme_url(&recipe);
    let text = fs::read_to_string(&recipe)
        .unwrap()
        .replace("urls = [\"https://example.org/dl/DATA.BIN\"]\n", "");
    fs::write(&recipe, text).unwrap();
    let fetcher = FakeFetcher::new(vec![]);
    let store = image_store::Store::open(t.path("store")).unwrap();
    let (outcome, lines) = build_with_fetcher(&recipe, &store, &[], Some(&fetcher));
    let Outcome::Missing(plan) = outcome else {
        panic!("{outcome:#?}")
    };
    assert_eq!(plan.summary(), "missing 2 of 2 inputs");
    assert_eq!(
        lines,
        [
            "fetch: input data has no URL; will look for it by hash after downloads",
            README_NO_URL,
            "fetch: nothing could be downloaded: no missing required file input has a URL",
        ]
    );
    assert!(fetcher.requests.lock().unwrap().is_empty());
    assert!(!store.root().join(DOWNLOADS).exists());
    assert_eq!(tags(&store), 0);
}

#[test]
fn fetch_downloads_inputs_with_the_same_hash_and_size_once() {
    let t = Temp::new("duplicate-fetch");
    let bytes = b"Synthetic package bytes.";
    let recipe = file_set_recipe(&t, bytes);
    let mut text = fs::read_to_string(&recipe).unwrap();
    text.push_str(&format!(
        r#"
[input.copy]
name = "copy.zip"
size = {}
sha256 = "{}"
urls = ["https://example.org/dl/copy.zip"]
[[extra]]
id = "copy"
file_name = "copy.zip"
kind = "other"
from = "copy"
"#,
        bytes.len(),
        sha(bytes),
    ));
    let lib = t.path("lib");
    fs::create_dir_all(&lib).unwrap();
    // A different file-set member keeps both URL'd copies required and missing.
    text = text.replace(
        &format!(
            "path = \"APPEND.ZIP\", size = {}, sha256 = \"{}\"",
            bytes.len(),
            sha(bytes)
        ),
        &format!(
            "path = \"APPEND.ZIP\", size = 1, sha256 = \"{}\"",
            sha(b"x")
        ),
    );
    fs::write(lib.join("APPEND.ZIP"), b"x").unwrap();
    fs::write(&recipe, text).unwrap();
    let fetcher = FakeFetcher::new(vec![
        ("package.zip", bytes.to_vec()),
        ("copy.zip", bytes.to_vec()),
    ]);
    let store = image_store::Store::open(t.path("store")).unwrap();
    let (outcome, lines) = build_with_fetcher(&recipe, &store, &[lib], Some(&fetcher));
    assert!(matches!(outcome, Outcome::Built(_)), "{outcome:#?}");
    assert_eq!(
        *fetcher.requests.lock().unwrap(),
        ["https://example.org/dl/copy.zip"]
    );
    assert_eq!(
        lines[0],
        "fetch: downloading 1 input (24 bytes), as --fetch allows:"
    );
}

/// Run the CLI parser and build handler with a fake fetcher.
fn upcli_fetch(
    t: &Temp,
    path: &Path,
    fetcher: &dyn recipe::build::Fetcher,
) -> (u8, String, String) {
    let args = words(t, path, &["--fetch"]);
    let cli_core::Command::Recipe(cli_core::recipe::RecipeCommand::Build(options)) =
        cli_pc::command::parse(&args).unwrap()
    else {
        panic!("expected recipe build --fetch");
    };
    assert!(options.fetch);
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::with_platform(|pc| {
        cli_core::recipe::build(
            &options,
            false,
            recipe::Providers(&PROVIDERS),
            &cli_core::HostSources::default(),
            &[pc],
            Some(fetcher),
            &mut cli_core::Io {
                output: &mut output,
                errors: &mut errors,
            },
        )
    });
    (
        code,
        String::from_utf8(output).unwrap(),
        String::from_utf8(errors).unwrap(),
    )
}

#[test]
fn recipe_build_fetch_prints_hash_lookup_then_builds_and_exits_0() {
    let t = Temp::new("cli-file-set-fetch");
    let bytes = b"Synthetic package bytes.";
    let recipe = file_set_recipe(&t, bytes);
    let fetcher = FakeFetcher::new(vec![("package.zip", bytes.to_vec())]);
    let (code, output, errors) = upcli_fetch(&t, &recipe, &fetcher);
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    assert!(
        output.starts_with(&format!(
            "{PACKAGE_NO_URL}\nfetch: downloading 1 input (24 bytes), as --fetch allows:\n"
        )),
        "{output}"
    );
    assert!(
        output.contains("  zip: fetched from https://example.org/dl/package.zip\n"),
        "{output}"
    );
    assert!(
        output
            .lines()
            .last()
            .unwrap()
            .starts_with("built private/media/ibm-pc/package:r1\tsha256:"),
        "{output}"
    );
    assert_eq!(
        *fetcher.requests.lock().unwrap(),
        ["https://example.org/dl/package.zip"]
    );
}

#[test]
fn recipe_build_fetch_prints_remaining_missing_input_and_exits_1() {
    let t = Temp::new("cli-partial-fetch");
    let recipe = recipe(&t, b"Missing readme.", &data());
    remove_readme_url(&recipe);
    let fetcher = FakeFetcher::new(vec![("DATA.BIN", data())]);
    let (code, output, errors) = upcli_fetch(&t, &recipe, &fetcher);
    assert_eq!((code, errors.as_str()), (1, ""), "{output}");
    assert!(
        output.starts_with(&format!(
            "{README_NO_URL}\nfetch: downloading 1 input (737280 bytes), as --fetch allows:\n"
        )),
        "{output}"
    );
    assert!(
        output.contains("  data: fetched from https://example.org/dl/DATA.BIN\n"),
        "{output}"
    );
    assert!(
        output.contains("  readme (README.TXT): missing"),
        "{output}"
    );
    assert!(!output.contains("  data (DATA.BIN): missing"), "{output}");
    assert!(
        output.ends_with("not built: missing 1 of 2 inputs; nothing was tagged\n"),
        "{output}"
    );
    assert_eq!(
        *fetcher.requests.lock().unwrap(),
        ["https://example.org/dl/DATA.BIN"]
    );
}

fn words(t: &Temp, recipe: &Path, extra: &[&str]) -> Vec<OsString> {
    let mut words: Vec<OsString> = vec!["recipe".into(), "build".into(), recipe.into()];
    words.extend(["--config".into(), t.path("preferences.toml").into()]);
    words.extend(["--store".into(), t.path("store").into()]);
    words.extend(extra.iter().map(OsString::from));
    words
}

#[test]
fn recipe_build_prints_what_it_built_and_exits_0_then_reports_it_already_built() {
    let t = Temp::new("cli");
    let readme = b"Read me first.\r\n".to_vec();
    let data = data();
    let recipe = recipe(&t, &readme, &data);
    let lib = t.path("lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(lib.join("README.TXT"), &readme).unwrap();

    // Missing: the plan, then why nothing was tagged; exit 1.
    let (code, output, errors) = upcli(&words(&t, &recipe, &["--library", lib.to_str().unwrap()]));
    assert_eq!((code, errors.as_str()), (1, ""), "{output}");
    assert!(
        output.ends_with(
            "not built: missing 1 of 2 inputs; nothing was tagged; --fetch downloads the inputs that have URLs\n"
        ),
        "{output}"
    );

    fs::write(lib.join("DATA.BIN"), &data).unwrap();
    let (code, output, errors) = upcli(&words(&t, &recipe, &["--library", lib.to_str().unwrap()]));
    assert_eq!((code, errors.as_str()), (0, ""), "{output}");
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(
        lines[..2],
        [
            "  data: kept as stored in disk-1.img",
            "  readme: kept as stored in README.TXT"
        ]
    );
    assert!(
        lines[2].starts_with("built private/media/ibm-pc/fetched:r1\tsha256:")
            && lines[2].ends_with("\tdraft"),
        "{output}"
    );
    let (code, again, _) = upcli(&words(&t, &recipe, &["--library", lib.to_str().unwrap()]));
    assert_eq!(code, 0);
    assert_eq!(
        again.lines().last().unwrap(),
        lines[2].replacen("built", "already built", 1)
    );
}

#[test]
fn recipe_build_usage_errors_exit_2() {
    let t = Temp::new("usage");
    let recipe = recipe(&t, b"x", b"y");
    for extra in [
        &["--json"][..],
        &["--fetch", "--fetch"],
        &["--keep-as"],
        &["--keep-as", "a", "--keep-as", "b"],
        &["--library"],
    ] {
        let (code, _, errors) = upcli(&words(&t, &recipe, extra));
        assert_eq!(code, 2, "{extra:?}");
        assert!(errors.starts_with("usage: unemu recipe check"), "{errors}");
    }
    let (code, _, errors) = upcli(&[
        "recipe".into(),
        "build".into(),
        t.0.clone().into(),
        "--store".into(),
        t.path("store").into(),
    ]);
    assert_eq!(code, 2);
    assert!(
        errors.contains("a folder; build takes one recipe file"),
        "{errors}"
    );
}

fn placement_recipe(operand: &str, keys: &str) -> String {
    format!(
        r#"recipe = "1.1"
[media]
name = "private/media/ibm-pc/placements"
platform = "ibm-pc"
revision = 1
[title]
title = "Placements"
[input.zip]
name = "package.zip"
size = 512
sha256 = "{hash}"
urls = ["https://example.org/dl/package.zip"]
[[step]]
id = "disk"
fat = {operand}
format = "1.44M"
{keys}
[[medium]]
id = "disk-1"
label = "Disk 1"
format = "1.44M"
from = "disk"
"#,
        hash = sha(&[42; 512]),
    )
}

fn built_volume(t: &Temp, text: &str, name: &str) -> (Vec<u8>, recipe::build::Report) {
    use std::io::Read;
    let path = t.path(&format!("{name}.media.toml"));
    fs::write(&path, text).unwrap();
    let lib = t.path("lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(lib.join("package.zip"), [42; 512]).unwrap();
    let store = image_store::Store::open(t.path(name)).unwrap();
    let (outcome, _) = build(&path, &store, &[lib], None);
    let Outcome::Built(report) = outcome else {
        panic!("{outcome:#?}")
    };
    let image = store
        .resolve(
            &report.reference,
            &image_store::media_set::Validator(&pc_image::media_set::PcMediaFormats),
        )
        .unwrap();
    let bytes = report
        .outputs
        .iter()
        .find_map(|hash| {
            let digest = format!("sha256:{hash}").parse().unwrap();
            let mut bytes = Vec::new();
            image
                .content_reader(&digest)
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            (bytes.len() == 1_474_560).then_some(bytes)
        })
        .unwrap();
    (bytes, *report)
}

#[test]
fn placements_build_identical_volumes_for_order_and_time_rules() {
    let t = Temp::new("placements-equality");
    let mut cases = Vec::new();
    for order in ["", "order = \"listed\"\n"] {
        for step_time in ["", "time = 1994-03-02T04:06:08\n"] {
            for entry_time in ["", ", time = 2001-02-03T08:10:12"] {
                let placements = format!("[{{ from = \"zip\", dos_name = \"Z.ZIP\"{entry_time} }}, {{ from = \"zip\", dos_name = \"A.ZIP\" }}]");
                let keys = format!("{order}{step_time}label = \"PACKAGES\"\nserial = 12345");
                let direct = placement_recipe(&placements, &keys);
                let legacy = placement_recipe("\"package\"", &keys)
                    .replace("recipe = \"1.1\"", "recipe = \"1.0\"")
                    .replace("[[step]]", &format!(
                        "[input.package]\nname = \"package\"\nfiles = [{{ path = \"Z.ZIP\", size = 512, sha256 = \"{hash}\"{entry_time} }}, {{ path = \"A.ZIP\", size = 512, sha256 = \"{hash}\" }}]\n[[step]]", hash = sha(&[42; 512])))
                    // The historical two-input form must use its file input too.
                    + "\n[[extra]]\nid = \"archive\"\nfile_name = \"package.zip\"\nkind = \"archive\"\nfrom = \"zip\"\n";
                let n = cases.len();
                let (actual, report) = built_volume(&t, &direct, &format!("direct-{n}"));
                let (expected, _) = built_volume(&t, &legacy, &format!("legacy-{n}"));
                assert_eq!(actual.len(), 1_474_560);
                assert_eq!(actual, expected, "{order} {step_time} {entry_time}");
                assert_eq!(report.kept.len(), 1);
                assert_eq!(report.kept[0].input, "zip");
                let json = report.plan.json();
                assert!(json["steps"][0]["operand"].is_null());
                assert_eq!(json["steps"][0]["placements"][0]["from"], "zip");
                assert_eq!(json["steps"][0]["placements"][0]["dos_name"], "Z.ZIP");
                assert_eq!(
                    json["steps"][0]["placements"][0]["time"].as_str(),
                    (!entry_time.is_empty()).then_some("2001-02-03T08:10:12")
                );
                assert_eq!(json["steps"][0]["placements"][1]["dos_name"], "A.ZIP");
                assert!(json["steps"][0]["placements"][1]["time"].is_null());
                assert!(!json.as_object().unwrap().contains_key("schema_version"));
                cases.push(actual);
            }
        }
    }
    // Both order and each timestamp choice affect bytes, so equality cannot
    // pass through an encoder that ignores the keys in both forms.
    assert_ne!(cases[0], cases[4]);
    assert_ne!(cases[0], cases[2]);
    assert_ne!(cases[0], cases[1]);
}

#[test]
fn placements_are_refused_statically_before_fetch_or_payload_reads() {
    let t = Temp::new("placements-refusals");
    let bad = [
        ("[]", "at least 1"),
        ("[{ dos_name = \"A.ZIP\" }]", "missing key `from`"),
        ("[{ from = \"zip\" }]", "missing key `dos_name`"),
        (
            "[{ from = \"zip\", dos_name = \"A.ZIP\", other = 1 }]",
            "unknown key `other`",
        ),
        (
            "[{ from = \"zip\", dos_name = \"A.ZIP\", dos_path = \"DIR/A.ZIP\" }]",
            "unknown key `dos_path`",
        ),
        ("[{ from = \"disk\", dos_name = \"A.ZIP\" }]", "file input"),
        (
            "[{ from = \"unknown\", dos_name = \"A.ZIP\" }]",
            "file input",
        ),
        (
            "[{ from = \"zip/member\", dos_name = \"A.ZIP\" }]",
            "file input",
        ),
        (
            "[{ from = \"package\", dos_name = \"A.ZIP\" }]",
            "file input",
        ),
        (
            "[{ from = \"package/A.ZIP\", dos_name = \"A.ZIP\" }]",
            "file input",
        ),
        ("[{ from = \"zip\", dos_name = \"a.zip\" }]", "refuses"),
        (
            "[{ from = \"zip\", dos_name = \"TOOLONGNAME.ZIP\" }]",
            "refuses",
        ),
        (
            "[{ from = \"zip\", dos_name = \"DIR/A.ZIP\" }]",
            "root-level",
        ),
        (
            "[{ from = \"zip\", dos_name = \"A.ZIP\" }, { from = \"zip\", dos_name = \"A.ZIP\" }]",
            "stored name",
        ),
    ];
    let store = image_store::Store::open(t.path("store")).unwrap();
    let fetcher = FakeFetcher::new(vec![("package.zip", vec![42; 512])]);
    let mut missing_findings = Vec::new();
    for (operand, finding) in bad {
        let mut text = placement_recipe(operand, "");
        if operand.contains("package") {
            text = text.replace("[[step]]", &format!("[input.package]\nname = \"package\"\nfiles = [{{ path = \"A.ZIP\", size = 512, sha256 = \"{}\" }}]\n[[step]]", sha(&[42; 512])));
        }
        let path = t.path("bad.media.toml");
        fs::write(&path, &text).unwrap();
        let (outcome, _) = build_with_fetcher(&path, &store, &[], Some(&fetcher));
        let Outcome::Refused(report) = outcome else {
            panic!("{operand}: {outcome:#?}")
        };
        if !report.findings.iter().any(|f| f.message.contains(finding)) {
            missing_findings.push(format!("{operand}: expected {finding:?}: {report:#?}"));
        }
    }
    assert!(fetcher.requests.lock().unwrap().is_empty());
    assert_eq!(tags(&store), 0);
    assert!(
        missing_findings.is_empty(),
        "{}",
        missing_findings.join("\n")
    );
}

#[test]
fn placements_require_version_one_one() {
    let text = placement_recipe("[{ from = \"zip\", dos_name = \"A.ZIP\" }]", "")
        .replace("recipe = \"1.1\"", "recipe = \"1.0\"");
    let t = Temp::new("placements-version");
    let path = t.path("old.media.toml");
    fs::write(&path, text).unwrap();
    let report = recipe::check_paths(&[path], recipe::Providers(&PROVIDERS)).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.path == "step[1].fat" && f.message.contains("requires recipe 1.1")),
        "{report:#?}"
    );
}

#[test]
fn placements_fetch_required_file_once_with_consent() {
    let t = Temp::new("placements-fetch");
    let path = t.path("fetch.media.toml");
    fs::write(
        &path,
        placement_recipe(
            "[{ from = \"zip\", dos_name = \"A.ZIP\" }, { from = \"zip\", dos_name = \"B.ZIP\" }]",
            "",
        ),
    )
    .unwrap();
    let store = image_store::Store::open(t.path("store")).unwrap();
    let (outcome, _) = build(&path, &store, &[], None);
    let Outcome::Missing(plan) = outcome else {
        panic!("{outcome:#?}")
    };
    assert_eq!(plan.inputs.len(), 1);
    assert!(!plan.inputs[0].optional);
    let (address, requests) = serve(vec![("package.zip", vec![42; 512])]);
    let (outcome, _) = build(&path, &store, &[], Some(&address));
    let Outcome::Built(report) = outcome else {
        panic!("{outcome:#?}")
    };
    assert_eq!(*requests.lock().unwrap(), ["/dl/package.zip"]);
    assert_eq!(report.kept.len(), 1);
    assert_eq!(
        fs::read(store.root().join(DOWNLOADS).join(sha(&[42; 512]))).unwrap(),
        [42; 512]
    );
}

#[test]
fn placements_fetch_and_cache_through_the_cli_with_a_fake_fetcher() {
    let t = Temp::new("placements-fake-fetch");
    let path = t.path("fetch.media.toml");
    fs::write(
        &path,
        placement_recipe(
            "[{ from = \"zip\", dos_name = \"A.ZIP\" }, { from = \"zip\", dos_name = \"B.ZIP\" }]",
            "",
        ),
    )
    .unwrap();
    let fetcher = FakeFetcher::new(vec![("package.zip", vec![42; 512])]);
    let (code, output, errors) = upcli_fetch(&t, &path, &fetcher);
    assert_eq!(code, 0, "{output}\n{errors}");
    assert_eq!(
        *fetcher.requests.lock().unwrap(),
        ["https://example.org/dl/package.zip"]
    );
    assert!(
        output.contains("fetch: downloading 1 input (512 bytes)"),
        "{output}"
    );
    assert_eq!(
        fs::read(t.path("store").join(DOWNLOADS).join(sha(&[42; 512]))).unwrap(),
        [42; 512]
    );
    assert_eq!(
        output.matches("  zip: kept as files in disk-1.img").count(),
        1,
        "{output}"
    );
}

#[test]
fn placements_schema_one_one_matches_the_generated_file() {
    let generated = recipe::schema::render_version(&[&pc_image::recipe::PcRecipes], "1.1");
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../backlog/docs/design/recipe-schema-1.1.json");
    if std::env::var_os("UNEMUPC_WRITE_RECIPE_SCHEMA").is_some() {
        fs::write(&path, &generated).unwrap();
    }
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        generated,
        "regenerate with UNEMUPC_WRITE_RECIPE_SCHEMA=1"
    );
}

#[test]
fn placements_work_file_versions_follow_the_recipe_without_restamping() {
    let t = Temp::new("placements-work-version");
    let path = t.path("disk.media.toml");
    let text = placement_recipe("[{ from = \"zip\", dos_name = \"A.ZIP\" }]", "").replace(
        "recipe = \"1.1\"",
        "recipe = \"1.1\"\nwork = \"placements\"",
    );
    fs::write(&path, &text).unwrap();
    let work = t.path("placements.work.toml");
    let old = "recipe = \"1.0\"\n[work]\nid = \"placements\"\n[title]\ntitle = \"Packages\"\n";
    let check = |path: &std::path::Path| {
        recipe::check_paths(&[path.to_path_buf()], recipe::Providers(&PROVIDERS)).unwrap()
    };
    // A 1.1 recipe may name a 1.0 work file, so one work can hold releases
    // of both versions; a 1.1 work file serves it too.
    fs::write(&work, old).unwrap();
    let report = check(&path);
    assert!(report.is_clean(), "{report:#?}");
    fs::write(&work, old.replace("1.0", "1.1")).unwrap();
    let report = check(&path);
    assert!(report.is_clean(), "{report:#?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    assert_eq!(
        fs::read_to_string(&work).unwrap(),
        old.replace("1.0", "1.1")
    );
    // A 1.0 recipe still names a 1.0 work file only.
    let frozen = t.path("frozen.media.toml");
    let frozen_text = placement_recipe("\"package\"", "").replace(
        "recipe = \"1.1\"",
        "recipe = \"1.0\"\nwork = \"placements\"",
    );
    fs::write(&frozen, &frozen_text).unwrap();
    let report = check(&frozen);
    assert!(
        format!("{report:#?}")
            .contains("a recipe names a work file of its own recipe version only"),
        "{report:#?}"
    );
}

#[test]
fn placements_bundle_inputs_are_listed_once_and_rebuild_their_lock() {
    let t = Temp::new("placements-pin");
    let path = t.path("disk.media.toml");
    let original = placement_recipe(
        "[{ from = \"zip\", dos_name = \"A.ZIP\" }, { from = \"zip\", dos_name = \"B.ZIP\" }]",
        "",
    )
    .replace(
        "urls = [\"https://example.org/dl/package.zip\"]",
        "bundle = true",
    );
    fs::write(&path, original).unwrap();
    fs::write(t.path("package.zip"), [42; 512]).unwrap();
    let listed = recipe::inputs::inputs_path(&path, recipe::Providers(&PROVIDERS)).unwrap();
    let recipe::inputs::Listing::Inputs(inputs) = listed else {
        panic!("{listed:#?}")
    };
    assert!(inputs.rows.is_empty());
    let mut args = words(&t, &path, &[]);
    args[1] = "pin".into();
    let (code, output, errors) = upcli(&args);
    assert_eq!(code, 0, "{output}\n{errors}");
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("recipe = \"1.1\""));
    assert!(text.contains("digest = \"sha256:"));
    let lock = recipe::lock::dir(&path);
    let before: Vec<_> = fs::read_dir(lock.join("sha256"))
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.file_name(), fs::read(e.path()).unwrap())
        })
        .collect();
    let store = image_store::Store::open(t.path("rebuilt")).unwrap();
    let (outcome, _) = build(&path, &store, &[], None);
    let Outcome::Built(report) = outcome else {
        panic!("{outcome:#?}")
    };
    assert!(report.locked);
    assert_eq!(report.plan.inputs.len(), 1);
    assert_eq!(report.kept.len(), 1);
    assert_eq!(report.plan.inputs[0].used_by.len(), 1);
    assert_eq!(fs::read_to_string(&path).unwrap(), text);
    for (name, bytes) in before {
        assert_eq!(fs::read(lock.join("sha256").join(name)).unwrap(), bytes);
    }
}

fn machine_recipe(t: &Temp) -> PathBuf {
    let text = format!(
        "recipe = \"1.1\"\n[machine]\nname = \"unemu/machines/ibm-pc/open-at\"\nplatform = \"ibm-pc\"\nversion = \"1.3.0\"\n\n\
         [template]\nconfig = \"config\"\ndisks = [{{ id = \"system\", size = 512 }}]\n\n\
         [license]\nexpression = \"CC0-1.0\"\n\n\
         [input.config]\nname = \"open-at.toml\"\nsize = 1\nsha256 = \"{}\"\nbundle = true\n\n\
         [use.bios]\nimage = \"private/firmware/ibm-pc/bios:r1\"\ndigest = \"sha256:{}\"\n",
        sha(b"x"), "0".repeat(64)
    );
    let path = t.path("open-at.machine.toml");
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn machine_plan_resolves_config_and_reports_pinned_uses_and_disks() {
    let t = Temp::new("machine-plan");
    let path = machine_recipe(&t);
    image_store::Store::open(t.path("store")).unwrap();
    for bundled in [false, true] {
        if bundled {
            fs::write(t.path("open-at.toml"), b"x").unwrap();
        }
        for json in [false, true] {
            let mut args = words(&t, &path, &[]);
            args[1] = "plan".into();
            if json {
                args.push("--json".into());
            }
            let (code, output, errors) = upcli(&args);
            assert_eq!(code, 1, "{output}{errors}");
            assert!(errors.is_empty(), "{errors}");
            assert!(
                output.contains("unemu/machines/ibm-pc/open-at:1.3.0"),
                "{output}"
            );
            assert!(
                output.contains("private/firmware/ibm-pc/bios:r1"),
                "{output}"
            );
            assert!(output.contains("sha256:000000"), "{output}");
            assert!(
                output.contains("system") && output.contains("512"),
                "{output}"
            );
            assert!(output.contains("--fetch"), "{output}");
            assert!(!output.contains("schema_version"));
            if json {
                assert!(
                    output.contains("\"uses\"") && output.contains("\"disks\""),
                    "{output}"
                );
                assert!(
                    output.contains(if bundled { "\"bundle\"" } else { "\"missing\"" }),
                    "{output}"
                );
            } else {
                assert!(
                    output.contains(if bundled {
                        "beside the recipe"
                    } else {
                        "missing"
                    }),
                    "{output}"
                );
            }
        }
    }
}

#[test]
fn machine_build_without_consent_reports_missing_uses_without_network() {
    let t = Temp::new("machine-consent");
    let path = machine_recipe(&t);
    fs::write(t.path("open-at.toml"), b"x").unwrap();
    let (code, output, errors) = upcli(&words(&t, &path, &[]));
    assert_eq!(code, 1, "{output}{errors}");
    assert!(
        output.contains("use bios") && output.contains("--fetch"),
        "{output}{errors}"
    );
    assert!(!output.contains("machine templates are not built yet"));
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(tags(&image_store::Store::open(t.path("store")).unwrap()), 0);
}

#[test]
fn names_guard_publishes_after_the_whole_namespace_without_network() {
    for version in ["1.0", "1.1"] {
        let t = Temp::new(&format!("names-guard-{version}"));
        let path = recipe(&t, &[b'n'; 300], &data());
        let mut text = fs::read_to_string(&path).unwrap();
        if version == "1.1" {
            text = text
                .replace("recipe = \"1.0\"", "recipe = \"1.1\"")
                .replace(
                    "private/media/ibm-pc/fetched",
                    "private/experimental/media/ibm-pc/fetched",
                );
        }
        fs::write(&path, text).unwrap();
        let lib = t.path("lib");
        fs::create_dir_all(&lib).unwrap();
        fs::write(lib.join("README.TXT"), [b'n'; 300]).unwrap();
        fs::write(lib.join("DATA.BIN"), data()).unwrap();
        let mut args = words(&t, &path, &["--library", lib.to_str().unwrap()]);
        args[1] = "pin".into();
        let (code, output, errors) = upcli(&args);
        assert_eq!(code, 0, "{output}{errors}");
        let next = recipe::compat::NEXT_RELEASE;
        let levels = image_store::compat::Levels::new(Some((next, next)), next, next);
        let artifact =
            recipe::artifact::bundle_with_levels(&path, recipe::Providers(&PROVIDERS), levels)
                .unwrap();
        let namespaces = if version == "1.1" {
            ["private/experimental", "unemu/team/review"]
        } else {
            ["private", "unemu"]
        };
        for namespace in namespaces {
            let target = format!("{namespace}/recipes/media/ibm-pc/fetched");
            recipe::artifact::guard(
                &artifact,
                recipe::Providers(&PROVIDERS),
                &target,
                "r1",
                &t.path("scratch"),
            )
            .unwrap();
        }
        for wrong in [
            "private/recipes/experimental/media/ibm-pc/fetched",
            "private/recipes/media/ibm-pc/other",
            "private/media/recipes/media/ibm-pc/fetched",
        ] {
            assert!(
                matches!(
                    recipe::artifact::guard(
                        &artifact,
                        recipe::Providers(&PROVIDERS),
                        wrong,
                        "r1",
                        &t.path("bad-scratch")
                    ),
                    Err(recipe::artifact::Refusal::Name { .. })
                ),
                "{wrong}"
            );
        }
    }
}

#[test]
fn machine_core_without_bindings_is_refused_before_fetch() {
    use unemu_core_api::{Core, CoreDescription, CreateRequest, Error, Machine};
    struct NoBindings(CoreDescription);
    impl Core for NoBindings {
        fn description(&self) -> &CoreDescription {
            &self.0
        }
        fn create(&self, _: &CreateRequest<'_>) -> Result<Box<dyn Machine>, Error> {
            panic!("must not create a machine")
        }
    }
    let mut description = *unemu_pc_core::PcCore.description();
    let mut platforms = description.platforms.to_vec();
    for platform in &mut platforms {
        platform.content_bindings = false;
    }
    description.platforms = Box::leak(platforms.into_boxed_slice());
    let core = Box::leak(Box::new(NoBindings(description)));
    let cores: &'static [&'static dyn Core] = Box::leak(vec![core as &dyn Core].into_boxed_slice());
    let t = Temp::new("machine-capability");
    let path = machine_recipe(&t);
    let mut args = words(&t, &path, &["--fetch"]);
    args[1] = "build".into();
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::with_platform(|pc| {
        cli_core::run(
            &args,
            &mut cli_core::Io {
                output: &mut output,
                errors: &mut errors,
            },
            &cli_core::HostSources::default(),
            &[pc],
            &core_launch::Cores::new(cores),
        )
    });
    assert_ne!(code, 0);
    let errors = String::from_utf8(errors).unwrap();
    assert!(
        errors.contains("this core cannot compose templates"),
        "{errors}"
    );
    assert!(output.is_empty(), "{}", String::from_utf8_lossy(&output));
}

fn seed_template_media(
    registry: &image_registry::test_support::MockRegistry,
    repository: &str,
    marker: u8,
) -> (image_store::Digest, image_store::Manifest) {
    use image_store::{media_type as mt, Descriptor, Manifest};
    let bytes = vec![marker; 737_280];
    let medium = Descriptor::new(
        mt::FLOPPY,
        registry.seed_blob(repository, &bytes),
        bytes.len() as u64,
    );
    let config = image_store::media_set::Config {
        schema: 1,
        platform: "ibm-pc".into(),
        media: vec![image_store::media_set::Medium {
            id: "disk-1".into(),
            label: "Disk 1".into(),
            format: pc_image::media_set::format("720K").unwrap(),
            blob: medium.digest.clone(),
        }],
        extras: Vec::new(),
    }
    .encode()
    .unwrap();
    let config = Descriptor::new(
        mt::MEDIA_SET_CONFIG,
        registry.seed_blob(repository, &config),
        config.len() as u64,
    );
    let metadata = br#"{"schema":1,"platform":"ibm-pc","titles":[]}"#;
    let metadata = Descriptor::new(
        mt::METADATA,
        registry.seed_blob(repository, metadata),
        metadata.len() as u64,
    );
    let manifest = Manifest::new_media_set("ibm-pc", config, vec![medium, metadata]).unwrap();
    (
        registry.seed_manifest(repository, "1-r1", &manifest.encode().unwrap()),
        manifest,
    )
}

fn registry_machine(
    t: &Temp,
    host: &str,
    repository: &str,
    digest: &image_store::Digest,
) -> PathBuf {
    let path = machine_recipe(t);
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace(
            "private/firmware/ibm-pc/bios:r1",
            &format!("{repository}:1-r1"),
        )
        .replace(&format!("sha256:{}", "0".repeat(64)), &digest.to_string());
    fs::write(&path, text).unwrap();
    fs::write(t.path("open-at.toml"), b"x").unwrap();
    fs::write(
        t.path("preferences.toml"),
        format!("default_registry='{host}'\n[[registries]]\nhost='{host}'\nplain_http=true\n"),
    )
    .unwrap();
    path
}

#[test]
fn machine_fetch_uses_digest_despite_moved_tag_and_preserves_descriptor_order() {
    use image_registry::test_support::{MockRegistry, Policy};
    let registry = MockRegistry::start(Policy {
        anonymous_read: vec!["*".into()],
        ..Default::default()
    });
    let repository = "private/media/ibm-pc/disks";
    let (digest, manifest) = seed_template_media(&registry, repository, 17);
    let (moved, _) = seed_template_media(&registry, repository, 99);
    assert_ne!(digest, moved);
    let t = Temp::new("machine-fetch-digest");
    let path = registry_machine(&t, &registry.host(), repository, &digest);
    let (code, output, errors) = upcli(&words(&t, &path, &[]));
    assert_eq!(code, 1, "{output}{errors}");
    assert!(output.contains("missing"), "{output}{errors}");
    assert!(registry.log().is_empty(), "{:?}", registry.log());
    let mut plan = words(&t, &path, &["--json"]);
    plan[1] = "plan".into();
    let (code, output, errors) = upcli(&plan);
    assert_eq!(code, 1, "{output}{errors}");
    assert!(registry.log().is_empty(), "{:?}", registry.log());
    let (code, output, errors) = upcli(&words(&t, &path, &["--fetch"]));
    assert_eq!(code, 1, "{output}{errors}");
    assert!(
        format!("{output}{errors}").contains("not built:"),
        "{output}{errors}"
    );
    let log = registry.log();
    assert!(
        log.contains(&format!("GET /v2/{repository}/manifests/{digest}")),
        "{log:?}"
    );
    assert!(
        !log.iter()
            .any(|r| r.ends_with("/manifests/1-r1") || r.contains("tags/list")),
        "{log:?}"
    );
    let blobs: Vec<_> = log
        .iter()
        .filter(|r| r.starts_with("GET ") && r.contains("/blobs/"))
        .cloned()
        .collect();
    let expected: Vec<_> = std::iter::once(&manifest.config)
        .chain(&manifest.layers)
        .map(|d| format!("GET /v2/{repository}/blobs/{}", d.digest))
        .collect();
    assert_eq!(blobs, expected);
    let store = image_store::Store::open(t.path("store")).unwrap();
    assert_eq!(tags(&store), 0);
    let (code, output, errors) = upcli(&plan);
    assert_eq!(code, 0, "{output}{errors}");
    assert!(output.contains("\"present\""), "{output}");
    registry.clear_log();
    let (code, output, errors) = upcli(&words(&t, &path, &[]));
    assert_eq!(code, 1, "{output}{errors}");
    assert!(format!("{output}{errors}").contains("not built:"));
    assert!(registry.log().is_empty());
}

#[test]
fn machine_fetch_refuses_digest_type_platform_and_blob_mismatches() {
    use image_registry::test_support::{MockRegistry, Policy};
    for case in ["digest", "type", "platform", "blob", "unreadable"] {
        let registry = MockRegistry::start(Policy {
            anonymous_read: vec!["*".into()],
            ..Default::default()
        });
        let repository = "private/media/ibm-pc/disks";
        let (mut digest, mut manifest) = seed_template_media(&registry, repository, 42);
        match case {
            "digest" => {
                digest = format!("sha256:{}", "0".repeat(64)).parse().unwrap();
                registry.forge_manifest(repository, "1-r1", &digest, &manifest.encode().unwrap());
            }
            "type" => {
                let bytes = String::from_utf8(manifest.encode().unwrap())
                    .unwrap()
                    .replace(
                        image_store::media_type::MEDIA_SET,
                        image_store::media_type::MACHINE,
                    )
                    .replace(
                        image_store::media_type::MEDIA_SET_CONFIG,
                        image_store::media_type::CONFIG,
                    );
                digest = registry.seed_manifest(repository, "1-r1", bytes.as_bytes());
            }
            "platform" => {
                manifest
                    .annotations
                    .insert("unemu.platform".into(), "nes".into());
                digest = registry.seed_manifest(repository, "1-r1", &manifest.encode().unwrap());
            }
            "blob" => registry.corrupt_blob(&manifest.layers[0].digest),
            "unreadable" => {
                let bytes = b"not a package";
                manifest.config = image_store::Descriptor::new(
                    image_store::media_type::MEDIA_SET_CONFIG,
                    registry.seed_blob(repository, bytes),
                    bytes.len() as u64,
                );
                digest = registry.seed_manifest(repository, "1-r1", &manifest.encode().unwrap());
            }
            _ => unreachable!(),
        }
        let t = Temp::new(&format!("machine-refusal-{case}"));
        let path = registry_machine(&t, &registry.host(), repository, &digest);
        let (code, output, errors) = upcli(&words(&t, &path, &["--fetch"]));
        assert_ne!(code, 0, "{case}: {output}{errors}");
        let refusal = format!("{output}{errors}");
        assert!(
            !refusal.contains("machine templates are not built yet"),
            "{case}: {refusal}"
        );
        assert!(
            refusal.contains(match case {
                "digest" => "registry content does not match its digest",
                "blob" => "does not match",
                "type" => "artifact type or platform mismatch",
                "platform" => "platform",
                _ => "invalid",
            }),
            "{case}: {refusal}"
        );
        if ["digest", "type", "platform"].contains(&case) {
            assert!(
                !registry.log().iter().any(|r| r.contains("/blobs/")),
                "{:?}",
                registry.log()
            );
        }
        assert_eq!(tags(&image_store::Store::open(t.path("store")).unwrap()), 0);
    }
}

#[test]
fn machine_registry_fixture_is_portable_and_selects_the_configured_registry() {
    let t = Temp::new("machine-registry-config");
    let repository = "private/media/ibm-pc/disks";
    let digest = image_store::Digest::of(b"pinned manifest");
    let mut previous = None;
    for host in ["127.0.0.1:5000", "127.0.0.1:5001"] {
        let path = registry_machine(&t, host, repository, &digest);
        let checked =
            recipe::check_paths(std::slice::from_ref(&path), recipe::Providers(&PROVIDERS))
                .unwrap();
        assert!(checked.is_clean(), "{}", checked.render());
        let planned = recipe::plan_path(
            &path,
            recipe::Providers(&PROVIDERS),
            recipe::plan::Sources {
                store: None,
                downloads: None,
                libraries: &mut [],
            },
        )
        .unwrap();
        let recipe::Planned::Plan(plan) = planned else {
            panic!("expected a machine plan");
        };
        let usage = &plan.uses[0];
        assert_eq!(usage.image, format!("{repository}:1-r1"));
        assert_eq!(usage.digest, digest.to_string());
        let preferences = fs::read_to_string(t.path("preferences.toml")).unwrap();
        let config = image_registry::RegistryConfig::from_toml(&preferences).unwrap();
        let default_host = config.default_host(None, None).unwrap();
        assert_eq!(default_host, host);
        assert!(config.entry(host).unwrap().plain_http);
        let resolved = image_registry::Reference::parse(&usage.image, &default_host).unwrap();
        assert_eq!(resolved.host, host);
        assert_eq!(resolved.repository, repository);
        let text = fs::read_to_string(path).unwrap();
        if let Some(previous) = previous {
            assert_eq!(text, previous);
        }
        previous = Some(text);
    }
}

#[path = "../../pc-image/tests/support/template.rs"]
mod template_support;

#[test]
fn machine_draft_cli_builds_and_tags_portable_config_without_guest_execution() {
    for source in &pc_image::template::FIRMWARE {
        for relative in [source.rom, source.source_archive] {
            if pc_harness::test_support::fixtures::fixture(relative).is_none() {
                return;
            }
        }
    }
    let t = Temp::new("machine-draft");
    let store = image_store::Store::open(t.path("store"))
        .unwrap()
        .with_compatibility(pc_image::compat::compatibility(
            pc_image::compat::NEXT_RELEASE,
        ));
    let path = template_support::template_recipe(&store, &t.0);
    let (code, output, errors) = upcli(&words(&t, &path, &[]));
    assert_eq!(code, 0, "{output}{errors}");
    assert!(errors.is_empty(), "{errors}");
    assert!(
        output.contains("built private/machines/ibm-pc/bound:1.3.0"),
        "{output}"
    );
    let image = store
        .resolve("private/machines/ibm-pc/bound:1.3.0", &pc_image::PcProvider)
        .unwrap();
    assert_eq!(
        image.manifest.config.media_type,
        image_store::media_type::PC_LAUNCH_CONFIG
    );
    assert!(!store.root().join("workspaces").exists());
    let (code, output, errors) = upcli(&words(&t, &path, &[]));
    assert_eq!(code, 0, "{output}{errors}");
    assert!(output.contains("already built"), "{output}");
    let text = fs::read_to_string(&path).unwrap().replace(
        "version = \"1.3.0\"",
        &format!("version = \"1.3.0\"\ndigest = \"{}\"", image.image.digest),
    );
    fs::write(&path, text).unwrap();
    let (code, output, errors) = upcli(&words(&t, &path, &[]));
    assert_ne!(code, 0, "{output}{errors}");
    assert!(
        format!("{output}{errors}").contains("pinned machine recipes require machine lock support"),
        "{output}{errors}"
    );
    assert_eq!(
        store
            .tag_descriptor("private/machines/ibm-pc/bound:1.3.0")
            .unwrap()
            .digest,
        image.image.digest
    );
}
