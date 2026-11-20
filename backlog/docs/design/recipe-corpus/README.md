# Recipe corpus

The three firmware recipes, three FreeDOS recipes, `freedos.work.toml` and
all six `.toml.lock/` directories now live in
[`crates/pc-image/data/recipes/`](../../../../crates/pc-image/data/recipes/),
with the same subfolder names: `unemu-at-bios/`, `vgabios/`,
`xtide-universal-bios/` and `freedos-1.4-floppy/`. They are shipped build
inputs embedded by the template builder. This corpus retains their design
notes and HCL snapshots; Ken's Labyrinth and all other design examples stay
here. Recipe checks cover both locations.

## Current copies (2026-11-20)

The real TOML recipes, work files and locks track the
[`software` repository](https://github.com/unemu-org/software) at `3c493f7`:

| Recipe folder (shipped inputs above; other files here) | Source at that revision |
| --- | --- |
| `freedos-1.4-floppy/` | `recipes/media/freedos/`: three recipes, `freedos.work.toml` and their locks |
| `kenslab-2.01/` | `recipes/media/kenslab/`: the 2.01 recipe, `kenslab.work.toml` and its lock |
| `unemu-at-bios/`, `vgabios/`, `xtide-universal-bios/` | `recipes/firmware/`: three recipes and their locks |

The FreeDOS source-archive URLs use the repository's new name, `software`
(formerly `unemu-media`). Locks are copied byte for byte and contain no
payload. Bundled licence indexes and source-building scripts stay in the
software repository; the corpus's build tests supply inputs from fixtures.
All seven real recipes are now pinned. Their cited metadata supersedes the
earlier assumptions: work ids, requirements and needs, and firmware dates
and board compatibility. Firmware configs therefore differ from the older
template imports in those descriptive fields and the generated README.

The other TOML recipes remain design examples: Ken's Labyrinth 1.0
shareware, AZURAMON, FreeDOS LiveCD, Windows 3.0, IBM 5170 BIOS, Contra and
the Amiga disk. The shareware example still uses the shared Ken's Labyrinth
work file. HCL files remain first-round snapshots, with only the FreeDOS
source-archive URL rename applied. Tests compare the current TOML records
with those snapshots plus the explicit metadata corrections above.

## Design history

Status: **drafts** for the design in [`../recipes.md`](../recipes.md)
(`upc-5pw.113`), 2026-11-17. Nine recipes, each in the chosen syntax (TOML
1.0, `*.toml`) and in the strongest alternative (an HCL literal subset,
`*.hcl`). They exist to test the syntax and the model against real cases
before anything freezes (recipes.md §14). When it was written no builder
existed, so every recipe was a draft without a `digest`; the open ones are
pinned now (The pins, below).

Both syntaxes are written by one generator from one data model, so a pair
differs in syntax only. Every TOML file parses with a TOML 1.0 parser
(Python's `tomllib`). The HCL files were **not** parsed by any HCL tool:
none was at hand, and the subset's own parser is the next step. Treat their
syntax as reviewed by eye only.

Data marked **ASSUMED** or **PLACEHOLDER** in a file's comments was not
measured. Everything else is real: taken from unemu-media's recipes, from
this repository's firmware catalogue, or measured on the files themselves.
Two sets of numbers were measured for the earlier design draft and are
taken from it: the member sizes and times of LABFULL.ZIP and the size of
the LiveCD ISO.
Inputs of private recipes come from the user's library; no recipe names a
host path.

| Folder | What it is | Open | Exercises |
| --- | --- | --- | --- |
| [`freedos-1.4-floppy`](freedos-1.4-floppy/README.md) | FreeDOS 1.4 Floppy Edition, three formats | yes | Three artifacts from one input, `unpack`, `text`, licence with source |
| [`kenslab-2.01`](kenslab-2.01/README.md) | Ken's Labyrinth 2.01 | yes | `fat` in archive order with archive times, an extra from a tree |
| [`azuramon`](azuramon/README.md) | A private game as a folder tree | no | A file-set input of 561 files, a split over ten disks |
| [`freedos-1.4-livecd`](freedos-1.4-livecd/README.md) | A CD-ROM taken unmodified | not stated | An optical medium, a set of mixed formats |
| [`windows-3.0`](windows-3.0/README.md) | Windows 3.0 installation disks | no | Private inputs, database ids, an optional attachment |
| [`unemu-at-bios`](unemu-at-bios/README.md) | The open AT BIOS package | yes | A Firmwarefile without steps |
| [`vgabios`](vgabios/README.md) | The open VGA BIOS package | yes | A Firmwarefile without steps (added with the build, B7) |
| [`xtide-universal-bios`](xtide-universal-bios/README.md) | The open fixed-disk option ROM package | yes | A Firmwarefile without steps (added with the build, B7) |
| [`ibm-5170-bios`](ibm-5170-bios/README.md) | IBM 5170 BIOS from chip dumps | no | A ROM in parts, MAME ids |
| [`contra-nes`](contra-nes/README.md) | Contra and Probotector, NES | no | A future platform: regions, a work id |
| [`amiga-adf`](amiga-adf/README.md) | An Amiga disk image | no | A future platform: an unmodified medium, no release |

## The first builds (2026-11-18)

`upcli recipe build` (recipes.md §19.2, B7) builds these drafts. Two
firmware recipes were added then, for the video BIOS and the fixed-disk
option ROM of the open templates, so that the three open firmware packages
the templates bind each have a recipe; they have no HCL copy. MEASURED: the
three build the packages `pc_image::template::build_firmware` imports from
the same fixtures, digest for digest, and the three FreeDOS recipes build
sets whose configs equal byte for byte those the importer makes from the
same disks (a test in `pc-image`, `recipe_build`). Ken's Labyrinth and
AZURAMON build from a user's own files; AZURAMON's split fills exactly the
ten volumes its recipe assumed.

## The pins (2026-11-18)

`upcli recipe pin` (recipes.md §19.2, B8) pinned the six open recipes: the
three FreeDOS 1.4 floppy sets and the three firmware packages. Each now
states `digest` and has its lock beside it, `NAME.toml.lock/sha256/`, which
holds the manifest, the config and the metadata (a firmware package: its
README) as the pin wrote them, and no payload byte. The other recipes stay
drafts. `upcli recipe conformance` builds the pinned ones from the fixtures
and compares the digests (a test in `pc-image`, `recipe_lock`). A change to
one of their payload builders that alters a byte fails that test; a change
to how a release writes manifests, configs or metadata does not, as a pinned
build publishes the lock's bytes. To change a pinned artifact, raise
`revision`, remove `digest` and pin again. The FreeDOS pins equal the
digests of the template sets of the same tags (`pc-image` test `template`,
`MEDIA_PINNED`): the template builder now uses the same locked recipes,
including their title records merged with the work file.

## Second round: work files, TOML only (2026-11-17)

The maintainer chose TOML after reading this corpus. From here on only the
TOML files change; the `.hcl` files are kept as they were, as the evidence
for the fallback, and no longer match the TOML of the three folders below. They are
also the first round's full title records of those folders: a test in
`pc-image` (`recipe_check`) reads their `title`, `needs` and `ids` blocks and
compares them, field by field, with each recipe merged with its work file
(recipes.md §19.2, B5).

Three folders gained a **work file** (`*.work.toml`, recipes.md §6.16) with
what their releases share, and two gained a release to exercise it: Ken's
Labyrinth 1.0 shareware and Contra (USA) (Rev 1), both with placeholder
data. Lines without blanks and comments, MEASURED:

| Recipe | Before | After |
| --- | ---: | ---: |
| `freedos-1.4-floppy/freedos-1.4` | 116 | 107 |
| `freedos-1.4-floppy/freedos-1.4-1.2m` | 123 | 114 |
| `freedos-1.4-floppy/freedos-1.4-720k` | 151 | 142 |
| `freedos-1.4-floppy/freedos.work` | — | 15 |
| `kenslab-2.01/kenslab-2.01` | 94 | 86 |
| `kenslab-2.01/kenslab-1.0-shareware` (new) | — | 45 |
| `kenslab-2.01/kenslab.work` | — | 15 |
| `contra-nes/contra-usa` | 36 | 27 |
| `contra-nes/contra-japan` | 36 | 27 |
| `contra-nes/contra-europe` | 36 | 28 |
| `contra-nes/contra-usa-rev1` (new) | — | 27 |
| `contra-nes/contra.work` | — | 15 |

The three regional Contra recipes were 108 lines and are 82 plus a work
file of 15; a fourth release costs 27 lines instead of 36. The saving is
small where the build dominates (FreeDOS: 9 lines of 116) and a quarter
where the description dominates (cartridges). The larger gain is not lines:
a title, a developer list and a work id are written once and cannot drift
between regions.

The other six folders have one release each and no work file; `work` is
optional. The LiveCD recipe is a fourth release of FreeDOS and would name
`freedos` too if it lay in the same folder.

## Sizes of the first round (MEASURED)

Lines are counted without blank lines and comments.

| Recipe | TOML lines | TOML bytes | HCL lines | HCL bytes | Longest line, TOML | Longest line, HCL |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `freedos-1.4-floppy/freedos-1.4` | 116 | 4793 | 123 | 5156 | 529 | 537 |
| `freedos-1.4-floppy/freedos-1.4-1.2m` | 123 | 5076 | 130 | 5454 | 531 | 539 |
| `freedos-1.4-floppy/freedos-1.4-720k` | 151 | 6199 | 158 | 6637 | 532 | 540 |
| `kenslab-2.01/kenslab-2.01` | 94 | 5896 | 100 | 6205 | 851 | 859 |
| `azuramon/azuramoon-2026` | 641 | 72603 | 645 | 72783 | 291 | 299 |
| `freedos-1.4-livecd/freedos-1.4-livecd` | 56 | 2041 | 60 | 2257 | 279 | 287 |
| `windows-3.0/windows-3.0-1.2m` | 98 | 3974 | 108 | 4270 | 234 | 242 |
| `unemu-at-bios/unemu-at-bios-1.0.2-unemu.3` | 41 | 2132 | 47 | 2269 | 730 | 732 |
| `ibm-5170-bios/ibm-5170-bios-1985-06-10` | 38 | 1552 | 43 | 1692 | 174 | 176 |
| `contra-nes/contra-usa` | 36 | 1417 | 41 | 1602 | 130 | 138 |
| `contra-nes/contra-japan` | 36 | 1435 | 41 | 1620 | 132 | 140 |
| `contra-nes/contra-europe` | 36 | 1478 | 41 | 1663 | 138 | 146 |
| `amiga-adf/amdemo` | 24 | 955 | 28 | 1048 | 104 | 112 |
| **Total** | **1490** | **109,551** | **1565** | **112,656** | | |

HCL is 5% longer in lines (closing braces) and 3% in bytes (alignment). The
longest line of every file is its description, the same in both.

## What the two syntaxes showed

- **No recipe needed a workaround in one syntax that the other avoided.**
  The schema is the same tree in both.
- **HCL reads better at the top of a block**: `step "fat" "disk" {`,
  `medium "disk-1" {` and `input "labfull" {` say the kind and the name in
  one line. TOML says `[[step]]`, then `id = "disk"`, then `fat = "files"`.
- **TOML has the two value types the recipes want and HCL lacks**: a
  zone-free date-time (`time = 2008-07-03T00:00:00`; a string in HCL) and
  hexadecimal integers (`serial = 0x20080703`; `537396995` in HCL).
- **Nested blocks** (`[[attachment.extra]]`, `[package.fits]`) are where
  TOML is at its most awkward and HCL at its most natural. The corpus has
  two such places.
- **A member reference is a string in both** (`from = "files/README.TXT"`).
  HCL's own idiom would be an expression (`step.files["README.TXT"]`), which
  the literal subset forbids; so HCL's best feature for references goes
  unused.
- **Backslashes did not occur.** No v1 recipe names a DOS path. They will
  occur in Machinefiles (`type = 'C:\WINDOWS'`), where TOML's literal
  strings need no escape and HCL writes `"C:\\WINDOWS"`.

## What the model showed (across recipes)

Each folder's README has its own list. The ones that changed the design or
stay open:

1. **Most recipes have no steps at all.** Six of nine take their inputs
   unmodified. The execution part matters less than the first discussion
   assumed (recipes.md §4.3).
2. **One output cannot be built in v1**: the FreeDOS source archive (git,
   ISO, tar). It becomes a prepared input (recipes.md §12, Q6).
3. **An unmodified medium needs two tables**, an `[input]` and a
   `[[medium]]` (Windows 3.0: ten tables for five disks). A short form, a
   medium that states its own `size` and `sha256`, would cut such recipes by
   a third. Not in 0.1: two ways to say one thing. To be weighed during
   `0.x`.
4. **A file set is long**: 561 entries, 72 KiB. It is data; nothing in
   either syntax makes it shorter than one line per file.
5. **Host file times are useless** for a folder tree (they are the times of
   the user's copy). A recipe states its times or takes them from an
   archive.
6. **Software without a version** needs a rule for the release: the year
   when it is known (Azuramoon), nothing at all when it is not (the Amiga
   disk, tag `r1`). So `release` is optional for every kind.
7. **`needs` has two gaps**: "works without" (answered by `none`) and the
   operating system a title needs (`DOS 3.3+`; not in v1).
8. **One catalogue entry, two chip layouts** gives two different packages
   under one tag today (the IBM 5170 BIOS of 1984). The release part of a
   firmware tag separates them (recipes.md §8.5).
9. **A set of mixed formats** (a CD and its boot floppy) is natural for a
   recipe and awkward for today's insert rule, which wants one geometry per
   set.
10. **Cartridge dumps are catalogued without their header.** A recipe that
    names the file with its header cannot use a No-Intro hash as a hint.
    Needs provider input forms (recipes.md §13).
11. **Metadata schema 1's list of software platforms** has no NES and no
    AmigaOS. New platforms need schema 2 or a longer list. The corpus
    check fails `contra.work` and `amdemo` on it (recipes.md §4.6, O1).
12. **The ROM's hash was written twice** in the open firmware recipe (input
    and `[[rom]]`). Fixed in the specification: a `[[rom]]` with `from`
    takes size and hash from its value; only a ROM in `parts` states them.

## Machinefile corpus (2026-11-20)

**Recommendation: TOML stays.** This is the pre-freeze syntax review required
by recipes.md §14 step 5 and decision-33 point 7. The maintainer still decides;
this review neither changes decision-33 nor freezes the Machinefile schema.
The three installations below are documents in both syntaxes, outside the
shipped inputs. They are **not executable recipes**: `[machine]` and these
verbs are future work (recipes.md §11 and §13). Plain `.toml`/`.hcl`
filenames deliberately keep these drafts out of the current recursive recipe
checker's `*.machine.toml` executable-input discovery. None was installed in
this review. The historical media-corpus HCL snapshots above remain unchanged;
these new Machinefile pairs describe equal trees today.

| TOML / HCL pair | Real installation route | Syntax pressure |
| --- | --- | --- |
| [freedos-1.4.toml](machinefiles/freedos-1.4.toml) / [HCL](machinefiles/freedos-1.4.hcl) | FreeDOS 1.4 Floppy Edition: boot disk and six package disks onto a blank hard disk | 42 steps, seven inserts, repeated bounded text waits, multiline command input and screen match |
| [msdos-3.30.toml](machinefiles/msdos-3.30.toml) / [HCL](machinefiles/msdos-3.30.hcl) | Private MS-DOS 3.30 boot/utility floppies: FDISK, reboot, FORMAT /S, COPY, boot C: | 22 steps, 52 nested input actions; the longest input group has 17 actions |
| [windows-3.1.toml](machinefiles/windows-3.1.toml) / [HCL](machinefiles/windows-3.1.hcl) | Private Windows 3.1 six-disk Setup on the preceding DOS image, standard mode on a 286 | 27 steps, 30 nested input actions, graphics checkpoints, DOS paths and disk swaps |

READ: the [FreeDOS download page](https://www.freedos.org/download/) and
[installation guide](https://help.freedos.org/docs/howto/install.html)
describe the classic-PC floppy installation route. The shipped
[FreeDOS Mediafile](../../../../crates/pc-image/data/recipes/freedos-1.4-floppy/freedos-1.4.media.toml)
supplies the actual artifact digest, seven disks and medium ids used here.
[Microsoft's MS-DOS command reference](https://msarchive.pcjs.org/mspl13/msdos/encyclopedia/section3/)
documents partitioning and FORMAT's system transfer. Microsoft's archived
[KB Q113230](https://jeffpar.github.io/kbarchive/kb/113/Q113230/)
identifies disk 6 of the Windows 3.1 1.44 MB distribution;
[PCjs's Windows 3.10 machine](https://www.pcjs.org/software/pcx86/sys/windows/3.10/)
provides a separate AT/DOS 3.30 example (its seven 1.2 MB disks are a different
edition). These sources establish the routes, **not** the proposed key sequences.
No private media, title-file text or screenshots are included.

### Chosen shape and evidence boundaries

Every file marks the following **ASSUMED**. These are syntax probes, not
extensions to the accepted recipe schema:

- `recipe = "0.1"` retains the sketch's experimental family marker; it does
  not mean the current 0.1 checker accepts Machinefiles. `[machine]`, `[from]`
  and `[use.install]` follow §11. Steps have no ids. Each action has exactly
  one verb; `drive`, `budget`, `card`, `match`, `label` and `write` are arguments.
- The blank template is a proposed artifact, not an existing template tag.
  It contains the hardware described in each file's header: an AT/80286,
  VGA, 640 KiB conventional plus 2048 KiB extended RAM, a 1.44M drive and a
  30 MiB IDE master named `system` (960 cylinders, 4 heads, 16 sectors,
  512 bytes per sector). It is a base machine image, distinct from the
  hardware-only `schema = 1` documents in [machine-files.md](../machine-files.md).
  The Windows parent is the DOS output here, with matching placeholder pins.
- `insert` selects a medium of a digest-pinned set; `eject` empties the drive.
  Install media stay read-only. `start` powers on once; `reset = "warm"`
  restarts the guest while preserving its sector overlay. No host command is
  executed: `type = 'FORMAT C: /S'` types into the guest, then `key = "enter"`
  submits it. Commands are not a host shell escape.
- `[[step.input]]` is a finite, one-level sequence of the same input/wait
  actions, useful for a dialog or batch-file entry. It cannot nest another
  `input`, loop, branch or include another script. Named keys include `enter`,
  `escape`, `tab` and `ctrl+z`; `type` maps US-layout characters to make/break
  events and maps LF to Enter. It never submits a trailing Enter implicitly.
  Actions drain their queued input before the next action starts.
- `wait_text` searches the selected card's current text screen, with CRLF
  normalized to LF and trailing cell spaces removed per row. Multiline
  `contains` matches adjacent rows, not a concatenation of past frames.
  The three-line matches come from our own `VERIFY.BAT`, entered with
  `COPY CON` and Ctrl+Z. They prove only return to DOS and execution of that
  batch file; they do not verify installed files or boot Windows.
- `wait_state.kind = "screen-digest"` compares a complete visible VGA frame:
  SHA-256 over little-endian u32 width, u32 height, then row-major RGBA8.
  This proposed observer is not implemented. Private checkpoints use distinct
  placeholder hashes, so no installer text is copied. A real run must pin the
  exact frame and dialog order; hashes must not be interpreted as measured
  successes. The FreeDOS prompt fragments are also unverified hypotheses.
- Each wait has a positive `budget` in retired instructions as §11 sketches;
  exhaustion fails the build and prevents commit. This is a diagnostic cap,
  not elapsed time or a JIT-block count. No wall-clock delays are used. Future
  `run_for`/`--max-time` durations use virtual time, as core-contract.md §12.8
  requires; this corpus does not invent a host sleep action.
- `[output]` selects the captured disk, and the final `commit = "system"`
  emits a new base plus artifact metadata. Full-size disk bases are always
  SHA-256 identified and immutable; guest writes remain in a separate sector
  overlay until capture. The output records its parent, leaving hash-linked
  layers possible; commit never rewrites the parent. Publication permissions
  do not follow from a script completing: the DOS and Windows outputs stay
  private. No open licence claim is made for an installed image here.
- `[lock]` is an **inline draft snapshot** solely to compare readable pin
  structures in both syntaxes. It records the engine/CPU revision, deterministic
  run choices, parent disk, artifact inputs (including inherited firmware),
  captured disk hash/size and output manifest. It is not a replacement for
  §7.2's emitted manifest/config/metadata lock directory. Recipe-content pins
  belong in an enclosing recipe artifact, avoiding an inline self-hash.
  Missing exact engine/output pins make a build unverified, never silently
  reproducible. Zero-padded digests/revisions are placeholders.

READ: core-contract.md **§12.8**, not recipes.md, describes the shared future
key-script vocabulary. `crates/pc-harness/src/lib.rs` already drives FreeCOM through
`PromptThen`, `KeyboardStimulus` and set-1 make/break input; its small
`type_set1` helper does not support all the characters this corpus proposes.
`crates/unemupc/src/lib.rs` currently parses `--max-steps`, `--idle-limit`, `--diag-at`,
`--trace-exceptions` and `--print` for `run --machine`; `--keys` and
`--max-time` remain design work. The corpus therefore supplies no command line
that claims to run these files now. Input pacing, key names, graphics observers,
installer choices and budgets still need an implementation and guest validation.

READ: CPU revision and the three firmware package digests in each lock match
Cargo.toml and the shipped Firmwarefiles. MEASURED: hashing the specified
31,457,280 zero bytes produces the blank-disk hash in the two blank parents;
no base artifact was built. All other parent/media/output pins except the
FreeDOS media pin are placeholders. The MS-DOS two-disk layout, six-disk Windows
edition's exact Setup flow and all guest interactions remain ASSUMED.

### Where TOML reads badly

The cost is visible in a real long input list. The Windows destination and
confirmation take this much TOML (verbatim):

```toml
[[step.input]]
type = 'C:\WINDOWS'

[[step.input]]
key = "enter"

[[step.input]]
# ASSUMED checkpoint: Setup hardware selection.
budget = 400_000_000

[step.input.wait_state]
kind = "screen-digest"
card = "vga"
sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000409"
```

HCL makes the parentage local; in TOML the reader must remember which
`[[step]]` owns `[step.input.wait_state]`. The corresponding HCL is:

```hcl
input {
  type = "C:\\WINDOWS"
}
input {
  key = "enter"
}
input {
  # ASSUMED checkpoint: Setup hardware selection.
  budget = 400000000
  wait_state {
    kind = "screen-digest"
    card = "vga"
    sha256 = "sha256:0000000000000000000000000000000000000000000000000000000000000409"
  }
}
```

The DOS FDISK group repeats `[[step.input]]` 17 times. Reading it is linear;
editing it is less comfortable: a misplaced table header can silently attach
fields to the wrong input. Long command scripts also need the distinction
between characters and physical keys. From the DOS batch entry (verbatim):

```toml
[[step.input]]
type = '''
@ECHO OFF
PATH C:\DOS
PROMPT $P$G
'''

[[step.input]]
key = "ctrl+z"

[[step.input]]
key = "enter"
```

A single opaque `boot_command` string could be shorter, but would hide the
same key/wait ordering in another mini-language in **both** syntaxes.
The explicit input list keeps that ordering inspectable. HCL's heredoc is no
shorter for the command body; TOML's literal string keeps DOS backslashes plain.

The multiline FreeDOS match also stays plain (verbatim):

```toml
[[step]]
wait_text = '''
CORPUS-READY
SYSTEM=FREEDOS-1.4
ROOT=C:\FDOS
'''
card = "vga"
match = "contains"
budget = 400_000_000
```

HCL uses `wait_text = <<-TEXT` through `TEXT` for the same bytes, followed by
the same arguments inside `step { ... }`. Neither syntax resolves screen
whitespace, trailing-newline or graphics-mode semantics; those need explicit
observer rules. TOML multiline literal strings are readable here and avoid
escaping paths. A long screen transcript would obscure intent in either
syntax; three authored adjacent lines test the syntax without copying one.

### Measurement and verdict

MEASURED on 2026-11-20: Python `tomllib` parses all three TOML documents.
A throwaway literal-subset reader of HCL blocks, string/heredoc attributes,
booleans and decimal integers reconstructs exactly the same three trees.
This is a pair-equivalence check, **not** official HCL parser conformance.
Additional checks cover one verb per action, positive wait budgets, read-only
media references, commit/output/lock agreement, the DOS-to-Windows parent pin,
and the complete ordered seven/six-disk insert sequences. No corpus installation
was run.

Counts exclude blank lines and full-line comments; bytes include them:

| Installation | Steps | TOML lines | HCL lines | TOML bytes | HCL bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| FreeDOS 1.4 | 42 | 233 | 300 | 6,439 | 6,798 |
| MS-DOS 3.30 | 22 | 298 | 406 | 9,923 | 10,538 |
| Windows 3.1 | 27 | 261 | 350 | 9,285 | 9,831 |
| Total | 91 | 792 | 1,056 | 25,647 | 27,167 |

TOML's worst case is repeated absolute table paths in nested input groups;
HCL's braces show scope more clearly. Even at 42 top-level steps and a
17-action input group, TOML preserves visible execution order without an
escape language, computation or encoded key/wait tokens. Multiline literal
strings and DOS paths read well. HCL costs 33% more nonblank/noncomment lines
in this spelling, largely closing braces; line count alone does not choose a
syntax, but the gain in scope readability does not justify the fallback here.
**Keep TOML per decision-33.** The syntax-review gate has concrete evidence;
the schema and installation behaviour remain future work. If maintainer review
finds these nested lists unreadable, take point 7's HCL fallback before §14's
freeze, using these same trees. Do not postpone that choice until after 1.0.
