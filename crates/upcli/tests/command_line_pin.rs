// SPDX-License-Identifier: AGPL-3.0-or-later
//! The command line as it is documented, pinned so that moving its code
//! between crates cannot change it (`backlog/docs/design/core-contract.md`
//! §7.5): the usage text byte for byte, the `recipe` usage, the
//! `--version` and `--about` text, and how every documented command line
//! parses: each form of each usage line, every `unemu` example of
//! `backlog/docs/commands.md`, and a few refused lines whose reason depends
//! on which part of the command line reads them.
//!
//! `pinned/usage.txt` and `pinned/recipe-usage.txt` are the texts the
//! program printed before the command line was split into a neutral part
//! and the PC's, with the lines of commands added since. `pinned/documented.txt` holds the examples of commands.md
//! as they were written then, the arguments after `-p cli --` (the package then) of each
//! `cargo run` and each line that starts with `unemu `, without a trailing
//! comment; the test keeps its own copy so that editing the documentation
//! stays a documentation change. `pinned/command-lines.txt` is each line,
//! as `$ WORDS`, followed by the `Debug` form of its parse.

use std::ffi::OsString;
use std::path::Path;

/// Each form of each usage line, with every option it documents.
const USAGE_FORMS: &[&str] = &[
    "",
    "--version",
    "--about",
    "run --machine FILE",
    "run --machine FILE --no-store",
    "run --machine FILE --store DIR --rom-dir DIR --max-steps 5 --idle-limit 6 --print CARD --allow-unlisted-firmware",
    "image import FILE --store DIR --tag NAME:TAG",
    "image import FILE --store DIR --tag NAME:TAG --create --no-compress --rom-dir DIR --recover medium-0=tip --recover medium-1=generation:2 --min-app-version 0.0.1",
    "image compress NAME:TAG --store DIR --tag NAME:TAG2",
    "image ls --store DIR",
    "image sweep --store DIR",
    "image run NAME:TAG --store DIR --workspace NAME",
    "image run NAME:TAG --store DIR --workspace NAME --new --max-steps 7",
    "image workspace ls --store DIR",
    "image workspace discard NAME --store DIR",
    "image workspace discard NAME --store DIR --yes",
    "image workspace abandon-publication NAME --store DIR",
    "image publish --from-workspace NAME --store DIR --tag NAME:TAG",
    "image publish --from-workspace NAME --store DIR --tag NAME:TAG --flatten",
    "image machine ls --store DIR",
    "image machine new NAME:TAG --store DIR",
    "image machine new NAME:TAG --store DIR --name NAME",
    "image machine rename ID NAME --store DIR",
    "image machine duplicate ID --store DIR --name NAME",
    "image machine reset ID --store DIR --yes",
    "image machine rm ID --store DIR --yes",
    "image machine settings ID --store DIR",
    "image machine settings ID --store DIR --set memory.extended_kib=3072 --set video.card=vga --dry-run",
    "image machine settings ID --store DIR --clear",
    "image machine commit ID --store DIR --tag NAME:TAG",
    "image machine commit ID --store DIR --tag NAME:TAG --title TITLE --keep-on-old-image",
    "image machine run ID --store DIR --max-steps 8",
    "image media import FILE --title TITLE --tag NAME:TAG --store DIR",
    "image media import FILE FILE2 --title TITLE --tag NAME:TAG --store DIR --create --dry-run --no-compress --label 1=TEXT --format 2=1.44M --extra PATH:DOSNAME --min-app-version 0.0.1",
    "image media import DIR --title TITLE --tag NAME:TAG --store DIR --open --license LicenseRef-NAME --license-text EXTRA",
    "image media import DIR --title TITLE --tag NAME:TAG --store DIR --open --license MIT --license-text EXTRA --source EXTRA",
    "image media import FILE --title TITLE --tag NAME:TAG --store DIR --publisher P --publisher P2 --developer D --year 1990 --genre G --platform DOS --description TEXT --version V --edition E --set-label L --set-label L2",
    "image media import FILE --title TITLE --tag NAME:TAG --store DIR --need cpu=286+ --need sound=sb,none --requirements TEXT --work ID --synopsis TEXT --work-id wikidata=Q1 --work-id igdb=ID",
    "image firmware import FOLDER",
    "image firmware import FILES FILES2 --store DIR --create --rescan --dry-run",
    "image firmware import FOLDER --for kiselev-xi8088-1.0.2 --extra PATH:DOSNAME --extra-license MIT --min-app-version 0.0.1",
    "image firmware import FOLDER --for kiselev-xi8088-1.0.2 --title T --description T --notes T --vendor T --version T --date 2024-01-01 --label L --board B --class C --requires KIND --dumped-from T --revision R --url U --provenance-date 2024-01-01 --open --license MIT --source ARCHIVE.tar --source-license MIT",
    "image firmware import FILE --unlisted ID --role ROLE --store DIR --create --dry-run --user-supplied --downloaded-from T",
    "image firmware edit NAME:TAG --title T",
    "image firmware edit NAME:TAG --to NAME:TAG2 --store DIR --regenerate-readme --built-from T --min-app-version 0.0.1",
    "image firmware ls",
    "image firmware ls --store DIR",
    "image firmware show NAME:TAG",
    "image firmware show NAME:TAG --store DIR",
    "image media ls --store DIR",
    "image media show NAME:TAG --store DIR",
    "image export NAME:TAG --to DIR",
    "image export NAME:TAG --to DIR --files --store DIR",
    "image push NAME:TAG --store DIR",
    "image push NAME:TAG REFERENCE --store DIR --registry HOST --move --dry-run --config FILE",
    "image pull REFERENCE --store DIR",
    "image pull REFERENCE --store DIR --tag NAME:TAG --create --registry HOST --config FILE",
    "image remote ls REPOSITORY --registry HOST --config FILE",
    "image remote ls REGISTRY",
    "image login HOST --user NAME --config FILE",
    "image registry set HOST --private --config FILE",
    "image registry set HOST --namespace NAME --namespace OTHER:private --namespace THIRD:public",
    "image template build --store DIR",
    "image template build --store DIR --vendor DIR --dry-run",
    "image template build --store DIR --vendor DIR --vendor DIR2 --dry-run",
    "recipe check FILE FOLDER",
    "recipe show FILE",
    "recipe plan FILE",
    "recipe plan FILE --json --library DIR --library DIR2 --store DIR --config FILE",
    "recipe build FILE",
    "recipe build FILE --fetch --library DIR --library DIR2 --store DIR --config FILE --keep-as TAG",
    "recipe pin FILE",
    "recipe pin FILE --fetch --library DIR --library DIR2 --store DIR --config FILE",
    "recipe conformance FILE FOLDER",
    "recipe conformance FOLDER --library DIR --library DIR2 --store DIR --config FILE",
    "recipe scaffold FILE FOLDER",
    "recipe inputs FILE --format dat",
    "recipe inputs --format sha256sums FILE",
    "recipe plan REF:TAG",
    "recipe push FILE REF",
    "recipe push FILE REF --dry-run --registry HOST --store DIR --config FILE",
    "recipe pull REF",
    "recipe pull REF --to DIR --registry HOST --config FILE",
    "recipe",
    "library ls",
    "library ls --store DIR --config FILE",
    "library add DIR --config FILE",
    "library remove DIR",
    "library scan",
    "library scan DIR DIR2 --store DIR --config FILE",
    "media create floppy --type 1.44M OUT",
    "media create floppy --type 360K OUT --format --label NAME --force",
    "media create floppy --type 720K OUT --add PATH --add PATH2 --top NAME --label NAME --split --force",
    "media create floppy --type 1.2M OUT --add PATH",
    "media create hd --chs 20/4/17 OUT --force",
    "media create hd --size 32MiB OUT",
    "media create hd --size 1GiB OUT",
    "media generations --base IMG --journal J",
    "media verify --base IMG --journal J",
    "media verify --base IMG --journal J --repair-tail --backup PATH",
    "media rollback --base IMG --journal J --to 1 --backup PATH",
    "media commit --base IMG --journal J --to 1 --output OUT",
    "media compact --base IMG --journal J --to 1 --output OUT",
    // Refused lines, each read by another part of the command line.
    "--bogus",
    "--allow-unlisted-firmware",
    "--rom-dir DIR --boot",
    "--version extra",
    "media verify --base IMG --journal J --rom-dir DIR",
    "media create floppy --type 360K OUT --rom-dir DIR",
    "media bogus",
    "image",
    "image bogus",
    "image ls --store DIR --create",
    "image workspace bogus",
    "image media",
    "image firmware",
    "image firmware import FOLDER --for no-such-entry",
    "image machine",
    "image template",
    // Refused lines whose words the neutral part and the PC's part split
    // between them.
    "image workspace discard",
    "image workspace discard --store DIR",
    "image workspace abandon-publication",
    "image workspace abandon-publication --store DIR",
    "image media import",
    "image media ls --store DIR --title T",
    "image media show --store DIR",
    "image firmware show",
    "image firmware show --store DIR",
    "image firmware ls --store DIR --for X",
    "image firmware edit",
    "image export",
    "image export NAME:TAG",
    "image export --to DIR",
    "image compress",
    "image compress --store DIR --tag A:B",
    "image import",
    "image import --store DIR",
    "image run",
    "image run --store DIR",
    "image publish --store DIR",
    "image publish",
    "library",
    "library add",
    "library add DIR --store DIR",
];

/// Split a shell command line into words: whitespace separates, single and
/// double quotes group, nothing is expanded.
fn words(line: &str) -> Vec<String> {
    let (mut words, mut word, mut quote, mut open) = (Vec::new(), String::new(), None, false);
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                open = true;
            }
            (None, c) if c.is_whitespace() => {
                if open {
                    words.push(std::mem::take(&mut word));
                    open = false;
                }
            }
            (None, c) => {
                word.push(c);
                open = true;
            }
        }
    }
    assert!(quote.is_none(), "unclosed quote: {line}");
    if open {
        words.push(word);
    }
    words
}

/// A word as a shell would need it written.
fn quoted(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-./:=@%+,$".contains(&b));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// The `unemu` examples of commands.md, one per line.
fn documented() -> Vec<String> {
    pinned("documented.txt")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn render(lines: impl IntoIterator<Item = String>) -> String {
    let mut text = String::new();
    for line in lines {
        let args = words(&line);
        let shown: Vec<String> = args.iter().map(|w| quoted(w)).collect();
        let parsed = upcli::command::parse(&args.iter().map(OsString::from).collect::<Vec<_>>());
        text.push_str(&format!("$ {}\n{parsed:?}\n\n", shown.join(" ")));
    }
    text
}

fn pinned(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/pinned")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

/// Compare with a pinned file; on a difference, the actual text is written
/// beside the system's temporary files for review.
fn assert_pinned(name: &str, actual: &str) {
    let expected = pinned(name);
    if expected != actual {
        let out = std::env::temp_dir().join(format!("upcli-pin-{}-{name}", std::process::id()));
        std::fs::write(&out, actual).unwrap();
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
        panic!(
            "tests/pinned/{name} differs from line {}; actual text in {}",
            line + 1,
            out.display()
        );
    }
}

#[test]
fn the_usage_text_is_unchanged() {
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    assert_eq!(upcli::command::run(&[], &mut output, &mut errors), 0);
    assert!(errors.is_empty());
    let usage = String::from_utf8(output).unwrap();
    assert_pinned("usage.txt", &usage);

    // A refused line prints the same text before its reason.
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    assert_eq!(
        upcli::command::run(&["--bogus".into()], &mut output, &mut errors),
        2
    );
    assert!(output.is_empty());
    assert_eq!(
        String::from_utf8(errors).unwrap(),
        format!("{usage}unknown or extra arguments\n")
    );

    let (mut output, mut errors) = (Vec::new(), Vec::new());
    assert_eq!(
        upcli::command::run(&["recipe".into()], &mut output, &mut errors),
        2
    );
    assert!(output.is_empty());
    assert_pinned("recipe-usage.txt", &String::from_utf8(errors).unwrap());
}

/// `pinned/about.txt` is `--version` and `--about` with the release's list
/// of cores, and with the parts that change on their own (the release, the
/// licence notice whose source line names the build, the OPL3 notice)
/// replaced by names: the program's lines, one line per compiled-in core,
/// then the notices.
#[test]
fn the_about_text_is_pinned() {
    for option in ["--version", "--about"] {
        let (mut output, mut errors) = (Vec::new(), Vec::new());
        assert_eq!(
            upcli::command::run(&[option.into()], &mut output, &mut errors),
            0
        );
        assert!(errors.is_empty(), "{option}");
        let text = String::from_utf8(output)
            .unwrap()
            .replace(&platforms::licence_notice(), "LICENCE NOTICE")
            .replace(devices::OPL3_NOTICE, "OPL3 NOTICE")
            .replace(env!("CARGO_PKG_VERSION"), "VERSION");
        assert_pinned("about.txt", &text);
    }
}

#[test]
fn every_documented_command_line_parses_as_pinned() {
    let documented = documented();
    assert!(
        documented.len() > 80,
        "{} documented lines",
        documented.len()
    );
    let text = render(
        USAGE_FORMS
            .iter()
            .map(|line| (*line).to_owned())
            .chain(documented),
    );
    assert_pinned("command-lines.txt", &text);
}

#[test]
fn the_shell_words_of_a_documented_line_are_split_as_a_shell_would() {
    assert_eq!(
        words(r#"image media import a.img --title "Set one" --label '1=Disk 1' --store "$S""#),
        [
            "image", "media", "import", "a.img", "--title", "Set one", "--label", "1=Disk 1",
            "--store", "$S"
        ]
    );
    assert_eq!(words(""), Vec::<String>::new());
    assert_eq!(words("a '' b"), ["a", "", "b"]);
    assert_eq!(quoted("1=Disk 1"), "'1=Disk 1'");
    assert_eq!(quoted("it's"), r"'it'\''s'");
}
