// SPDX-License-Identifier: AGPL-3.0-or-later
//! The launch path over host files: which cores are asked and in what
//! order, each outcome of asking them, the check of the config, and the
//! machine's records under the content's digest. The test core stands for
//! a whole core; scripted cores below answer what it cannot (several cores
//! for one file, content that needs more than itself).

use super::*;
use crate::{Cores, Filter};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use unemu_core_api::{
    ContentKind, ContentRef, ContractVersion, CoreDescription, PlatformDescription, Validated,
};
use unemu_test_core::{TestCore, BATTERY_RECORD};

/// A directory of its own for one test, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "core-launch-tests-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(path.join("files")).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join("files").join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn dirs(&self) -> HostDirs {
        HostDirs {
            saves: self.0.join("saves"),
            states: self.0.join("states"),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

thread_local! {
    /// The ids of the scripted cores asked on this thread, in order.
    static ASKED: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

fn take_asked() -> Vec<&'static str> {
    ASKED.with(|asked| std::mem::take(&mut *asked.borrow_mut()))
}

const CONFIG: &str = "application/vnd.unemu.scripted-launch";

/// What a scripted core answers about any file.
#[derive(Clone, Copy)]
enum Answer {
    /// Not mine.
    Nothing,
    /// Recognised, cannot run it, for this reason.
    Unsupported(&'static str),
    /// Runs it; `validate` lists what `Needs` says.
    Runs(Needs),
    /// Runs it, as a content kind its platform does not list.
    OtherKind,
    /// Fails with this kind.
    Fails(ErrorKind),
}

/// What `create` of a scripted core does.
#[derive(Clone, Copy)]
enum Create {
    /// Refuses: a scripted core builds no machine.
    Refuse,
    /// Opens a record under a name no storage takes.
    BadName,
    /// Reads past the end of an empty record.
    PastTheEnd,
}

/// What `validate` lists for the config a scripted core wrote.
#[derive(Clone, Copy)]
enum Needs {
    /// The file, named twice.
    TheFile,
    /// The file and more content with these labels.
    More(&'static [&'static str]),
    /// The file's digest at another size.
    OtherSize,
    /// Nothing at all.
    Nothing,
}

struct Scripted {
    description: CoreDescription,
    answer: Answer,
    create: Create,
}

impl Scripted {
    const fn new(id: &'static str, name: &'static str, answer: Answer) -> Self {
        Self {
            description: CoreDescription::new(ContractVersion::new(0, 6), id, name, "1.0.0"),
            answer,
            create: Create::Refuse,
        }
    }

    const fn with_create(mut self, create: Create) -> Self {
        self.create = create;
        self
    }

    const fn with_platforms(mut self, platforms: &'static [PlatformDescription]) -> Self {
        self.description = self.description.with_platforms(platforms);
        self
    }
}

/// The config of a scripted core: the digest, then the size.
fn scripted_config(sha256: &[u8; 32], size: u64) -> Vec<u8> {
    let mut bytes = sha256.to_vec();
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes
}

impl Core for Scripted {
    fn description(&self) -> &CoreDescription {
        &self.description
    }

    fn create(&self, request: &CreateRequest<'_>) -> Result<Box<dyn Machine>, Error> {
        match self.create {
            Create::Refuse => {}
            Create::BadName => {
                request.storage.open("Not_A_Record")?;
            }
            Create::PastTheEnd => {
                let mut record = request.storage.open("memory")?;
                record.read_at(16, &mut [0; 4])?;
            }
        }
        Err(Error::new(ErrorKind::Unsupported, "a scripted core"))
    }

    fn identify(&self, content: &OfferedContent<'_>) -> Result<Option<Identified>, Error> {
        ASKED.with(|asked| asked.borrow_mut().push(self.description.id));
        match self.answer {
            Answer::Nothing => Ok(None),
            Answer::Unsupported(reason) => Err(Error::new(ErrorKind::Unsupported, reason)),
            Answer::Fails(kind) => Err(Error::new(kind, "the scripted core failed")),
            Answer::Runs(_) | Answer::OtherKind => Ok(Some(Identified::new(
                self.description.platforms[0].id,
                match self.answer {
                    Answer::OtherKind => "tape",
                    _ => self.description.platforms[0].content[0].id,
                },
                ConfigDocumentBuf::new(CONFIG, scripted_config(&content.sha256, content.size)),
            ))),
        }
    }

    fn validate(
        &self,
        _platform: &str,
        config: unemu_core_api::ConfigDocument<'_>,
    ) -> Result<Validated, Error> {
        let sha256: [u8; 32] = config.bytes[..32].try_into().unwrap();
        let size = u64::from_le_bytes(config.bytes[32..40].try_into().unwrap());
        let file = ContentRef::new(sha256, size, "cartridge");
        let content = match self.answer {
            Answer::Runs(Needs::TheFile) => vec![file.clone(), file],
            Answer::Runs(Needs::More(labels)) => std::iter::once(file)
                .chain(
                    labels
                        .iter()
                        .map(|label| ContentRef::new([0xaa; 32], 8192, *label)),
                )
                .collect(),
            Answer::Runs(Needs::OtherSize) => vec![ContentRef::new(sha256, size + 1, "cartridge")],
            _ => Vec::new(),
        };
        Ok(Validated::new(content))
    }
}

static NES_KINDS: [ContentKind; 1] = [ContentKind::new("ines", "NES cartridge images", &["nes"])];
static NES: [PlatformDescription; 1] = [PlatformDescription::new("nes", "NES")
    .with_config_types(&[CONFIG])
    .with_content(&NES_KINDS)];
static ALPHA_KINDS: [ContentKind; 1] = [ContentKind::new("alpha", "Alpha things", &["aaa"])];
static ALPHA: [PlatformDescription; 1] = [PlatformDescription::new("alpha", "Alpha")
    .with_config_types(&[CONFIG])
    .with_content(&ALPHA_KINDS)];
static DISK_KINDS: [ContentKind; 2] = [
    ContentKind::new("fds", "Disk images", &["fds", "qd"]),
    ContentKind::new("tape", "Tape images", &["tap", "fds"]),
];
static DISK: [PlatformDescription; 1] = [PlatformDescription::new("disk", "Disk system")
    .with_config_types(&[CONFIG])
    .with_content(&DISK_KINDS)];

static ALPHA_NONE: Scripted =
    Scripted::new("alpha", "Alpha core", Answer::Nothing).with_platforms(&ALPHA);
static NES_NONE: Scripted =
    Scripted::new("nes-none", "Quiet NES core", Answer::Nothing).with_platforms(&NES);
static NES_NONE_TOO: Scripted =
    Scripted::new("nes-none-too", "Another quiet NES core", Answer::Nothing).with_platforms(&NES);
static NO_PLATFORM: Scripted = Scripted::new("bare", "Bare core", Answer::Nothing);
static UNEMUNES: Scripted =
    Scripted::new("unemunes", "unemunes", Answer::Runs(Needs::TheFile)).with_platforms(&NES);
static NES_TWO: Scripted =
    Scripted::new("nes-two", "Second NES core", Answer::Runs(Needs::TheFile)).with_platforms(&NES);
static PICKY: Scripted = Scripted::new(
    "picky",
    "Picky core",
    Answer::Unsupported("cartridge board 5 (MMC5) is not modelled"),
)
.with_platforms(&NES);
static FIRMWARE: Scripted = Scripted::new(
    "fds",
    "Disk core",
    Answer::Runs(Needs::More(&["firmware", "bios"])),
)
.with_platforms(&DISK);
static OTHER_SIZE: Scripted =
    Scripted::new("sized", "Sized core", Answer::Runs(Needs::OtherSize)).with_platforms(&NES);
static NAMES_NOTHING: Scripted =
    Scripted::new("empty", "Empty core", Answer::Runs(Needs::Nothing)).with_platforms(&NES);
static TWO_KINDS: Scripted =
    Scripted::new("disk-two", "Disk core", Answer::Nothing).with_platforms(&DISK);
/// A core of another contract that would open `.nes` files.
static OLD_NES: Scripted = Scripted {
    description: CoreDescription::new(ContractVersion::new(0, 1), "old", "Old core", "0.1.0")
        .with_platforms(&NES),
    answer: Answer::Runs(Needs::TheFile),
    create: Create::Refuse,
};
static BROKEN: Scripted =
    Scripted::new("broken", "Broken core", Answer::Fails(ErrorKind::Config)).with_platforms(&NES);
static CHANGED: Scripted =
    Scripted::new("changed", "Changed core", Answer::Fails(ErrorKind::Content))
        .with_platforms(&NES);
static UNREADABLE: Scripted = Scripted::new(
    "unreadable",
    "Unreadable core",
    Answer::Fails(ErrorKind::Storage),
)
.with_platforms(&NES);
static WRONG_KIND: Scripted =
    Scripted::new("wrong-kind", "Wrong kind core", Answer::OtherKind).with_platforms(&NES);
static BAD_ID: Scripted =
    Scripted::new("../escape", "Escaping core", Answer::Runs(Needs::TheFile)).with_platforms(&NES);
static BAD_NAME: Scripted =
    Scripted::new("bad-name", "Bad name core", Answer::Runs(Needs::TheFile))
        .with_platforms(&NES)
        .with_create(Create::BadName);
static PAST_THE_END: Scripted = Scripted::new(
    "past-the-end",
    "Past the end core",
    Answer::Runs(Needs::TheFile),
)
.with_platforms(&NES)
.with_create(Create::PastTheEnd);

const SCRIPT: &[u8] =
    b"unemu-test-core 1\ntitle Two players\nnotice the header says PAL\nbattery\n";

fn sha256_of(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn ready(outcome: Result<Outcome, LaunchError>) -> Prepared {
    match outcome {
        Ok(Outcome::Ready(prepared)) => prepared,
        Ok(Outcome::Choose(_)) => panic!("several cores answered"),
        Err(error) => panic!("refused: {error}"),
    }
}

fn refusal(outcome: Result<Outcome, LaunchError>) -> LaunchError {
    match outcome {
        Err(error) => error,
        Ok(_) => panic!("the file was not refused"),
    }
}

/// `root/unemu-test-core/aa/DIGEST` for the test script.
fn script_dir(root: &Path) -> PathBuf {
    let digest = hex(&sha256_of(SCRIPT));
    root.join("unemu-test-core")
        .join(&digest[..2])
        .join(&digest)
}

fn battery_file(prepared: &Prepared) -> PathBuf {
    prepared
        .saves_dir()
        .unwrap()
        .join(unemu_core_files::file_name(BATTERY_RECORD).unwrap())
}

#[test]
fn cores_that_list_the_extension_are_asked_first_then_the_others_in_list_order() {
    static LIST: [&dyn Core; 4] = [&ALPHA_NONE, &NES_NONE, &NO_PLATFORM, &NES_NONE_TOO];
    let cores = Cores::new(&LIST);
    let scratch = Scratch::new();
    take_asked();
    // The extension is compared in lower case.
    let game = scratch.file("GAME.NES", b"not a cartridge");
    refusal(prepare(&game, &cores, None, &scratch.dirs()));
    assert_eq!(take_asked(), ["nes-none", "nes-none-too", "alpha", "bare"]);
    let alpha = scratch.file("thing.aaa", b"x");
    refusal(prepare(&alpha, &cores, None, &scratch.dirs()));
    assert_eq!(take_asked(), ["alpha", "nes-none", "bare", "nes-none-too"]);
    // No core lists it, or there is no extension: the list's order.
    for name in ["notes.txt", "README"] {
        let other = scratch.file(name, b"x");
        refusal(prepare(&other, &cores, None, &scratch.dirs()));
        assert_eq!(
            take_asked(),
            ["alpha", "nes-none", "bare", "nes-none-too"],
            "{name}"
        );
    }
}

#[test]
fn a_named_core_is_the_only_one_asked() {
    static LIST: [&dyn Core; 3] = [&NES_NONE, &ALPHA_NONE, &UNEMUNES];
    let cores = Cores::new(&LIST);
    let scratch = Scratch::new();
    let game = scratch.file("game.nes", b"cartridge");
    take_asked();
    let prepared = ready(prepare(&game, &cores, Some("unemunes"), &scratch.dirs()));
    assert_eq!(take_asked(), ["unemunes"]);
    assert_eq!(prepared.core().description().id, "unemunes");

    let error = refusal(prepare(&game, &cores, Some("alpha"), &scratch.dirs()));
    assert_eq!(take_asked(), ["alpha"]);
    assert_eq!(
        error.to_string(),
        "The core Alpha core does not open game.nes. It opens: Alpha things (*.aaa)."
    );
}

#[test]
fn a_named_core_the_program_cannot_drive_is_refused_before_anything_is_asked() {
    static LIST: [&dyn Core; 3] = [&OLD_NES, &NES_NONE, &ALPHA_NONE];
    let cores = Cores::new(&LIST);
    let scratch = Scratch::new();
    let game = scratch.file("game.nes", b"cartridge");
    take_asked();
    let error = refusal(prepare(&game, &cores, Some("nobody"), &scratch.dirs()));
    assert_eq!(
        error.to_string(),
        "This program has no core nobody. Its cores: nes-none, alpha."
    );
    // A core that is compiled in and drives another contract says so.
    let error = refusal(prepare(&game, &cores, Some("old"), &scratch.dirs()));
    assert_eq!(
        error.to_string(),
        "core Old core implements contract 0.1; this program needs 0.6"
    );
    assert!(take_asked().is_empty());
    // A refused core is never asked, also without a name.
    refusal(prepare(&game, &cores, None, &scratch.dirs()));
    assert_eq!(take_asked(), ["nes-none", "alpha"]);
}

#[test]
fn a_file_no_core_opens_is_refused_with_what_the_cores_open() {
    static LIST: [&dyn Core; 5] = [&ALPHA_NONE, &TestCore, &NO_PLATFORM, &NES_NONE, &OLD_NES];
    let cores = Cores::new(&LIST);
    let scratch = Scratch::new();
    let notes = scratch.file("holiday-notes.txt", b"Dear diary");
    let error = refusal(prepare(&notes, &cores, None, &scratch.dirs()));
    assert_eq!(
        error,
        LaunchError::NoCore {
            file: "holiday-notes.txt".to_owned(),
            kinds: vec![
                "Alpha things (*.aaa)".to_owned(),
                "unemu test core scripts (*.utc)".to_owned(),
                "NES cartridge images (*.nes)".to_owned(),
            ],
        }
    );
    assert_eq!(
        error.to_string(),
        "No core in this program opens holiday-notes.txt. This program opens: Alpha things (*.aaa); unemu test core scripts (*.utc); NES cartridge images (*.nes)."
    );
}

#[test]
fn a_core_that_recognises_the_file_and_cannot_run_it_is_heard() {
    static LIST: [&dyn Core; 3] = [&ALPHA_NONE, &PICKY, &NES_NONE];
    let cores = Cores::new(&LIST);
    let scratch = Scratch::new();
    let game = scratch.file("odd-board.nes", b"cartridge");
    let error = refusal(prepare(&game, &cores, None, &scratch.dirs()));
    assert_eq!(
        error.to_string(),
        "The core Picky core recognised odd-board.nes and cannot run it: cartridge board 5 (MMC5) is not modelled"
    );

    // The test core's own refusal of a script it cannot run.
    static TEST: [&dyn Core; 1] = [&TestCore];
    let script = scratch.file(
        "board.utc",
        b"unemu-test-core 1\nunsupported no such board\n",
    );
    let error = refusal(prepare(&script, &Cores::new(&TEST), None, &scratch.dirs()));
    assert_eq!(
        error,
        LaunchError::Unsupported {
            file: "board.utc".to_owned(),
            core: "unemu test core".to_owned(),
            reason: "no such board".to_owned(),
        }
    );
}

#[test]
fn a_core_that_runs_the_file_wins_over_one_that_cannot() {
    static LIST: [&dyn Core; 2] = [&PICKY, &UNEMUNES];
    let scratch = Scratch::new();
    let game = scratch.file("game.nes", b"cartridge");
    let prepared = ready(prepare(&game, &Cores::new(&LIST), None, &scratch.dirs()));
    assert_eq!(prepared.core().description().id, "unemunes");
}

#[test]
fn one_core_that_runs_the_file_makes_it_ready() {
    static LIST: [&dyn Core; 2] = [&ALPHA_NONE, &TestCore];
    let scratch = Scratch::new();
    let path = scratch.file("two-players.utc", SCRIPT);
    let prepared = ready(prepare(&path, &Cores::new(&LIST), None, &scratch.dirs()));
    assert_eq!(prepared.core().description().id, "unemu-test-core");
    assert_eq!(prepared.platform(), "unemu-test");
    assert_eq!(prepared.platform_name(), "Test machine");
    assert_eq!(prepared.kind(), Some("script"));
    assert_eq!(prepared.file_name(), Some("two-players.utc"));
    assert_eq!(prepared.title(), "Two players");
    assert_eq!(
        prepared
            .notes()
            .iter()
            .map(|note| note.text.as_str())
            .collect::<Vec<_>>(),
        ["the header says PAL"]
    );
    assert_eq!(prepared.sha256(), Some(&sha256_of(SCRIPT)));
    assert_eq!(prepared.size(), Some(SCRIPT.len() as u64));
    assert_eq!(prepared.config().media_type, unemu_test_core::CONFIG_TYPE);
    assert_eq!(
        prepared.saves_dir(),
        Some(script_dir(&scratch.0.join("saves")).as_path())
    );
    assert_eq!(
        prepared.states_dir(),
        Some(script_dir(&scratch.0.join("states")).as_path())
    );
    // Preparing writes nothing.
    assert!(!scratch.0.join("saves").exists());
    assert!(!scratch.0.join("states").exists());
}

#[test]
fn a_file_without_a_title_of_its_own_is_titled_by_its_name() {
    static LIST: [&dyn Core; 1] = [&TestCore];
    let scratch = Scratch::new();
    let path = scratch.file("plain.utc", b"unemu-test-core 1\n");
    let prepared = ready(prepare(&path, &Cores::new(&LIST), None, &scratch.dirs()));
    assert_eq!(prepared.title(), "plain.utc");
}

#[test]
fn the_whole_file_is_hashed_however_large() {
    static LIST: [&dyn Core; 1] = [&UNEMUNES];
    let scratch = Scratch::new();
    // Two whole reads and a part of a third.
    let bytes: Vec<u8> = (0..CHUNK * 2 + 12_345).map(|i| (i % 251) as u8).collect();
    let path = scratch.file("large.nes", &bytes);
    let prepared = ready(prepare(&path, &Cores::new(&LIST), None, &scratch.dirs()));
    assert_eq!(prepared.sha256(), Some(&sha256_of(&bytes)));
    assert_eq!(prepared.size(), Some(bytes.len() as u64));

    let empty = scratch.file("empty.nes", b"");
    let prepared = ready(prepare(&empty, &Cores::new(&LIST), None, &scratch.dirs()));
    assert_eq!(prepared.sha256(), Some(&sha256_of(b"")));
}

#[test]
fn several_cores_that_run_the_file_leave_the_choice_and_name_themselves() {
    static LIST: [&dyn Core; 3] = [&UNEMUNES, &ALPHA_NONE, &NES_TWO];
    let scratch = Scratch::new();
    let game = scratch.file("test-pattern.nes", b"cartridge");
    let outcome = prepare(&game, &Cores::new(&LIST), None, &scratch.dirs()).unwrap();
    let Outcome::Choose(choices) = outcome else {
        panic!("one core was chosen");
    };
    assert_eq!(
        choices
            .iter()
            .map(|prepared| prepared.core().description().id)
            .collect::<Vec<_>>(),
        ["unemunes", "nes-two"]
    );
    let error = Outcome::Choose(choices).into_ready().err().unwrap();
    assert_eq!(
        error.to_string(),
        "More than one core in this program runs test-pattern.nes: unemunes (unemunes), Second NES core (nes-two). Name one on the command line: --open FILE --core ID."
    );
    // Named, one of them runs it alone.
    let prepared = ready(prepare(
        &game,
        &Cores::new(&LIST),
        Some("nes-two"),
        &scratch.dirs(),
    ));
    assert_eq!(prepared.core().description().id, "nes-two");
    assert!(Outcome::Ready(prepared).into_ready().is_ok());
}

#[test]
fn content_that_needs_more_than_the_file_is_refused_with_what_is_missing() {
    static LIST: [&dyn Core; 1] = [&FIRMWARE];
    let scratch = Scratch::new();
    let disk = scratch.file("disk-side-a.fds", b"disk");
    let error = refusal(prepare(&disk, &Cores::new(&LIST), None, &scratch.dirs()));
    assert_eq!(
        error.to_string(),
        "disk-side-a.fds needs more than itself to run. Missing: firmware, bios. Open… hands a machine the one file and nothing else."
    );
}

#[test]
fn the_config_must_name_the_file_by_digest_and_size_and_nothing_else() {
    let scratch = Scratch::new();
    let game = scratch.file("game.nes", b"cartridge");
    static SIZED: [&dyn Core; 1] = [&OTHER_SIZE];
    assert_eq!(
        refusal(prepare(&game, &Cores::new(&SIZED), None, &scratch.dirs())),
        LaunchError::NeedsMore {
            file: "game.nes".to_owned(),
            missing: vec!["cartridge".to_owned()],
        }
    );
    static EMPTY: [&dyn Core; 1] = [&NAMES_NOTHING];
    assert_eq!(
        refusal(prepare(&game, &Cores::new(&EMPTY), None, &scratch.dirs())).to_string(),
        "The core Empty core failed on game.nes: the config it wrote does not name the file"
    );
    // The file named twice is still the file alone.
    static TWICE: [&dyn Core; 1] = [&UNEMUNES];
    ready(prepare(&game, &Cores::new(&TWICE), None, &scratch.dirs()));
}

#[test]
fn a_file_that_cannot_be_read_is_refused() {
    static LIST: [&dyn Core; 1] = [&TestCore];
    let scratch = Scratch::new();
    let missing = scratch.0.join("files/gone.utc");
    assert!(matches!(
        refusal(prepare(&missing, &Cores::new(&LIST), None, &scratch.dirs())),
        LaunchError::Read { file, .. } if file == "gone.utc"
    ));
    let directory = scratch.0.join("files");
    assert_eq!(
        refusal(prepare(
            &directory,
            &Cores::new(&LIST),
            None,
            &scratch.dirs()
        ))
        .to_string(),
        "files could not be read: it is not a file."
    );
}

#[test]
fn a_started_machine_keeps_its_records_under_the_core_and_the_content_s_digest() {
    static LIST: [&dyn Core; 1] = [&TestCore];
    let scratch = Scratch::new();
    let path = scratch.file("two-players.utc", SCRIPT);
    let prepared = ready(prepare(&path, &Cores::new(&LIST), None, &scratch.dirs()));
    let mut machine = start(&prepared).unwrap();
    machine.finish().unwrap();
    drop(machine);
    let record = script_dir(&scratch.0.join("saves"))
        .join(unemu_core_files::file_name(BATTERY_RECORD).unwrap());
    assert_eq!(record, battery_file(&prepared));
    assert!(std::fs::read_to_string(&record)
        .unwrap()
        .ends_with("starts 1\n"));
}

#[test]
fn the_record_written_by_one_machine_is_read_by_the_next() {
    static LIST: [&dyn Core; 1] = [&TestCore];
    let scratch = Scratch::new();
    let path = scratch.file("two-players.utc", SCRIPT);
    // The same content under another name finds the same record.
    let copy = scratch.file("copy.utc", SCRIPT);
    for (starts, file) in [(1, &path), (2, &copy), (3, &path)] {
        let prepared = ready(prepare(file, &Cores::new(&LIST), None, &scratch.dirs()));
        let mut machine = start(&prepared).unwrap();
        machine.finish().unwrap();
        drop(machine);
        assert!(
            std::fs::read_to_string(battery_file(&prepared))
                .unwrap()
                .ends_with(&format!("starts {starts}\n")),
            "start {starts}"
        );
    }
}

#[test]
fn a_second_machine_on_content_whose_record_the_first_holds_is_refused() {
    static LIST: [&dyn Core; 1] = [&TestCore];
    let scratch = Scratch::new();
    let path = scratch.file("two-players.utc", SCRIPT);
    let first = ready(prepare(&path, &Cores::new(&LIST), None, &scratch.dirs()));
    let copy = scratch.file("same-bytes.utc", SCRIPT);
    let second = ready(prepare(&copy, &Cores::new(&LIST), None, &scratch.dirs()));
    let mut running = start(&first).unwrap();
    let error = start(&second).err().unwrap();
    assert_eq!(error.kind, ErrorKind::Usage);
    assert_eq!(
        error.message,
        "same-bytes.utc is already running in another machine, here or in another program, and its saved data is in use. Stop that machine first."
    );
    // The first machine runs on, and once it is gone the record is free.
    running.finish().unwrap();
    drop(running);
    start(&second).unwrap();
}

#[test]
fn a_file_that_changes_after_hashing_is_refused() {
    static LIST: [&dyn Core; 1] = [&TestCore];
    let scratch = Scratch::new();
    let path = scratch.file("two-players.utc", SCRIPT);
    let prepared = ready(prepare(&path, &Cores::new(&LIST), None, &scratch.dirs()));
    let mut changed = SCRIPT.to_vec();
    changed.extend_from_slice(b"# one more line\n");
    std::fs::write(&path, &changed).unwrap();
    let error = start(&prepared).err().unwrap();
    assert_eq!(error.kind, ErrorKind::Content);
}

#[test]
fn the_machine_gets_the_one_file_and_nothing_else() {
    static LIST: [&dyn Core; 1] = [&TestCore];
    let scratch = Scratch::new();
    let path = scratch.file("two-players.utc", SCRIPT);
    let prepared = ready(prepare(&path, &Cores::new(&LIST), None, &scratch.dirs()));
    let source = OneFile(&prepared);
    let sha256 = sha256_of(SCRIPT);
    let size = SCRIPT.len() as u64;
    let Content::Reader(reader) = source.open(&sha256, size).unwrap() else {
        panic!("the file came as bytes");
    };
    let mut bytes = vec![0; SCRIPT.len()];
    reader.read_at(0, &mut bytes).unwrap();
    assert_eq!(bytes, SCRIPT);
    for (digest, length) in [(sha256, size + 1), ([0; 32], size)] {
        assert_eq!(
            source.open(&digest, length).unwrap_err().kind,
            ErrorKind::Content
        );
    }
}

#[test]
fn the_filters_list_each_platform_that_opens_files_then_all_supported_then_all() {
    static LIST: [&dyn Core; 6] = [
        &TestCore,
        &NO_PLATFORM,
        &UNEMUNES,
        &OLD_NES,
        &TWO_KINDS,
        &NES_TWO,
    ];
    let names: Vec<(String, Vec<&str>)> = Cores::new(&LIST)
        .filters()
        .into_iter()
        .map(|Filter { name, extensions }| (name, extensions))
        .collect();
    assert_eq!(
        names,
        [
            ("unemu test core scripts (*.utc)".to_owned(), vec!["utc"]),
            // Two cores of one platform give one line.
            ("NES cartridge images (*.nes)".to_owned(), vec!["nes"]),
            (
                "Disk system: Disk images, Tape images (*.fds;*.qd;*.tap)".to_owned(),
                vec!["fds", "qd", "tap"]
            ),
            (
                "All supported files (*.utc;*.nes;*.fds;*.qd;*.tap)".to_owned(),
                vec!["utc", "nes", "fds", "qd", "tap"]
            ),
            ("All files (*.*)".to_owned(), vec!["*"]),
        ]
    );
}

#[test]
fn a_program_whose_cores_open_no_files_offers_only_all_files() {
    static LIST: [&dyn Core; 2] = [&NO_PLATFORM, &OLD_NES];
    assert_eq!(
        Cores::new(&LIST).filters(),
        [Filter {
            name: "All files (*.*)".to_owned(),
            extensions: vec!["*"],
        }]
    );
}

#[test]
fn a_core_that_fails_is_left_out_and_heard_only_when_no_core_is_left() {
    let scratch = Scratch::new();
    let game = scratch.file("game.nes", b"cartridge");
    // In identify, and in the check of its answer.
    static IDENTIFY: [&dyn Core; 2] = [&BROKEN, &UNEMUNES];
    static CHECK: [&dyn Core; 3] = [&NAMES_NOTHING, &UNEMUNES, &WRONG_KIND];
    for list in [&IDENTIFY[..], &CHECK[..]] {
        let cores = Cores::new(list);
        let prepared = ready(prepare(&game, &cores, None, &scratch.dirs()));
        assert_eq!(prepared.core().description().id, "unemunes");
    }
    static ALONE: [&dyn Core; 2] = [&NES_NONE, &BROKEN];
    assert_eq!(
        refusal(prepare(&game, &Cores::new(&ALONE), None, &scratch.dirs())).to_string(),
        "The core Broken core failed on game.nes: the scripted core failed"
    );
    // A failed check speaks before a core that cannot run the file, and that
    // before a core that failed in identify.
    static ORDER: [&dyn Core; 3] = [&BROKEN, &PICKY, &NAMES_NOTHING];
    assert_eq!(
        refusal(prepare(&game, &Cores::new(&ORDER), None, &scratch.dirs())).to_string(),
        "The core Empty core failed on game.nes: the config it wrote does not name the file"
    );
    static UNSUPPORTED: [&dyn Core; 2] = [&BROKEN, &PICKY];
    assert!(matches!(
        refusal(prepare(&game, &Cores::new(&UNSUPPORTED), None, &scratch.dirs())),
        LaunchError::Unsupported { core, .. } if core == "Picky core"
    ));
}

#[test]
fn the_file_failing_in_identify_ends_the_open() {
    let scratch = Scratch::new();
    let game = scratch.file("game.nes", b"cartridge");
    static CHANGES: [&dyn Core; 2] = [&CHANGED, &UNEMUNES];
    assert_eq!(
        refusal(prepare(&game, &Cores::new(&CHANGES), None, &scratch.dirs())),
        LaunchError::Changed {
            file: "game.nes".to_owned()
        }
    );
    static FAILS: [&dyn Core; 2] = [&UNREADABLE, &UNEMUNES];
    assert_eq!(
        refusal(prepare(&game, &Cores::new(&FAILS), None, &scratch.dirs())),
        LaunchError::Read {
            file: "game.nes".to_owned(),
            reason: "the scripted core failed".to_owned(),
        }
    );
}

#[test]
fn an_answer_in_a_content_kind_the_platform_does_not_list_is_a_core_failure() {
    static LIST: [&dyn Core; 1] = [&WRONG_KIND];
    let scratch = Scratch::new();
    let game = scratch.file("game.nes", b"cartridge");
    assert_eq!(
        refusal(prepare(&game, &Cores::new(&LIST), None, &scratch.dirs())).to_string(),
        "The core Wrong kind core failed on game.nes: it answered with the content kind tape, which its platform nes does not list"
    );
}

#[test]
fn a_core_id_that_cannot_name_one_directory_is_a_core_failure() {
    static LIST: [&dyn Core; 1] = [&BAD_ID];
    let scratch = Scratch::new();
    let game = scratch.file("game.nes", b"cartridge");
    assert_eq!(
        refusal(prepare(&game, &Cores::new(&LIST), None, &scratch.dirs())).to_string(),
        "The core Escaping core failed on game.nes: its id ../escape cannot name a directory for its records"
    );
    for id in [
        "",
        ".",
        "..",
        ".hidden",
        "a/b",
        "a\\b",
        "c:",
        &"x".repeat(65),
    ] {
        assert!(!is_directory_name(id), "{id}");
    }
    for id in ["unemupc", "unemu-test-core", "nes_2.1", &"x".repeat(64)] {
        assert!(is_directory_name(id), "{id}");
    }
}

#[test]
fn a_usage_failure_of_the_core_s_own_is_not_taken_for_a_held_record() {
    static LIST: [&dyn Core; 2] = [&BAD_NAME, &PAST_THE_END];
    let cores = Cores::new(&LIST);
    let scratch = Scratch::new();
    let game = scratch.file("game.nes", b"cartridge");
    for id in ["bad-name", "past-the-end"] {
        let prepared = ready(prepare(&game, &cores, Some(id), &scratch.dirs()));
        let error = start(&prepared).err().unwrap();
        assert_eq!(error.kind, ErrorKind::Usage, "{id}");
        assert!(
            !error.message.contains("already running"),
            "{id}: {}",
            error.message
        );
    }
}

#[test]
fn a_file_whose_modification_time_changes_after_hashing_is_refused() {
    static LIST: [&dyn Core; 1] = [&TestCore];
    let scratch = Scratch::new();
    let path = scratch.file("two-players.utc", SCRIPT);
    let prepared = ready(prepare(&path, &Cores::new(&LIST), None, &scratch.dirs()));
    // The same bytes, the same length, another time.
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    let modified = file.metadata().unwrap().modified().unwrap();
    file.set_modified(modified - std::time::Duration::from_secs(3600))
        .unwrap();
    drop(file);
    assert_eq!(std::fs::read(&path).unwrap(), SCRIPT);
    assert_eq!(start(&prepared).err().unwrap().kind, ErrorKind::Content);
}

// Starts from parts: a platform's own content and storage, made on a worker
// and opened at each start.

/// A script in memory, for `identify` to write its config.
struct InMemory(Vec<u8>);

impl ContentReader for InMemory {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let bytes = start
            .checked_add(buf.len())
            .and_then(|end| self.0.get(start..end))
            .ok_or_else(|| Error::new(ErrorKind::Usage, "a read past the end"))?;
        buf.copy_from_slice(bytes);
        Ok(())
    }
}

/// The test core's config for `script`, as it writes it.
fn script_config(script: &[u8]) -> ConfigDocumentBuf {
    let reader = InMemory(script.to_vec());
    let offered = OfferedContent::new("script", sha256_of(script), reader.size(), &reader);
    TestCore
        .identify(&offered)
        .unwrap()
        .expect("the test core runs the script")
        .config
}

/// Content in memory, opened as bytes; counts its opens.
struct BytesPlan {
    bytes: Vec<u8>,
    opens: Arc<AtomicUsize>,
}

impl ContentPlan for BytesPlan {
    fn open(&self) -> Result<Box<dyn ContentSource>, Error> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(BytesSource(self.bytes.clone())))
    }
}

/// The plan's one piece of content, by its digest and size.
struct BytesSource(Vec<u8>);

impl ContentSource for BytesSource {
    fn open(&self, sha256: &[u8; 32], size: u64) -> Result<Content, Error> {
        if *sha256 == sha256_of(&self.0) && size == self.0.len() as u64 {
            Ok(Content::Bytes(self.0.clone()))
        } else {
            Err(Error::new(ErrorKind::Content, "not in this plan"))
        }
    }
}

/// Records in one directory; counts its opens.
struct DirectoryPlan {
    dir: PathBuf,
    opens: Arc<AtomicUsize>,
}

impl StoragePlan for DirectoryPlan {
    fn open(&self) -> Result<Rc<dyn Storage>, Error> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        Ok(Rc::new(FileStorage::open(&self.dir)?))
    }
}

/// A plan that cannot open.
struct Broken;

impl ContentPlan for Broken {
    fn open(&self) -> Result<Box<dyn ContentSource>, Error> {
        Err(Error::new(ErrorKind::Storage, "the plan's file is gone"))
    }
}

impl StoragePlan for Broken {
    fn open(&self) -> Result<Rc<dyn Storage>, Error> {
        Err(Error::new(ErrorKind::Storage, "the plan's file is gone"))
    }
}

/// Parts for the test core over `SCRIPT`, its records in `dir`, with the
/// plans' open counters.
fn script_parts(dir: &Path) -> (Parts, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let content = Arc::new(AtomicUsize::new(0));
    let storage = Arc::new(AtomicUsize::new(0));
    let parts = Parts {
        core: &TestCore,
        platform: unemu_test_core::PLATFORM,
        title: "A stored machine".to_owned(),
        notes: vec![Message::new(
            unemu_core_api::MessageLevel::Note,
            "started from parts",
        )],
        config: script_config(SCRIPT),
        content: Box::new(BytesPlan {
            bytes: SCRIPT.to_vec(),
            opens: Arc::clone(&content),
        }),
        storage: Box::new(DirectoryPlan {
            dir: dir.to_owned(),
            opens: Arc::clone(&storage),
        }),
        options: Vec::new(),
    };
    (parts, content, storage)
}

fn starts_in(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(unemu_core_files::file_name(BATTERY_RECORD).unwrap())).unwrap()
}

#[test]
fn a_start_from_parts_keeps_its_record_for_the_next_start_of_the_same_prepared() {
    let scratch = Scratch::new();
    let records = scratch.0.join("records");
    let (parts, content, storage) = script_parts(&records);
    let prepared = Prepared::from_parts(parts);
    // A power cycle starts again from the same `Prepared`: each start opens
    // the plans anew and finds what the one before it kept.
    for starts in 1..=3 {
        let mut machine = start(&prepared).unwrap();
        machine.finish().unwrap();
        drop(machine);
        assert!(
            starts_in(&records).ends_with(&format!("starts {starts}\n")),
            "start {starts}"
        );
        assert_eq!(content.load(Ordering::Relaxed), starts);
        assert_eq!(storage.load(Ordering::Relaxed), starts);
    }
    // A second `Prepared` over the same records continues them.
    let (parts, _, _) = script_parts(&records);
    let mut machine = start(&Prepared::from_parts(parts)).unwrap();
    machine.finish().unwrap();
    drop(machine);
    assert!(starts_in(&records).ends_with("starts 4\n"));
}

#[test]
fn a_start_from_parts_answers_what_the_parts_say_and_no_file_s_facts() {
    let scratch = Scratch::new();
    let (parts, _, _) = script_parts(&scratch.0);
    let prepared = Prepared::from_parts(parts);
    assert_eq!(prepared.core().description().id, unemu_test_core::CORE_ID);
    assert_eq!(prepared.platform(), "unemu-test");
    assert_eq!(prepared.platform_name(), "Test machine");
    assert_eq!(prepared.title(), "A stored machine");
    assert_eq!(
        prepared
            .notes()
            .iter()
            .map(|note| note.text.as_str())
            .collect::<Vec<_>>(),
        ["started from parts"]
    );
    assert_eq!(prepared.config().media_type, unemu_test_core::CONFIG_TYPE);
    // The facts of an opened file are not there.
    assert_eq!(prepared.kind(), None);
    assert_eq!(prepared.file_name(), None);
    assert_eq!(prepared.sha256(), None);
    assert_eq!(prepared.size(), None);
    assert_eq!(prepared.saves_dir(), None);
    assert_eq!(prepared.states_dir(), None);
    // Nothing was opened: preparing writes nothing.
    assert!(!scratch
        .0
        .join(unemu_core_files::file_name(BATTERY_RECORD).unwrap())
        .exists());

    // A platform the core does not describe is named by its id, and the
    // core refuses it at the start.
    let (mut parts, _, _) = script_parts(&scratch.0);
    parts.platform = "nes";
    let prepared = Prepared::from_parts(parts);
    assert_eq!(prepared.platform_name(), "nes");
    let error = start(&prepared).err().unwrap();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert_eq!(error.message, "the test core has no platform nes");
}

#[test]
fn start_with_hands_back_the_caller_s_own_type() {
    /// A platform's own machine type, built from the request.
    struct Typed {
        platform: String,
        machine: Box<dyn Machine>,
    }
    let scratch = Scratch::new();
    let (parts, _, _) = script_parts(&scratch.0);
    let prepared = Prepared::from_parts(parts);
    let mut typed: Typed = start_with(&prepared, |request| {
        Ok(Typed {
            platform: request.platform.to_owned(),
            machine: TestCore.create(request)?,
        })
    })
    .unwrap();
    assert_eq!(typed.platform, "unemu-test");
    typed.machine.finish().unwrap();
    drop(typed);
    assert!(starts_in(&scratch.0).ends_with("starts 1\n"));

    // A file's start, too, and its records are where `start` keeps them.
    static LIST: [&dyn Core; 1] = [&TestCore];
    let path = scratch.file("two-players.utc", SCRIPT);
    let opened = ready(prepare(&path, &Cores::new(&LIST), None, &scratch.dirs()));
    let size = start_with(&opened, |request| {
        let script = sha256_of(SCRIPT);
        match request.content.open(&script, SCRIPT.len() as u64)? {
            Content::Reader(reader) => Ok(reader.size()),
            _ => panic!("the file came as bytes"),
        }
    })
    .unwrap();
    assert_eq!(size, SCRIPT.len() as u64);
}

thread_local! {
    /// The options the recording core was created with on this thread.
    static OPTIONS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// A core that keeps the options it is created with and builds nothing.
struct Recording;

static RECORDING: CoreDescription = CoreDescription::new(
    ContractVersion::new(0, 6),
    "recording",
    "Recording core",
    "1.0.0",
);

impl Core for Recording {
    fn description(&self) -> &CoreDescription {
        &RECORDING
    }

    fn create(&self, request: &CreateRequest<'_>) -> Result<Box<dyn Machine>, Error> {
        OPTIONS.with(|options| {
            *options.borrow_mut() = request
                .options
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect();
        });
        Err(Error::new(ErrorKind::Unsupported, "a recording core"))
    }
}

fn option(key: &str, value: &str) -> (String, String) {
    (key.to_owned(), value.to_owned())
}

#[test]
fn an_option_reaches_create_and_an_unknown_one_is_the_core_s_refusal() {
    let scratch = Scratch::new();
    let (mut parts, _, _) = script_parts(&scratch.0);
    parts.core = &Recording;
    parts.options = vec![option("allow-unlisted-firmware", "yes")];
    // The application's run-only options come after those of the parts.
    let prepared = Prepared::from_parts(parts).with_options(vec![option("speaker-trace", "on")]);
    start(&prepared).err().unwrap();
    assert_eq!(
        OPTIONS.with(|options| options.take()),
        [
            option("allow-unlisted-firmware", "yes"),
            option("speaker-trace", "on")
        ]
    );

    // The test core describes no option: its refusal comes back as it is,
    // from parts and from a file alike.
    let (parts, _, _) = script_parts(&scratch.0);
    let prepared = Prepared::from_parts(parts).with_options(vec![option("speaker-trace", "on")]);
    let error = start(&prepared).err().unwrap();
    assert_eq!(error.kind, ErrorKind::Usage);
    assert_eq!(error.message, "the test core takes no option speaker-trace");
    static LIST: [&dyn Core; 1] = [&TestCore];
    let path = scratch.file("two-players.utc", SCRIPT);
    let opened = ready(prepare(&path, &Cores::new(&LIST), None, &scratch.dirs()))
        .with_options(vec![option("speaker-trace", "on")]);
    let error = start(&opened).err().unwrap();
    assert_eq!(error.kind, ErrorKind::Usage);
    assert_eq!(error.message, "the test core takes no option speaker-trace");
    // Without options, it starts.
    let (parts, _, _) = script_parts(&scratch.0);
    start(&Prepared::from_parts(parts)).unwrap();
}

#[test]
fn a_plan_that_cannot_open_fails_the_start_with_its_own_error() {
    let scratch = Scratch::new();
    for broken in ["content", "storage"] {
        let (mut parts, _, _) = script_parts(&scratch.0);
        match broken {
            "content" => parts.content = Box::new(Broken),
            _ => parts.storage = Box::new(Broken),
        }
        let error = start(&Prepared::from_parts(parts)).err().unwrap();
        assert_eq!(error.kind, ErrorKind::Storage, "{broken}");
        assert_eq!(error.message, "the plan's file is gone", "{broken}");
    }
}

#[test]
fn a_prepared_start_from_parts_crosses_threads() {
    let scratch = Scratch::new();
    let records = scratch.0.join("records");
    let worker_dir = records.clone();
    // Made on a worker, started on this thread.
    let prepared = std::thread::spawn(move || Prepared::from_parts(script_parts(&worker_dir).0))
        .join()
        .unwrap();
    let mut machine = start(&prepared).unwrap();
    machine.finish().unwrap();
    drop(machine);
    assert!(starts_in(&records).ends_with("starts 1\n"));
}

#[test]
fn a_held_record_of_a_start_from_parts_comes_back_as_the_storage_said_it() {
    let scratch = Scratch::new();
    let records = scratch.0.join("records");
    let (first, _, _) = script_parts(&records);
    let mut running = start(&Prepared::from_parts(first)).unwrap();
    // What the storage itself answers for the held record.
    let held = FileStorage::open(&records)
        .unwrap()
        .open(BATTERY_RECORD)
        .err()
        .unwrap();
    assert_eq!(held.kind, ErrorKind::Usage);
    let (second, _, _) = script_parts(&records);
    let error = start(&Prepared::from_parts(second)).err().unwrap();
    // Not the file start's sentence: the plan's storage words its own.
    assert_eq!(error, held);
    assert!(
        !error.message.contains("already running"),
        "{}",
        error.message
    );
    running.finish().unwrap();
}
