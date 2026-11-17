# Recipes: Mediafile, Firmwarefile and Machinefile

Status: **Proposed** (design step of `upc-5pw.113`), 2026-11-17. Nothing here is
implemented and nothing is frozen. The syntax is experimental (`recipe = "0.1"`)
until unemu-media has migrated and one private recipe works end to end; see §14.
The maintainer accepted the syntax, TOML at `0.x`, on 2026-11-17 after
reading the corpus (decision-33); HCL stays as the documented fallback.
On the same day he answered questions Q2 to Q7 (§16); none is open.
Accepted: decision-34 (inputs by content, the lock), decision-35 (recipe
artifacts and naming), decision-36 (optional parts), decision-38 (tags and
versions), decision-39 (work files), and then decision-37 (export
guarantees) with every remaining recommendation (§16, Also decided). All
seven records are accepted. The proposed order of work is §19. The
throwaway corpus check (§19.2, B1) ran the same day; its findings, the
fixes it made and the points it leaves open are §4.6. The corpus of
real recipes in two syntaxes is in [`recipe-corpus/`](recipe-corpus/README.md).

Markers: **[REPO]** = read in this repository or in unemu-media; **[SRC n]** =
external source listed in §18; **ASSUMED** = not verified, to be checked by the
throwaway checker or the named step; **MEASURED** = measured for this document.

## 0. The design in ten lines

1. A recipe is one **TOML 1.0** file that builds one artifact under one tag:
   a Mediafile (`[media]`), a Firmwarefile (`[firmware]`) or, later, a
   Machinefile (`[machine]`). It has a description part and an ordered list of
   steps from a closed set. What all releases of one work share (title,
   developers, genres, work ids) is written once, in a work file beside the
   recipes (§6.16). Nothing in it can run a program, read a host path
   or compute a string.
2. Every input is named by **SHA-256 and size**. Names, URLs and database ids
   are hints for finding it; the hash decides. Sources are the store, the
   download cache, the files beside the recipe, the user's libraries and,
   last and only when allowed, URLs.
3. Steps have **no working directory**. Each step reads named values (a file
   or a tree of files) and makes one named value. So the build is checked
   before it runs, and the checker knows exactly which outputs depend on
   which inputs.
4. A published recipe carries a **lock**: the output's manifest, config and
   metadata, byte for byte, as its author's `upcli` wrote them. A build makes
   the payload blobs, checks each against the lock and publishes the locked
   manifest unchanged. Only the payload builders must stay byte-stable for
   good; the encodings of manifests, configs and metadata need not.
5. If one required input is missing, **nothing is built** and the report
   lists every missing input by name, size, hash and database id.
6. **Optional parts are attachments**: separate OCI artifacts that refer to
   the main artifact (OCI 1.1 `subject`). The main artifact's digest never
   depends on which optional files a user has.
7. Recipes are OCI artifacts in a `recipes/` namespace beside their outputs:
   `unemu/recipes/media/ibm-pc/freedos:1.4-r1` builds
   `unemu/media/ibm-pc/freedos:1.4-r1`.
8. Tags are `<release>-r<N>`: the release names the software (version,
   edition, region, format), `rN` counts our packagings of it. Such a tag
   never moves. `<release>` alone is an alias that follows the newest `rN`.
   A metadata fix is a new `rN`; it downloads nothing, because every blob is
   already there.
9. Export is lossless and documented without `upcli`: every stored blob
   comes back byte-identical under its recorded file name, and a built file
   system also comes back as files with names and times.
10. What binds forever is short and listed in §9: the hash identity, the
    TOML subset and schema of each recipe version, the bytes each step
    writes, the tag grammar and the export layout.

## 1. Scope and terms

The unemu family keeps three kinds of images as OCI artifacts in a local store
and on registries (machine-images §2, §5.2, §5.3 [REPO]): machine images,
installation media sets and firmware packages. Today a person makes a set or
a package with `upcli image media import` or `image firmware import`, and
unemu-media builds the open ones with one hand-written `build.sh` per recipe
plus a `media.toml` or `firmware.toml` [REPO: unemu-media `docs/recipes.md`].
Those scripts run curl, unzip, mtools, git, tar and Python. They cannot be
handed to a user for private media, cannot be run by the window, and tie
every digest to one pinned `upcli` binary.

This document designs the replacement: a recipe format and engine that
builds media sets and firmware packages, and later machine images, from
inputs found by content.

| Term | Meaning |
| --- | --- |
| **Recipe** | One TOML file: a Mediafile, a Firmwarefile or a Machinefile. |
| **Input** | A file the recipe needs and does not contain, named by SHA-256 and size. |
| **Bundled input** | An input whose bytes travel with the recipe: our own text, such as a licence index. |
| **Value** | What a step reads or makes: a file, or a tree (paths to files, with times). |
| **Step** | One action from the closed set, with named operands and one named result. |
| **Output** | A part of the artifact: a medium, a ROM, an extra. |
| **Lock** | The output's manifest, config and metadata bytes, kept with the recipe. |
| **Attachment** | A separate artifact with optional parts, referring to the main one. |
| **Library** | A folder of the user's own files, or a catalogue that lists such files. |
| **Release** | The tag stem that names the software: `1.4-720k`, `usa-rev1`. |
| **Revision** | The count of our packagings of one release: the `rN` of a tag. |

READ: machine-images §2 (artifact types, annotations), §3 (store and tags),
§5 (blob policy), §5.2 (media sets), §5.3 (firmware packages), §5.6
(compatibility); decisions 25, 26, 27, 30 and 32; registry-library-plan §3
(attachments as referrers); deferred § Layered disk images.

## 2. Requirements

Each requirement is written so a test can decide it.

| Id | Requirement |
| --- | --- |
| R1 | One recipe file builds exactly one artifact under exactly one tag. The checker refuses a file with two identity tables. |
| R2 | A recipe has a description part and an ordered execution part. The execution part uses only the steps of §6.9; an unknown step, key or value is refused before anything is read or fetched. |
| R3 | No recipe can run a program or a shell, read a host path, read the environment or the clock, or build a string from other strings. The grammar has no construct for any of these. |
| R4 | Every input is identified by SHA-256 and exact size. A file is used only after its whole content has been hashed and both values match. |
| R5 | A build with every required input present publishes the artifact. A build with one missing required input writes no tag and no artifact, and reports every missing input with name, size, SHA-256 and database ids. |
| R6 | A recipe that holds only hashes, sizes, names, URLs and our own text builds the same artifact as one whose inputs are public. Publishing such a recipe publishes no payload byte. |
| R7 | For a pinned recipe, two builds on two hosts, with two `upcli` releases that both support the recipe's version, give the same manifest digest. |
| R8 | The main artifact's digest is the same whether or not the user has the optional inputs. |
| R9 | A build never uses a library's index as proof. An index or catalogue only proposes candidates. |
| R10 | Finding an input does not need a full library scan when a size, a name or a catalogue entry narrows the candidates. A scan hashes only files whose size equals a wanted size. |
| R11 | The engine and syntax contain no platform rule. A platform provider supplies media formats, geometry, file-system profiles, firmware roles and the `needs` vocabulary. |
| R12 | Every artifact exports to plain files, byte-identical for unmodified media, with a layout simple enough for a script without `upcli` (§3.10). |
| R13 | A recipe is itself an OCI artifact that a registry lists, and a client can tell from it, without any payload, whether the user can build it. |
| R14 | `upcli recipe check` decides every static rule of §6.11 without reading an input, the store or the network. |
| R15 | A fetch from a URL happens only with the user's consent for that recipe or publisher, reads at most the pinned size, and is hashed before use. |
| R16 | A media recipe states what the title needs from the platform family as features, never as machine models. The library warns and suggests; it never blocks. |

## 3. The model

### 3.1. Recipe, lock and artifact

```text
          recipe (TOML)                       lock
   description      steps            manifest, config, metadata
        |              |                 (exact bytes, no payload)
        |              v                          |
        |     inputs found by hash                |
        |              |                          |
        |              v                          v
        +------> payload blobs  -- each checked against a descriptor
                                     of the locked manifest
                                                  |
                                                  v
                                  artifact = locked manifest + blobs
```

There are two ways to run a recipe.

- **Pin** (the author, once per revision): `upcli recipe pin` builds the
  payload blobs, writes the config, the metadata and the manifest with the
  writers of that `upcli`, records the manifest digest in the recipe
  (`digest = "sha256:…"`) and writes the lock beside it.
- **Build** (everyone, with any later `upcli`): `upcli recipe build` builds
  the payload blobs, checks each against its descriptor in the locked
  manifest (decoded digest and size, and the stored digest and size), and
  publishes the locked manifest, config and metadata unchanged.

A recipe without `digest` is a **draft**. It builds, the result is tagged
locally and its digest is printed, and nothing is checked against a pin. A
user's own private recipe may stay a draft for ever.

Why a lock: today the digest of a set depends on how one `upcli` release
spells TOML and JSON, which annotations it writes and how it orders them.
unemu-media handles that by pinning one `upcli` binary [REPO: unemu-media
`upcli.toml`]. A user's later `upcli` cannot be pinned. The manifest gained
five annotations on 2026-11-15 and every set digest moved (machine-images
§5.2, Manifest annotations [REPO]); the same would happen at each later
improvement. With the lock, a later release never has to write an old
encoding again. It only has to read it, which decision-30 already requires.
The same idea is `Cargo.lock` and `flake.lock`: the description says what is
wanted, the lock says exactly what that came to [SRC 12].

The lock holds digests, sizes, labels, titles and annotations. It holds no
payload byte, so it is publishable with the recipe (R6).

### 3.2. Values, not a working directory

A Dockerfile step changes a file system that the next step sees [SRC 1]. That
hidden state is why a Dockerfile cannot be checked without running it. A
recipe step instead reads named values and makes one named value:

- a **file**: a byte string;
- a **tree**: an ordered list of entries, each a relative path, a file and
  an optional time. A tree has no host path, no owner and no mode.

A reference is a value's name (`edition`) or a member of a tree
(`disks/144m/x86BOOT.img`: the tree `disks`, the path after the first `/`).
Steps are written in order and a step may refer only to inputs and to
earlier steps. The file order is therefore a valid execution order, as the
maintainer asked, and it is also a dependency graph. Three things follow:

1. Every reference is checked before the build reads a byte (R14).
2. The checker knows which outputs each input feeds. An input is
   **required** exactly when a part of the main artifact depends on it, and
   optional when only attachments do. The recipe has no `optional` flag that
   could contradict the steps.
3. A path such as `../x` has nothing to escape from. Paths exist only inside
   trees.

### 3.3. Inputs by content

An input states `sha256` and `size`, plus a canonical `name` for people and
for name-first search. Everything else is a hint: other names, weaker hashes
that catalogues carry (`crc32`, `md5`, `sha1`), database ids and URLs.

The size is not decoration. It is what makes a library scan cheap: a scan
reads directory entries and hashes only the files whose size equals a wanted
size (R10). A 293,950,337-byte archive has one candidate. A 32,768-byte ROM
chip has many, and they hash in milliseconds.

A **file-set input** names many loose files at once: a list of
`{ path, size, sha256 }`. Its value is a tree. It serves a game that exists
as a folder (the AZURAMON recipe of the corpus has 561 files). Each file is
found on its own by hash, wherever it lies and whatever it is called there;
`path` is where the tree puts it.

Prior art. Nix calls this a fixed-output derivation, and `requireFile` is
the same contract for files that cannot be downloaded: the build names a
hash, and the user adds the file to the store [SRC 8]. PKGBUILD and
Homebrew pin `source` and `sha256sums`, but find by URL only [SRC 6, 7].
ROM managers (clrmamepro, RomVault, igir) rebuild a set from the user's
files against a DAT of names, sizes and hashes [SRC 10]; they copy and
rename but build nothing. A recipe is a DAT that can also build.

### 3.4. Resolution, libraries and the index

Sources are tried in this order. The first match wins, and every match is
hashed in full while it is copied into the store's staging area, with the
streaming ingest the importers use today (machine-images §3.1 [REPO]).

| # | Source | How a candidate is found | Network |
| --- | --- | --- | --- |
| 1 | The store | A blob whose decoded digest equals the hash. Needs an index of decoded digests, which the store lacks today; a rebuildable cache. | no |
| 2 | The download cache | A file named by its hash. | no |
| 3 | The recipe's bundle | A file beside the recipe file, or a layer of the recipe artifact. | no |
| 4 | Libraries | Size first, then name, then catalogue hashes (below). | no |
| 5 | URLs of the input | In the order written. | yes, with consent (R15) |

A **library** is a folder the user names (Preferences, or `--library`), as
ROM folders are named today. The library index is a rebuildable cache in the
store's `cache/` directory. For each file it records path, size, change time
and, once computed, SHA-256. It also looks **inside zip archives**: the
central directory gives each member's name, size and CRC-32 without
unpacking, so a ROM set kept as one zip per title is found as easily as
loose files. Zip is the only container in v1; §13 lists the others.

Within a library, candidates for one input are tried in this order: files of
the wanted size with the canonical name; with another listed name; with a
matching `crc32`, `sha1` or `md5` from a catalogue or a zip directory; then
every other file of that size. Each candidate is hashed until one matches. A
computed hash is kept in the index under (path, size, change time), so a
later build does not hash the file again to *find* it. It is still hashed
again when it is *used* (R4, R9).

**Found instead of made.** The same search serves every value whose hash
and size the recipe or the lock states, not only inputs: a member of an
`unpack` step that gives `size` and `sha256`, and every payload blob of a
locked recipe. Such a value is looked for in sources 1 to 4 first and made
only when it is not found. An archive is opened only for the members still
missing, and its own input is then required; if every member is found, the
archive is never fetched. A user who has the 19 files of Ken's Labyrinth
and not the zip can build the set; a user who already has the finished
disk in the store builds nothing. The plan says which case holds:
`LABFULL.ZIP: not needed, its 19 members were found`. This rule is the
earlier design's (its "leaves and sources"), taken over because it is the
content-addressed idea applied one level further.

It has one consequence that §6.9 spells out: a build must give the same
bytes whichever source supplied a value, so **nothing but bytes is ever
read from an archive**. Order and times are written in the recipe.

A **catalogue** can stand in for a scan (point 7 of the brief). Three forms
are read in v1:

- a CSV of `path,size,sha256` (extra columns `crc32`, `sha1`, `md5` allowed);
- a `SHA256SUMS` file (`<hex>  <path>`), sizes read from the files;
- a Logiqx DAT (the XML of No-Intro, Redump and TOSEC) together with the
  folder its names are relative to [SRC 10].

A catalogue row is a claim, never proof. A row that leads to a file with
another hash is reported as stale and skipped.

### 3.5. The missing-input report

`upcli recipe plan` resolves without writing. For every input it prints one
row: found (and where), or missing. For a missing input it prints the
canonical name, size, SHA-256, every database id, what the input is used for
(the labels of the outputs that depend on it) and any **near miss**: a file
with the canonical name but another size or hash. That last line is what
tells a user "you have another version of this disk". `--json` gives the
same data for the window. The summary line is the one the Remotes view
shows: `can build: all 12 inputs found`, `missing 3 of 12`, `built`,
`can build; 2 optional parts not found`.

### 3.6. Steps

v1 has three steps (§6.9): `unpack` (members of a zip archive), `fat`
(one or more FAT volumes from a tree) and `text` (our own text with DOS line
ends). Unmodified media and ROM chips need no step at all: an output refers
to the input directly. The corpus shows that these three cover every open
recipe of unemu-media except one output, the FreeDOS source archive (§12).

A step is a pure function of its operands and of the recipe's version. It
reads no clock, no locale, no host file name and no host time. A new step,
or a change to the bytes a step writes, needs a new recipe version (§9).

### 3.7. Outputs

A Mediafile lists `[[medium]]` entries in installer order and `[[extra]]`
entries; a Firmwarefile lists `[[rom]]` entries and `[[extra]]` entries. Each
names the value it stores. They map one to one to the entries of today's set
and package configs (§6.14), so the artifact a recipe makes is an ordinary
media set or firmware package. Nothing downstream changes.

### 3.8. Verification

For a pinned recipe the build checks, in this order:

1. the recipe's `digest` equals the SHA-256 of the lock's manifest;
2. the manifest's config and metadata descriptors match the lock's config
   and metadata bytes;
3. the lock agrees with the recipe: platform, artifact type, the media, ROMs
   and extras in order with their ids, labels, formats and file names, and
   the title record's fields that the recipe file itself states. This is a
   comparison of decoded fields, not of bytes, so a later release can make
   it. A field that comes from the work file (§6.16) and differs from the
   lock is not an error: the lock wins, and `check` reports it as a change
   the next revision will take up;
4. every payload blob the steps made matches its descriptor: decoded SHA-256
   and size, then stored SHA-256 and size;
5. every descriptor of the manifest is satisfied, and no blob is left over.

A failure at 1 to 3 is a broken recipe and stops before any input is read. A
failure at 4 names the output, the expected and the actual hash, and tags
nothing. Every input was verified by hash, so such a failure means a step
no longer writes the bytes it once did: the message asks for a report.
`--keep-as TAG` keeps the differing result under a local tag of the user's
choosing, never under the recipe's tag.

Point 4 has one hard case. A payload stored with `+zstd` has a stored digest
that depends on the encoder. RFC 8878 fixes the format, and nothing found
for this document promises that an encoder's output stays the same from one
library version to the next (ASSUMED not stable [SRC 13]; this repository
pins the library exactly for that reason). §9 and question Q3 deal with
it.

### 3.9. Optional parts

A Windows 3.0 recipe wants the installation disks and also the manuals,
because the project is an archive; but the manuals are not needed to run
Windows, and few users have them. The requirement is R8: the set's digest
must not depend on them.

An optional part is therefore an **attachment**: its own small OCI artifact
whose manifest names the main artifact as its `subject` (OCI image-spec 1.1
[SRC 11]). A recipe declares attachments as `[[attachment]]` tables, each
with its own extras and its own pinned digest. An attachment is built when
every input it depends on is found, and skipped otherwise; within one
attachment it is all or nothing. The main artifact is built from required
inputs only. Which inputs are required follows from the steps (§3.2), and
`recipe check` prints the split, so an author sees at once when a manual has
become required by mistake.

This is the shape registry-library-plan §3 already accepts for covers,
manuals and scans [REPO], so the window shows recipe-built documents and
later attachments in one place. It needs what that plan needs: a `subject`
field in the manifest codec and a local table of referrers.

Rejected: one artifact with the optional layers listed and some blobs absent
(the store and every registry refuse a manifest with a missing layer); one
pinned digest per combination (2^k digests under one tag); a second
ordinary set for the documents (a set needs at least one medium).

### 3.10. Export

`upcli image export REF --to DIR` writes every part of an artifact as a
plain file (decision-37):

- each medium and ROM chip: the decoded bytes, byte-identical to the input
  for unmodified media, under its recorded file name;
- each extra under its `file_name`;
- `config.toml`, `metadata.json` and `manifest.json` as stored;
- `SHA256SUMS` over all of them;
- `EXPORT-VERSION`, the version of this layout.

No medium, ROM chip, source archive or extra takes one of these five names,
ignoring case, and in a firmware package none takes `README.md`, the name of
its README layer.

`--files` also writes the content of every built file system as a folder
tree with the names and times the recipe pinned, with the volumes of a split
set merged into one tree.

Three limits are stated instead of hidden. A container that a recipe
unpacked is not rebuilt: the user gets the members the recipe took, not the
zip. Members the recipe did not take are not in the artifact. And host file
times are never stored, so `--files` gives the recipe's times, not the ones
the user's copies had. `recipe build` therefore prints, per input, `kept as
stored`, `kept as files` or `not kept: N members unused`, so a user who
means to delete the originals knows what stays. A recipe that wants the
archive itself kept lists it as an extra.

So that a script needs no `upcli`: every layer descriptor carries the
annotation `org.opencontainers.image.title` with its export file name, the
convention ORAS uses to name pulled files [SRC 11]. A layer whose media type
ends in `+zstd` is one Zstandard frame titled with `.zst` after its file
name (`x86BOOT.img.zst`); `zstd -d` on the pulled file gives the decoded
bytes under the file name itself, and `unemu.decoded.digest` is their
SHA-256 (machine-images §2, Export file names [REPO]; decision-37,
amendment of point 3). The name rules and the uniqueness of names apply to
the decoded name. MEASURED on 2026-11-17: releases 0.0.1 and 0.0.2 read sets
and packages with a title on every layer; scratch builds of each tag read
titled copies of the artifacts that release builds itself through its own
decoding, store resolution, library summaries, New machine, firmware edit,
set re-import and registry pull.

## 4. The syntax decision

### 4.1. What the syntax must carry

Measured on the corpus: a description of 10 to 20 short fields and one long
text; lists of names; an SPDX expression of up to 256 characters (§4.6); 1 to 3
inputs for most recipes, 561 for one; 0 to 2 steps; 1 to 12 media; DOS
names and paths with backslashes; file times without a time zone.

It must also carry nothing else: no variables, no interpolation, no
conditionals, no loops, no includes. R3 is easier to keep when the host
language cannot express them than when a checker has to forbid them.

### 4.2. Candidates

| | TOML 1.0 | HCL (a literal subset) | Own line format, Dockerfile-like |
| --- | --- | --- | --- |
| Specification | Frozen since 2021, with a conformance suite [SRC 3] | A syntax specification, defined in practice by the Go implementation [SRC 2] | Ours to write and keep |
| Independent parsers | Every language; Python has one in its standard library | Go (reference), Rust `hcl-rs` (one maintainer, 0.x) [SRC 2], Python third party | None |
| Already in this project | Machine files, all three artifact configs, `media.toml`, `firmware.toml`, the `toml` crate [REPO] | No | No |
| Description part | Good: tables, arrays, multi-line strings | Good: attributes, blocks, heredocs | Weak: a Dockerfile has only `LABEL key=value` [SRC 1] |
| Ordered steps | `[[step]]` arrays of tables, ordered by the specification | Blocks, ordered within a body | Best: one instruction per line |
| Block labels | No; an `id` key instead | Yes: `step "fat" "disk" { … }` | By position |
| Big tables of hashes | One inline table per line | One object per line | Best: columns |
| DOS paths | Literal strings: `'C:\WINDOWS'` | Escapes: `"C:\\WINDOWS"`; `${` and `%{` start a template | Ours to define |
| Zone-free file times | Native local date-time | Strings | Ours to define |
| Can express computation | No | Yes: expressions, functions, templates, `for`; a subset must forbid them [SRC 2] | Whatever we add, for ever |
| Editor support | Highlighting everywhere; completion and validation from a published JSON Schema (Taplo, Even Better TOML) [SRC 3] | Highlighting; no schema mechanism for a custom dialect | None |
| Known by | Rust and Python users; reads like an INI file | Terraform, Packer, Nomad and Docker Bake users [SRC 2] | Looks familiar; every rule is new |
| Prior art for our problem | Cargo, pyproject; step lists as data in GitHub Actions and Ansible (YAML) [SRC 5] | Packer builds machine images by typing into an installer [SRC 2] | Dockerfile, RPM spec, PKGBUILD [SRC 1, 6] |

Others were set aside early. YAML has ordered steps and millions of users,
but implicit typing and a specification that parsers implement differently;
it is the wrong base for a file that must mean the same in twenty years. KDL
is the maintainer's own example of a language nobody knows. JSON has no
comments. Starlark, Nix and Lua are programs. XML is what the ROM community
already reads (DATs and MAME software lists [SRC 9, 10]), but it is
unpleasant to write and brings entity attacks; recipes export a DAT instead
(§6.15).

### 4.3. The maintainer's objection to TOML

The objection was that a recipe needs a description and an ordered execution
part, and TOML has only the first. The corpus does not bear that out:

- **TOML arrays of tables are ordered.** `[[step]]` tables are read in the
  order written; that is in the specification, not a parser habit [SRC 3].
- **A step is a table with one verb key**, as an Ansible task is a map with
  one module key and a GitHub Actions step has either `uses` or `run`
  [SRC 5]. The verb set is closed and shares no name with any argument, so
  the rule "exactly one verb" does not depend on key order.
- **The execution part is short.** Once inputs are found by hash and media
  are taken unmodified, most of a build script disappears. MEASURED on the
  corpus: FreeDOS 1.4 needs two steps where `build.sh` has 356 lines; Ken's
  Labyrinth needs three; the firmware packages, Windows 3.0, the Amiga disk
  and the NES cartridges need none. The longest step list in the corpus is
  three. A syntax chosen for long scripts would be chosen for a case the
  model removed.

Where TOML is worse, measured on the same corpus (§4.4): no block labels, so
a step carries `id = "…"`; an inline table cannot span lines in TOML 1.0, so
a medium is a four-line `[[medium]]` table where HCL writes one labelled
block of the same length; and `[[step]]` headers are noise that a
line format would not have.

### 4.4. Evidence from the corpus

Every recipe of the corpus exists in TOML and in HCL, generated from one
data model, so the two differ only in syntax. Sizes are in
`recipe-corpus/README.md`. MEASURED over the thirteen files: 1490 lines and
109,551 bytes in TOML, 1565 lines and 112,656 bytes in HCL, without blank
lines and comments. HCL is 5% longer in lines and 3% in bytes; on no recipe
do the two differ by more than a tenth, except the smallest. Neither syntax
needed a workaround that the other avoided. HCL reads better where a block
has a kind and a name (`step "fat" "disk" {`) and where blocks nest. TOML
has two value types the recipes use and HCL lacks: a date-time without a
zone, and hexadecimal integers. Backslashes, where TOML's literal strings
would win, do not occur in any v1 recipe; they will in Machinefiles. The
TOML files were parsed by a TOML 1.0 parser; the HCL files were checked by
eye only, which is itself a small piece of evidence about tooling at hand.

### 4.5. Decision

Accepted by the maintainer on 2026-11-17, after reading the corpus in both
syntaxes: TOML, at `0.x`, with HCL kept only as the documented fallback
(decision-33).

**TOML 1.0, restricted as §6.1 says.** The reasons, in order of weight:

1. **Permanence.** A published recipe must mean the same for good. TOML 1.0
   is frozen by someone else, has a conformance suite and a dozen
   independent parsers. An HCL subset would be a language we define and
   police; a line format would be one we also have to invent.
2. **It cannot compute.** There is no interpolation to forbid, no function
   to refuse, no template sequence to escape. R3 holds by construction, and
   so does the security argument of §6.13.
3. **Anyone can read a recipe.** A twenty-line Python script with the
   standard library reads a Mediafile and checks a folder against it. For an
   archive that wants its recipes to outlive its tools, that matters more
   than block labels.
4. **One language in the project.** Machine files, configs and today's
   manifests are TOML. Migrating `media.toml` is mostly renaming keys.
5. **Editors.** One published JSON Schema gives completion, hover text and
   validation in the common editors, for all three file kinds.

**Costs, stated plainly.** Steps are wordier than in HCL or a line format.
References are plain strings, so only the checker tells a value's name from
a text. Long inline tables stay on one line. People who know Terraform will
find HCL more natural, and people who know Docker will miss `COPY`.

**The fallback is cheap.** The model of §3 and the schema of §6 are a tree of
tables, arrays and scalars, and the corpus generator writes both syntaxes
from it. If the Machinefile corpus, which does not exist yet and will have
the long step lists, shows TOML to be unreadable there, the fallback is the
HCL subset of the corpus with the same schema. That is a second parser, not
a second design. The syntax stays `0.x` until that corpus exists (§14).

### 4.6. Findings of the corpus check (B1)

Done on 2026-11-17 for `upc-5pw.113.1`, before any product code, as §14 and
§19.2 (B1) ask. Its fixes are already in §6 and in the corpus; what needs a
decision is the open list at the end.

**What was checked.** A throwaway Rust program of about 2,000 lines,
outside the repository and not kept, read all 18 `*.toml` files of the
corpus: 15 recipes and 3 work files. It used the `toml` crate at the
version the workspace pins (1.2.1+spec-1.1.0, whose parser is
`toml_parser` 1.1.5) and a serde model of §6 that refuses unknown keys at
every level. It applied §6.1, merged work files by §6.16 and ran every
rule of §6.11; rule 8 only as "a `digest` without a lock", since nothing is
pinned. Platform data came from mock providers: `ibm-pc` with the media
formats, ROM roles, boards and lanes of `pc-image`, the needs vocabulary of
§6.7 and the two firmware catalogue entries the corpus names [REPO]; `nes`
and `amiga` with the corpus's own ASSUMED words. A real checker refuses the
five recipes of those two platforms at `platform`, as rule 7 says; the mocks
let the rest of each recipe be checked. The `.hcl` files were not read:
they are the fallback's evidence (decision-33).

**Per recipe.** MEASURED, after the fixes below.

| File | Result |
| --- | --- |
| `freedos-1.4-floppy`: three recipes and `freedos.work` | Pass. Each medium's member size fits its format. |
| `freedos-1.4-livecd` | Pass. |
| `kenslab-2.01/kenslab-2.01` and `kenslab.work` | Pass. |
| `kenslab-2.01/kenslab-1.0-shareware` | Failed, fixed: it had no `[license]` and was named under `unemu/` (§7.3). |
| `azuramon/azuramoon-2026` | Pass: 561 file-set entries, all 8.3 names, ten volumes. |
| `windows-3.0/windows-3.0-1.2m` | Pass. A warning: the placeholder size of `users-guide`, 1 byte, would meet the 256-byte guard of §6.13 at push. |
| `unemu-at-bios`, `ibm-5170-bios` | Pass. Licence, size and hash agree with the firmware catalogue. |
| `contra-nes`: four recipes | Pass, through the work file. |
| `contra-nes/contra.work` | Fails: `software_platform = "NES"` is not in metadata schema 1's list (O1). |
| `amiga-adf/amdemo` | Fails: `software_platform = "AmigaOS"`, the same (O1). |

The split of rule 5: every input is required except Windows 3.0's
`users-guide`, which only the attachment uses. No recipe has an unused
input or step, a reference forward or a duplicate name.

**Fixes made.** Each is small and in place.

| Where | Fix |
| --- | --- |
| `kenslab-1.0-shareware` | Gains the `[license]` its folder's README already called for (the publisher's terms), PLACEHOLDER like the rest of its data, with the terms text as an extra from the archive. 35 lines became 45. |
| §6.1 | The ASSUMED note on the `toml` crate is now MEASURED, with the method. |
| §6.2 | `work` is media only; `[machine]` is reserved and a 0.1 checker refuses it; the extras of attachments share the output-id namespace. |
| §6.6 | The source rule per kind, as the importers apply it: a set's expression that is not one `LicenseRef-`, a firmware package's copyleft one. A ROM the catalogue lists as user-supplied takes no `[license]`. An expression is at most 256 bytes, as the store keeps it. |
| §6.8 | `ids[].db` is one of `no-intro`, `redump`, `tosec`, `mame`, and a MAME `set` may be a machine's ROM set: the IBM 5170 BIOS is one, not a software list. A file-set entry's `time` has a member's form. |
| §6.9 | A member's time has a year from 1980 to 2107, which FAT and zip can hold. |
| §6.10 | Of a part's keys, `lane` and `from` are required, `position` and `part_number` optional, and `offset` defaults to 0, as in the package config. |
| §6.11, rule 3 | Also a member path the tree does not list: every tree of 0.1 lists its members. |
| §6.16 | A work file states `recipe`, as the example and all three corpus work files do. |
| §10 | NES `tv_system`, not `tv-system`: keys are `snake_case`. |
| §4.1 | The longest SPDX expression is 256 characters (FreeDOS), not 400. |

The FreeDOS expression is exactly at the store's 256-byte limit (MEASURED).
One more licence in a later FreeDOS release would not fit; that is a limit
of the store to revisit then, not of recipes.

**TOML 1.1.** MEASURED with a probe of 19 snippets.

| Parser | TOML 1.1 cases refused | TOML 1.0 controls parsed |
| --- | --- | --- |
| `toml` 1.2.1 alone, the workspace's | 0 of 11 | 7 of 7 |
| `toml` 1.2.1 after the pre-pass below | 11 of 11 | 7 of 7, none flagged |
| `toml` 0.9.2 with `toml_parser` 1.0.1 | 6 of 6 | 1 of 1 |
| Python 3.13 `tomllib` | 6 of 6 | 1 of 1 |

The crate has no switch for TOML 1.0, neither a feature nor an option, and
`toml_datetime` 1.1 even models a time without seconds as
`second: Option<u8>` [READ: the crate sources]. Three ways out were
weighed. A version pin works, since 0.9.2 refuses all four forms, but holds
the whole workspace on an old crate line for good. A second, strict
parser means two grammars that can disagree. The way taken is a pre-pass
over the event stream of `toml_parser`, the parser inside the crate, about
120 lines. It refuses a line break or a comment while the innermost open
container is an inline table, a value separator followed by that table's
close, `\e` or `\x` in a basic string or a quoted key, and a bare value
whose time has hours and minutes and no seconds. Its controls include a
multi-line array inside an inline table, an escaped backslash before `e`,
`'C:\x'` in a literal string and a line-ending backslash. Recommendation
for B4: the pre-pass, proved on the TOML 1.0 cases of `toml-test`
[SRC 3], every valid one parsed and every invalid one refused by the
pre-pass and the crate together (ASSUMED: the suite can select its 1.0
cases; not tried).

**Error messages.** MEASURED on 44 broken copies of corpus files, one
change each: an unknown key, a float for an integer, each 1.1 form, a
byte-order mark, a reference to a later step, a member the tree lacks, a
licence that names no extra, an unknown need in a work file, a `digest`
without a lock, a release field in a work file, and so on. The checker
refused all 44, and every message names the file, line, column, key path
and rule:

```text
kenslab-2.01/kenslab-2.01.media.toml:15:1: error[6.1] media.revison: unknown field `revison`, expected one of `name`, `platform`, `release`, `revision`, `digest`, `min_app_version`
kenslab-2.01/kenslab-2.01.media.toml:49:27: error[6.1] step[1].members[1]: a line break inside an inline table is TOML 1.1 syntax; recipes are TOML 1.0
kenslab-2.01/kenslab.work.toml:20:9: error[6.11.7] needs.video: "8514" is not one of mda, hercules, cga, ega, mcga, vga, svga, or none
kenslab-2.01/kenslab-2.01.media.toml:96:8: error[6.11.3] extra[1].from: the tree "files" has no member "READ.ME"
```

What B4 should do better than the throwaway:

- serde stops at the first error of a file, and its type errors name Rust
  types (`invalid type: floating point 940242.0, expected i64`).
- `toml::de::Error` carries a span but no public key path. The checker
  recovered the path from the spans of `toml::de::DeTable`, the innermost
  key or value around the error; B4 can do the same. A missing key is still
  reported at the table header, without the array index
  (`medium: missing field format`).
- Errors cascade. A broken reference also leaves its target unused
  (rule 4), and a step with two verbs makes every reference to it unknown.
  Rule 4 should skip values behind a failed reference.
- A work file's needs are checked against each recipe's platform, so one
  bad value is found once per recipe. The message names the work file, and
  the copies must be merged.

**JSON Schema.** One schema covers the three file kinds 0.1 specifies,
Mediafile, Firmwarefile and work file:
[`recipe-schema-0.1.json`](recipe-schema-0.1.json), draft 2020-12, about 560
lines. It dispatches on `work` being a table (a work file) and on
`[firmware]`, and carries the `ibm-pc` vocabularies under an `if` on
`platform`; other platforms get the shape only. MEASURED with Python
`jsonschema` 4.26, and the schema itself passes the 2020-12 metaschema. On
the corpus it gives exactly the checker's failures: the two above and,
before the fix, the shareware recipe's. Of the 44 broken copies it refuses
27, six of them only because Python's TOML 1.0 parser refuses the file
first.

It expresses every key, type and required key; the closed vocabularies;
ids, hashes, the release grammar with its ban on an `rN` part, a local time
with even seconds, 8.3 names and volume labels as patterns; one verb per
step with that verb's keys (`oneOf` of closed objects); a file or a file
set; a ROM from one value or in parts; a bundled input of at most 1 MiB and
a disclosable file under 256 bytes; no `[license]` means not under
`unemu/`; the source rules of §6.6; `synopsis` only in a work file and the
release fields only in a recipe.

It cannot express the following, so `upcli recipe check` stays the
authority:

1. Syntax: the 1.1 forms, a byte-order mark, the 1 MiB limit. A schema sees
   data after parsing, and an editor whose TOML parser reads 1.1 accepts
   them (ASSUMED for Taplo and Even Better TOML; not tried).
2. Types the TOML-to-JSON mapping loses: `integer` accepts a float without
   a fraction (`940242.0` passed), and a local date-time becomes a string,
   so a time written as a quoted string passes.
3. References: an input or an earlier step, a member the tree lists, a file
   or a tree, unused inputs and steps, `terms` and `source` naming an extra.
4. Uniqueness of one key across items: output ids over four arrays, value
   names over inputs and steps, file-set and member paths, DOS names.
   `uniqueItems` compares whole items only.
5. Sums: at most 256 outputs and 100,000 file-set entries, each counted
   over several arrays.
6. Equality between fields or files: the work id and the last component of
   `name`; the platform segment of `name`, except per known platform;
   `players.min` at most `max`; sizes and hashes against the catalogue; the
   work file's existence and the merge; the lock beside a `digest`.
7. A work file's needs: a work file states no platform.
8. SPDX: the nesting of the grammar and the identifier list; a character
   pattern stands in.
9. Good messages: a failed `oneOf` says "is not valid under any of the
   given schemas"; the validator's best match recovers the useful one.

A keyword beside `$ref` needs draft 2019-09 or later. Which editors read
draft 2020-12 is ASSUMED, not tried.

**Open after B1.** Decisions, not fixes.

- **O1. Software platforms of new platforms.** `software_platform` is
  metadata schema 1's list (`image_store::metadata`), which the importer
  enforces and the codec keeps open [REPO]. It has neither `NES` nor
  `AmigaOS`, so `contra.work` and `amdemo` fail. Either the list grows,
  append-only (§9, row 7), when a platform arrives, or each provider owns
  its part of it. The keys of `[ids]` (`wikidata`, `mobygames`, `igdb` in
  the checker) raise the same question.
- **O2. Four more static checks**, as a rule 10 of §6.11: a stated size the
  medium's format does not allow; a `fat` step whose format differs from
  the medium it feeds; a name in a `fat` tree that the volume profile
  refuses (§6.9 leaves it to the build, but the names are in the recipe);
  a ROM whose size or hash differs from its catalogue entry. The checker
  had all four; none fires on the corpus.
- **O3. Who keeps the schema.** It holds provider vocabularies, so a copy
  kept by hand drifts. Either `upcli` generates it from the providers (B4)
  and the freeze publishes it (B17), or it is kept by hand.
- **O4. A work file's version.** May a recipe of one version name a work
  file of another? It matters from the second version on.
- **O5. A work whose releases differ in openness.** A shareware episode
  under `unemu/` and a registered version under `private/` are two
  repositories of one work, against "the work is the repository" (§6.16).
  The corpus avoids it only because both Ken's Labyrinth releases are
  open.

## 5. A first example

Ken's Labyrinth 2.01, complete. It names the author's archive, takes its
19 files in the archive's order with the archive's times (written out, so
the files can also be found loose), builds one 1.44M disk and attaches the
two licence texts. `digest` is absent: no builder exists
yet, so every recipe of the corpus is a draft.

```toml
recipe = "0.1"

[media]
name = "unemu/media/ibm-pc/kenslab"
platform = "ibm-pc"
release = "2.01"
revision = 1

[title]
title = "Ken's Labyrinth"
version = "2.01"
edition = "Full Version"
year = 1993
publishers = ["Epic MegaGames"]
developers = ["Ken Silverman", "Andy Cotter"]
genres = ["first-person shooter"]
software_platform = "DOS"
labels = ["game", "freeware", "first-person-shooter"]
description = "…"

[license]
expression = "LicenseRef-KensLabyrinth-Freeware"
terms = "readme"

[needs]
cpu = "286+"
video = ["vga"]
sound = ["sb", "adlib", "pc-speaker", "none"]

[input.labfull]
name = "LABFULL.ZIP"
size = 940242
sha256 = "643aa0ca8d1a7d2c88afecd952da00f3b912be8b83c4c1d2f4ab9b50491fadfd"
urls = ["https://advsys.net/ken/klab/labfull.zip"]

[input.licenses-lf]
name = "licenses.txt"
size = 3442
sha256 = "b4b52913919b8f481289fe0beef2a76a812b4014e6a2b09357caa2aec91f4ff7"
bundle = true

[[step]]
id = "files"
unpack = "labfull"
format = "zip"
complete = true
members = [
    { path = "BOARDS.KZP", size = 55110, sha256 = "4cf55fc511b52ac063e1f717440ff98fedb85ccf5912e96051bf449b4a6c9be2", time = 1993-03-27T13:58:22 },
    # … 18 more, in the archive's order
]

[[step]]
id = "disk"
fat = "files"
format = "1.44M"
label = "KENSLAB"
serial = 0x20080703
order = "listed"
time = 2008-07-03T00:00:00

[[step]]
id = "licenses"
text = "licenses-lf"
eol = "crlf"

[[medium]]
id = "disk-1"
label = "Disk 1 of 1 - the game (labfull.img)"
format = "1.44M"
file_name = "labfull.img"
from = "disk"

[[extra]]
id = "readme"
file_name = "README.TXT"
dos_name = "README.TXT"
kind = "document"
from = "files/README.TXT"

[[extra]]
id = "licenses"
file_name = "LICENSES.TXT"
dos_name = "LICENSES.TXT"
kind = "document"
from = "licenses"
```

## 6. Specification of version 0.1

Version 0.1 is the candidate for 1.0. Everything in this section is open to
change until the freeze of §14.

### 6.1. The file

- UTF-8 without a byte-order mark, at most 1 MiB.
- **TOML 1.0.0** [SRC 3]. Syntax that only TOML 1.1 accepts (a line break or
  a trailing comma inside an inline table, the `\e` and `\xHH` escapes, a
  time without seconds) is refused, so that every TOML 1.0 parser reads
  every recipe. MEASURED (§4.6): the `toml` crate this repository uses
  [REPO: `Cargo.lock`, `toml 1.2.1+spec-1.1.0`] reads all four and has no
  1.0-only mode, so the checker refuses them itself, in a pre-pass over the
  events of `toml_parser`, the crate's own parser.
- Value types used: string, integer, boolean, local date-time, array, table.
  A float, an offset date-time, a lone date or a lone time is refused.
- A key that this section does not name is refused, at every level. So is a
  value outside a stated vocabulary. There is no "ignored for forward
  compatibility": a recipe that a release does not fully understand is not
  built by it.
- Keys are `snake_case`. An **id** matches `[a-z0-9][a-z0-9._-]{0,63}`, the
  store's media-id grammar (machine-images §3 [REPO]).
- A recipe is named `<anything>.media.toml`, `<anything>.firmware.toml` or
  `<anything>.machine.toml`. The suffix is a convention for people and
  editors; the identity table decides the kind. The lock of `x.media.toml`
  is the directory `x.media.toml.lock/` (§7.2).

### 6.2. Top level

| Key | Kind | Required | Content |
| --- | --- | --- | --- |
| `recipe` | string | yes | The recipe version, `"0.1"`. It selects the schema and the bytes every step writes (§9). |
| `work` | string | no | The id of the work this is a release of. Its shared description is read from `<id>.work.toml` beside the recipe (§6.16). Media only: a work file holds only media tables. |
| `[media]`, `[firmware]` or `[machine]` | table | exactly one | The kind and the output's identity (§6.3). |
| `[title]` | table | media: yes | The title record (§6.4). |
| `[package]` | table | firmware: yes | The package description (§6.5). |
| `[license]` | table | no | The open statement (§6.6). Absent: the artifact is user-supplied. |
| `[needs]` | table | no | What the title needs from the platform (§6.7). Media only. |
| `[ids]` | table | no | Database ids of the work (§6.8). |
| `[input.<id>]` | tables | no | The inputs (§6.8). |
| `[[step]]` | array | no | The steps, in order (§6.9). |
| `[[medium]]` | array | media: at least one | The media, in installer order (§6.10). |
| `[[rom]]` | array | firmware: at least one | The logical ROMs (§6.10). |
| `[[extra]]` | array | no | Loose files of the artifact (§6.10). |
| `[[attachment]]` | array | no | Optional parts (§6.10). |

Inputs and steps share one namespace of value names. Media, ROMs, extras,
attachments and the extras of attachments share one namespace of output
ids.

`[machine]` is reserved for the Machinefile (§11). Version 0.1 gives it no
keys, so a 0.1 checker refuses a file that has it.

### 6.3. Identity: `[media]`, `[firmware]`, `[machine]`

| Key | Required | Content |
| --- | --- | --- |
| `name` | yes | The repository, without host and tag, in the OCI repository grammar: `unemu/media/ibm-pc/freedos`. |
| `platform` | yes | The machine family the provider routes on: `ibm-pc` (decision-32). |
| `release` | no | The tag stem (§8): `1.4-720k`. Left out only when nothing names the software: a firmware package whose repository is its catalogue id, a disk nobody has identified. |
| `revision` | yes | A whole number from 1. |
| `digest` | no | `sha256:<hex>`, the manifest digest of the locked output. Absent: a draft. |
| `min_app_version` | no | Raises the computed `unemu.min-app-version` (decision-30, point 5). It may never lower it. |

The tag is `<release>-r<revision>`, or `r<revision>` when there is no
release. It is never written in the file, so the two cannot disagree.

### 6.4. `[title]` (Mediafile)

One title record of metadata schema 1 (machine-images §2.1 [REPO]), with the
schema's own field names, and the image-level labels.

| Key | Type | Maps to |
| --- | --- | --- |
| `title` (required), `sort_title`, `series` | string | the same fields |
| `version`, `edition`, `source_media`, `description` | string | the same fields |
| `requirements` | string | the box's own words ("IBM PC, XT, AT; 640K; CGA, EGA or VGA"): shown beside `[needs]`, never evaluated (metadata schema 2; from the earlier design) |
| `year` | integer 1 to 9999 | `release_year` |
| `publishers`, `developers`, `genres`, `languages`, `regions` | arrays of strings | the same fields |
| `software_platform` | string from the schema's list (`DOS`, `Windows 3.x`, …) | the title's `platform` |
| `players` | `{ min, max }` | `players` |
| `labels` | array of ids | the envelope's `labels` |

The record's stable `id` is the last component of `name`. A set has one
title record; a recipe cannot state two. With a work file, `title` and
every other shared field may be left to it (§6.16); `title` is then
required of the two together.

### 6.5. `[package]` (Firmwarefile)

`title` (required; the config's `name`), `vendor`, `version`, `date` (a
string, ISO calendar date), `description`, `notes`, `labels`, and
`[package.fits]` with `boards` and `machine_classes`: the fields of the
firmware-package config and its descriptive metadata (machine-images §5.3
[REPO]).

### 6.6. `[license]`

| Key | Content |
| --- | --- |
| `expression` | An SPDX licence expression, or one `LicenseRef-` for a vendor's own terms. At most 256 bytes with single spaces, as the store keeps it. |
| `terms` | The id of the `[[extra]]` that holds the licence text. |
| `source` | The id of the `[[extra]]` that holds the corresponding source archive. |

This is today's open statement (`--open --license --license-text --source`)
with the same rules and refusals (machine-images §5.2, Redistributable sets
[REPO]): a set's expression that is not one `LicenseRef-` needs `source`,
and so does a firmware package's expression with a GPL, LGPL, AGPL or MPL
identifier anywhere in it; `terms` is required for a set; a firmware
package's expression must equal the one the firmware catalogue records for
its ROM, and a ROM the catalogue lists as user-supplied takes no
`[license]`. Without `[license]` every layer
is `user-supplied`, as an import without `--open` leaves it. The push guard
is unchanged and decides alone what may leave the machine.

A recipe's licence statement is a claim by its author. It opens nothing that
the guard's own rules (catalogue, terms text, source archive, the most
restrictive local mark per digest) do not also allow.

The recipe file, its bundled texts and its lock are our own text. The recipe
artifact states `CC0-1.0` for them, as the generated config and metadata of
an open set do today.

### 6.7. `[needs]` (Mediafile)

What the title needs from the platform family, as features (R16). Every key
is a **kind**; every kind stated is required; the value lists alternatives,
any one of which satisfies the kind. Kinds and values come from the platform
provider's vocabulary, never from the engine.

```toml
[needs]
cpu = "286+"                                  # this CPU class or a later one
memory = "640K"                               # at least this much conventional memory
video = ["vga"]
sound = ["sb", "adlib", "pc-speaker", "none"] # best first; runs silent too
```

- A value is one string or an array of strings. An array is in the author's
  order of preference, best first; the library uses the order when it
  suggests a template.
- The provider declares each kind as one of three types. **Ordered** (`cpu`):
  a value names a point on the provider's scale, and a trailing `+` means
  "or later"; without `+` it means exactly that one, for titles that break
  on a faster machine. **Quantity** (`memory`, `ems`, `xms`): a number with
  `K` or `M`, read as a minimum. **Choice** (`video`, `sound`, `input`):
  names from the provider's list.
- `none` is a value of every choice kind. It means the title also works
  without any feature of that kind. `sound = ["sb", "none"]` reads "uses a
  Sound Blaster, runs without sound".
- A kind or value the provider does not know is refused by the checker
  (§6.1). The vocabulary is append-only (§9).
- The provider judges a machine against the needs, and it knows what the
  statement leaves out: whether a VGA card serves a title written for EGA
  is the provider's answer, not the recipe's.
- Needs never block. The library warns on a mismatch and suggests a
  template; the user may insert the set into any machine whose drives take
  the media.

The first vocabulary for `ibm-pc`, accepted by the maintainer on
2026-11-17. It is append-only (§9).

| Kind | Type | Values |
| --- | --- | --- |
| `cpu` | ordered | `8088`, `286`, `386`, `486`, `pentium`; stated as a minimum (`286+`) |
| `fpu` | choice | `none`, `287`, `387` |
| `video` | choice | `mda`, `hercules`, `cga`, `ega`, `mcga`, `vga`, `svga` |
| `sound` | choice | `pc-speaker`, `tandy`, `adlib`, `sb`, `sbpro`, `sb16`, `gus`, `mt32`, `none` |
| `memory` | quantity | conventional memory, in KiB (`640K`) |
| `ems`, `xms` | quantity | expanded and extended memory |
| `input` | choice | `mouse`, `joystick` |

The provider owns the vocabulary and holds it in code. The accepted list
names `none` for `fpu` and `sound`; this document reads the rule above,
`none` in every choice kind, as still standing, so `input = ["mouse",
"none"]` stays valid. That reading is the author's and is noted in §16.

Where needs are stored: the lock's metadata carries them (metadata schema 2,
which older releases show read-only, decision-30 point 5), and the manifest
carries a compact copy in the annotation `unemu.needs` so Remotes can warn
from a manifest alone. Both are in the lock, so neither binds a later
writer.

### 6.8. `[ids]` and `[input.<id>]`

`[ids]` holds ids of the **work**, the thing regional variants and ports
share: `wikidata = "Q…"`, and others the provider or the library knows
(`mobygames`, `igdb`). A recipe stores an id or an entry name its author
typed and a hash its author measured. It never copies a catalogue's data:
Wikidata's is CC0, the dump catalogues and commercial databases have terms
of their own (ASSUMED; to be checked before any tool imports from them). The library groups artifacts with one work id:
Contra (USA), Contra (Japan) and Probotector (Europe) are three artifacts of
one work. Ids are strings; the engine does not interpret them.

An input is a single file or a file set.

| Key | Required | Content |
| --- | --- | --- |
| `name` | yes | The canonical file name (a file) or folder name (a file set). A hint, never an identity. |
| `size`, `sha256` | a file: yes | Exact byte size and lower-case SHA-256. |
| `files` | a file set: yes | An array of `{ path, size, sha256 }`, optionally with `time` in the form of an `unpack` member's (§6.9). Paths are relative, use `/`, and are unique. |
| `names` | no | Other names the file is known under. |
| `crc32`, `md5`, `sha1` | no | Weaker hashes, as catalogues carry them. Hints for choosing candidates. |
| `ids` | no | An array of `{ db, set, name }`, all three required: the dump's entry in No-Intro, Redump, TOSEC or MAME [SRC 9, 10]. `db` is `no-intro`, `redump`, `tosec` or `mame`; for MAME, `set` is a machine's ROM set or a software list. |
| `urls` | no | `https` URLs that serve exactly these bytes, tried in order. |
| `bundle` | no | `true`: the bytes travel with the recipe (§7.1). Only for our own text, at most 1 MiB. |
| `disclosable` | no | `true`: this file is under 256 bytes and its hash may be published all the same (§6.13). Also allowed on a file-set entry. |

An input has exactly one of `size` with `sha256`, or `files`. There is no
`optional` key (§3.2) and no path.

### 6.9. `[[step]]`

Every step has `id`, the name of the value it makes, and exactly one of the
verb keys `unpack`, `fat`, `text`. The verb's value is the step's operand: a
reference to an input or to an earlier step.

**`unpack`**: listed members of an archive, as a tree.

| Key | Content |
| --- | --- |
| `unpack` | A file: the archive. |
| `format` | `"zip"`. The only format in v1. |
| `members` | An array of `{ path, size, sha256, time }`; only `path` is required. Only these members are taken; the tree has them in this order, each with its stated `time` or none. |
| `complete` | `true`: when the archive is opened, its file members must be exactly the listed ones, in this order. Default `false`. |

Zip rules: methods stored and deflate only; no encryption, no split
archives; a member name must be relative, without `..`, without a drive or a
leading `/`, and ASCII or flagged UTF-8; a duplicate name is refused;
directory entries are skipped and not counted by `complete`; CRC-32 and the
declared size are verified, and `size` and `sha256` when given.

Nothing but bytes is taken from the archive (§3.4). A member's time is the
`time` the recipe states, a local date-time with even seconds and a year
from 1980 to 2107, which is what both a zip entry and a FAT directory
entry can hold. `upcli recipe scaffold` writes the list, with sizes, hashes and the archive's own times,
when a recipe is first written. When the archive is opened and a stated
size, hash or time differs from the archive's, the build stops: a recipe
may not claim an archive's times and state others. A member with `size` and
`sha256` can be found without the archive; one without them needs it.

**`fat`**: a FAT volume, or several, from a tree.

| Key | Content |
| --- | --- |
| `fat` | A tree. |
| `format` | A media format name of the platform provider: `"1.44M"`. It selects the geometry and the provider's volume profile (§10). |
| `label` | The volume label, at most 11 characters of the DOS set. Optional. |
| `serial` | The volume serial, a 32-bit integer. Default: the builder's fixed serial. |
| `order` | `"name"` (default): entries by the bytes of their names, files of a folder before its folders. `"listed"`: the tree's own order, which is the order its members or files are written in the recipe. |
| `time` | A local date-time for entries that have none, for folders and for the label. Default `1980-01-01T00:00:00`. |
| `volumes` | A whole number N from 2: split the tree over exactly N volumes, in folder order. |

Without `volumes` the result is a file, the image, and a tree that does not
fit is an error. With `volumes` the result is a tree with the members `1`
to `N`; a tree that needs another number of volumes is an error that states
the number. Names must already be valid for the file system (8.3 names, the
folder depth DOS allows): the step refuses a bad name and never renames. No
long-name entries are written. The bytes are those of the deterministic
builder in `crates/media/src/fat12` [REPO] with four additions it lacks
today: listed order, entry times, a chosen serial and a label.

**The boot sector** (accepted by the maintainer on 2026-11-17). Every disk
the builder makes behaves as a DOS-formatted data disk does when a machine
starts from it: the boot sector's code prints a message that this is not a
system disk and asks to replace it and press a key, waits for a key, and
tries the boot again with INT 19h. The code and the message are our own,
written for this project; no byte of any vendor's boot sector is used. The
platform provider's volume profile supplies it (§10), and it is part of
the bytes recipe version 1.0 freezes. It replaces today's halt loop, so the
goldens of `media create` change once.

**`text`**: our own text with DOS line ends.

| Key | Content |
| --- | --- |
| `text` | A file. |
| `eol` | `"crlf"`: every LF becomes CR LF. A CR in the operand is refused. |

The operand must be ASCII in v1.

### 6.10. Outputs

**`[[medium]]`** (Mediafile), in installer order:

| Key | Required | Content |
| --- | --- | --- |
| `id` | yes | The medium's id in the config. |
| `label` | yes | The label a person sees: `Disk 1 of 7 - boot disk and installer`. |
| `format` | yes | A media format name of the provider: `1.44M`, `1.2M`, `720K`, `360K`, `iso-2048`. The provider turns it into the config's format table and checks the exact size. |
| `file_name` | no | The export file name (§3.10). Default: the id with the provider's extension. |
| `from` | yes | A file. |

**`[[rom]]`** (Firmwarefile):

| Key | Required | Content |
| --- | --- | --- |
| `id`, `role` | yes | As in the package config: `system-bios`, `video-bios`, `option-rom`, … |
| `catalogue_id` | no | The firmware catalogue entry. |
| `from` | one of | A file: a ROM stored as one blob. Its size and hash are the value's. |
| `parts` | one of | An array of `{ position, part_number, lane, offset, from }`: one blob per chip. `lane` and `from` are required; `position` and `part_number` are optional, and `offset` defaults to 0, as in the package config. |
| `size`, `sha256` | with `parts` | The logical ROM, the chips assembled. Refused with `from`. |

**`[[extra]]`**: `id`, `file_name`, `kind` (`document`, `archive`,
`executable`, `other`), optional `dos_name`, and `from`, a file.

**`[[attachment]]`**: `id`, `title`, `kind` (`documentation` in v1),
optional `digest`, and its own `[[attachment.extra]]` entries with the keys
of `[[extra]]`.

### 6.11. Static checks

`upcli recipe check` refuses, without reading any input:

1. a file that is not the TOML of §6.1, or has an unknown key, type or value;
2. no identity table, or two; a table that belongs to another kind;
3. a duplicate value name or output id; a reference to a name that is not
   an input or an earlier step; a member path under a value that is a file,
   or one the tree does not list (every tree of 0.1 lists its members: a
   file set, an `unpack` step, the volumes `1` to `N` of a `fat` step); a
   file where a tree is wanted, or a tree where a file is wanted;
4. an input or a step that no output uses;
5. nothing here, but it prints the split: every input a `[[medium]]`,
   `[[rom]]` or top-level `[[extra]]` depends on is required, every other
   input is optional;
6. `[license]` that names an extra which does not exist, or breaks the
   rules of §6.6;
7. a format, role, kind, need or platform the provider does not know;
8. `digest` without a lock, or a lock that disagrees with the recipe (§3.8,
   points 1 to 3);
9. a bundled input over 1 MiB, or more than 4096 inputs, 64 steps, 256
   outputs or 100,000 file-set entries.

### 6.12. Determinism

- A step's bytes depend only on its operands' bytes, its keys and the recipe
  version. Never on the host, the clock, the locale, a file name or time on
  the host, or the order a directory lists its entries.
- Times exist only where the recipe or an archive states them.
- A build has no network access except to fetch an input whose hash is
  pinned; what the network returns cannot change the result, only whether
  there is one.
- A build either publishes the whole artifact under its tag, last, or
  nothing (machine-images §3.1, tag-last publication [REPO]).
- The same recipe version means the same bytes in every release that
  supports it. Each release's tests build the conformance recipes of every
  version it supports and compare digests (§9).

### 6.13. Security

A recipe is untrusted input, like an image. The format gives it little to
work with:

- **No execution.** No step starts a process. There is no escape hatch, and
  §13 does not plan one.
- **No host paths.** A recipe cannot name a file on the host. It can only
  state a hash, and the user's own libraries decide where files are.
- **No computed strings.** A URL in a recipe is a literal. A recipe cannot
  put a file name, a hash it found or anything else about the host into a
  request, so it has no channel to send anything out.
- **Network only with consent** (R15). A fetch needs the user's consent for
  that recipe or its publisher; `https` only; redirects stay on `https`; the
  download stops at the pinned size and is hashed before any step reads it.
  Without consent the URLs are shown in the plan and the input counts as
  missing.
- **Parsers of hostile data.** The zip reader and, later, other readers see
  bytes that only match a hash someone else chose. They are bounded: listed
  members only, declared sizes enforced while decoding, the limits of §6.11,
  no links, no devices.
- **A hash proves identity, nothing else.** It does not say a file is safe,
  legal to share or what its name claims (machine-images §5 [REPO]). The
  push guard, not the recipe, decides what may be published.
- **Publishing a recipe** goes through a guard of its own: the artifact may
  hold the recipe, bundled texts under the size limit and the lock, and
  nothing else; a bundled file that is not text is refused.
- **A hash of a tiny file gives the file away.** A four-byte serial number
  can be found from its SHA-256 by trying every value. The guard therefore
  refuses to publish a recipe without `[license]` that names a file under
  256 bytes, unless the entry says `disclosable = true`. The rule and the
  key are the earlier design's.

### 6.14. Mapping to today's artifacts and import paths

A recipe build ends in the same call the importers end in. Nothing about
the artifacts changes.

| Recipe | Today |
| --- | --- |
| `[media]`, `[title]`, `[[medium]]`, `[[extra]]`, `[license]` | The plan of `pc_image::media_import`, which `image media import` and the window's import dialog share: ordered sources, labels, formats, extras, the title record and the open statement [REPO]. |
| `[[medium]].format` | `--format N=1.44M`: the provider's `MediaFormats::validate_format` turns the name into the config's format table and checks the size [REPO: `image_store::media_set`]. |
| `[firmware]`, `[package]`, `[[rom]]`, `[[extra]]` | A `Candidate` of `pc_image::firmware_import`: the config with its ROMs and parts, checked against the firmware catalogue by SHA-256 and size [REPO]. |
| An input found in a library | **Locate firmware** and Import ROM folder…, which already match chips by hash and size, never by name (machine-images §5 [REPO]). The library index generalises that scan to every input. |
| `digest` and the lock | The `digest` pins of `media.toml` and `firmware.toml`, and the pinned `upcli` of unemu-media [REPO]. |
| `fat` | `crates/media/src/fat12`, and unemu-media's `fat_image` over mtools [REPO]. |
| `revision` | `[[set]].revision` and `[package].revision` [REPO]. |

The engine belongs in a platform-neutral crate beside `image-store`. It
calls the provider for formats, volume profiles, firmware rules and the
needs vocabulary, and the importers' shared plan-and-publish functions for
the last step.

### 6.15. Commands

The spellings follow `backlog/docs/commands.md` when the work is planned;
the set of commands is the proposal.

| Command | What it does |
| --- | --- |
| `upcli recipe check FILE` | The static checks of §6.11. Reads nothing else. |
| `upcli recipe plan FILE\|REF [--json]` | Resolves every input and prints the report of §3.5. Writes nothing. |
| `upcli recipe build FILE\|REF [--fetch] [--library DIR]…` | Builds and tags the artifact and every attachment whose inputs are found. `--fetch` is the consent of R15. |
| `upcli recipe pin FILE` | The author's build: writes `digest` into the file and the lock beside it. |
| `upcli recipe scaffold FILE\|DIR…` | Writes the `[input]` tables, member lists and file sets for files the author has, with sizes, hashes and an archive's times. The author adds the description. |
| `upcli recipe inputs FILE --format dat\|csv\|sha256sums` | The inputs as a Logiqx DAT, a CSV or a checksum file, for a ROM manager or `sha256sum -c`. |
| `upcli recipe push FILE REF`, `recipe pull REF` | The recipe artifact (§7). |
| `upcli library add DIR`, `library index [--catalogue FILE]`, `library list` | The user's libraries and their index (§3.4). |
| `upcli image export REF --to DIR [--files]` | The export of §3.10. |

### 6.16. Work files

Several releases of one work repeat most of `[title]`: the regional
releases of a cartridge, the shareware and full versions of a game, the
disk formats of an operating system. A **work file** holds what they share.

```toml
recipe = "0.1"

[work]
id = "contra"

[title]
title = "Contra"
series = "Contra"
publishers = ["Konami"]
developers = ["Konami"]
genres = ["run and gun"]
software_platform = "NES"
players = { min = 1, max = 2 }
labels = ["game"]
synopsis = "Konami's run-and-gun game: …"

[ids]
wikidata = "Q…"
```

and a release states only what is its own:

```toml
recipe = "0.1"
work = "contra"

[media]
name = "private/media/nes/contra"
platform = "nes"
release = "europe"
revision = 1

[title]
title = "Probotector"
year = 1990
regions = ["Europe"]
description = "…"
```

**The file.** `<id>.work.toml`, in the recipe's own folder. `work = "contra"`
names it by id; there is no path to write and none to escape through. It
states `recipe` as a recipe does, and its identity table is `[work]` with
`id`. Besides those it may hold `[title]`, `[ids]` and `[needs]`, and
nothing else: no inputs, steps, outputs, licence, digest or `work` key of
its own. One level only: a work file includes nothing.

**The work is the repository.** The last component of a recipe's `name`
must equal its work's id. So all releases of one work on one platform are
the tags of one repository, which is what §8 already assumes, and the work
file is that repository's description.

**Which field lives where.**

| Field | Work file | Recipe | When both state it |
| --- | --- | --- | --- |
| `title`, `sort_title`, `series`, `software_platform`, `players` | yes | yes | The recipe's value (Probotector) |
| `developers`, `publishers`, `genres` | yes | yes | The recipe's list replaces the work's |
| `labels` | yes | yes | The union: the work's, then the recipe's new ones |
| `synopsis` | yes | no | — |
| `version`, `edition`, `year`, `regions`, `languages`, `source_media`, `requirements`, `description` | no | yes | — |
| `[ids]` | yes | yes | Per key, the recipe's value |
| `[needs]` | yes | yes | Per kind, the recipe's value |

Why these rules. A single value has one obvious rule, the nearer one wins.
A list of people or companies is a statement about one release ("published
by X in Europe"); merging two such lists would credit the wrong publisher,
so the recipe's list replaces. Labels are the exception because they are
marks, not credits: `game` belongs to the work and `freeware` to one
release, and nobody wants to repeat `game` in order to add `freeware`.
There is no way to remove a work's label in a release; a label that not
every release carries does not belong in the work file. Needs merge per
kind because releases mostly share them and differ in one (`tv_system`).
Release fields are refused in a work file, and `synopsis` in a recipe, so
that each text has one home.

**Synopsis and description.** The synopsis says what the work is; the
description says what this release is ("the Europe release, as one
cartridge dump"). The title record's `description` is the recipe's; when a
recipe states none, it is the synopsis. Metadata schema 2 also carries the
synopsis and the work id on their own, so the library can show one text
for the work above its releases.

**`upcli recipe show FILE`** prints the merged recipe as one TOML document,
with a comment on each field that came from the work file. `check` works on
the merged result and names the file a bad value came from.

**Work files and the lock.** A work file is not pinned by the recipes that
name it; if it were, a corrected synopsis would mean editing every recipe.
The lock makes that safe. A pinned recipe builds the locked metadata,
whatever the work file says today, so **an edit to a work file never
changes a published image**. `check` lists, per pinned recipe, the fields
where the work file now differs from the lock. The edit takes effect when
the author pins a new revision, release by release, as §8.4 publishes any
metadata change. A draft has no lock and always uses the work file as it
is.

**In a registry.** A recipe artifact carries its work file as a layer, the
copy that was beside it at pin time (§7.1). So a pulled recipe is
self-contained, and two releases of one work may carry different copies:
each agrees with its own lock.

**Not a registry artifact of its own, for now.** The work's data already
reaches every place that needs it: merged into each artifact's metadata,
and bundled in each recipe. A third home would be a second source of truth
with its own tag and revision rules. The natural later home is the
catalogue index of registry-library-plan §2 (`upc-5pw.65`), which has one
entry per repository: the work is the repository, so the catalogue entry
can carry the work's fields, **derived** from the newest revision's
metadata (which names its work id and synopsis), never written by hand.

**Grouping.** Three levels, each with one mechanism:

- releases of one work on one platform: one repository; Remotes and the
  library show the work's title and synopsis once, with the releases (USA,
  Japan, Europe as Probotector, USA Rev 1) as rows under it;
- one work across platforms (Contra on the NES, on MSX2, in DOS): the
  `wikidata` id in `[ids]`, which is why Wikidata comes first there;
- a series: the `series` field.

**Does this replace `include`?** For the description, yes, and with rules a
general include would not have: a fixed place, one level, a closed set of
fields, a merge rule per field. For the execution part nothing is shared by
file, on purpose. MEASURED on FreeDOS's three formats after the work file:
77 of the 107 lines of the 1.44M recipe still occur in all three (86 of
116 before): the three `[input]` tables, `[license]`, one `text` step, two
`[[extra]]` tables and the table headers and keys every medium has. Those are
the build of one artifact; sharing them would let an edit in one place
change what several pinned recipes build, which the description cannot do
because the lock holds it. So no general include is planned (§13).

## 7. Recipes as registry artifacts

### 7.1. The artifact

| Part | Content |
| --- | --- |
| Manifest `artifactType` | `application/vnd.unemu.recipe.media.v1` or `application/vnd.unemu.recipe.firmware.v1`: one per kind, because a registry can filter a listing by artifact type and by nothing else. |
| Config | The recipe file itself, media type `application/vnd.unemu.recipe.v1+toml`. The config digest is the recipe's digest. |
| Layers | The work file when the recipe names one, each bundled input, and the three lock files, each with `org.opencontainers.image.title` as its file name. The work file is the one that was beside the recipe when it was pinned, so the artifact is self-contained. |
| Annotations | `unemu.platform`; `unemu.recipe.output` (`name:tag`); `unemu.recipe.output.digest`; the title and description for registry web pages; `unemu.min-app-version`, here the oldest release that can *build* the recipe; `org.opencontainers.image.licenses = "CC0-1.0"`. |

A bundled input is an ordinary input: it has a hash and a size in the
recipe, and the recipe's own folder or artifact is simply the source that
has it (§3.4, source 3). Nothing in the model is special about it.

A client decides "can I build this" by fetching the manifest and the config,
a few kilobytes, and running the plan against its libraries. It fetches no
payload, because there is none (R13).

### 7.2. The lock

The lock is three files named by their SHA-256, as the store names blobs:
the output's manifest, its config and its metadata. Beside a recipe file
they live in `<recipe file>.lock/sha256/<hex>`; in the artifact they are
layers. The recipe's `digest` names the manifest, and the manifest's
descriptors name the other two, so the recipe file alone pins all three.

A pinned recipe file that has lost its lock is not useless. `recipe build`
then writes the config, metadata and manifest with its own writers. If the
result has the pinned digest, the build is as good as any; if not, it says
that the recipe needs its lock and tags nothing.

### 7.3. Names

A recipe's repository is its publisher's namespace, `recipes/`, and the
output's path after its first component. Its tag is the output's tag.

| Output | Recipe |
| --- | --- |
| `unemu/media/ibm-pc/freedos:1.4-720k-r1` | `unemu/recipes/media/ibm-pc/freedos:1.4-720k-r1` |
| `unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1` | `unemu/recipes/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1` |
| `private/media/ibm-pc/windows:3.0-1.2m-r1` | `unemu/recipes/media/ibm-pc/windows:3.0-1.2m-r1` |

One namespace holds every recipe, so a registry can list them and give them
their own access rule: recipes for private media are public, their outputs
are not. A recipe without `[license]` names its output under `private/`,
the namespace decision-25 keeps for content that never leaves the user's
own registries; the checker refuses such a recipe a name under `unemu/`,
where the push guard would refuse its output anyway. A user may give a
build another local name; the name is not part of any digest.

A recipe tag is **not** identity-bearing. The output's digest is. Two
recipes that pin the same output digest are interchangeable, and a better
one (a new mirror URL, another database id, a clearer name) should replace
the old without a new revision of the artifact. So a recipe tag may be
replaced, by a recipe that pins the same output digest and by nothing else.
The push guard enforces that; a record of which recipe text built a local
artifact is its digest, kept on the local tag (`unemu.build.recipe`, an
index-descriptor annotation like `unemu.pull.registry`).

### 7.4. The Recipes group in Remotes

The approved Remotes mock-up gets a Recipes group. A row is a recipe; its
state comes from the plan:

| State | Shown as |
| --- | --- |
| Every required input found | `can build: all 12 inputs found` |
| Some required input missing | `missing 3 of 12`, and the report of §3.5 on selection |
| Output already in the store with the pinned digest | `built` |
| Attachments | `2 of 3 optional parts found` beside either state |
| Recipe version or a step newer than this release | `Needs unemupc X or newer` (decision-30) |
| URLs present, no consent yet | `can build after download (294 MB)` |

Mock-ups come first, as for every GUI change.

## 8. Tags and versions

### 8.1. The problem

A tag should carry the software's version (`freedos:1.4`). A tag, once
public, must not move. And yet our packaging of the same software changes:
a corrected description, a new label, an added licence text. Some things
have no version at all: an NES cartridge has a region and sometimes a
revision.

### 8.2. The scheme

```text
tag      = [ release "-" ] "r" N          the immutable name of one packaging
alias    = release                         follows the newest rN of that release
release  = part ( "-" part )*
part     = [a-z0-9]+ ( [._] [a-z0-9]+ )*   and never "r" followed only by digits
```

- **The release names the software**, not our work on it. Its parts are, in
  this order and each only when it applies: the version as the software
  shows it; the edition; the region; the language; the format when it is not
  the release's own. `1.4`, `1.4-720k`, `2.01`, `3.0-1.2m`, `usa`,
  `usa-rev1`, `europe`. Software with none of these takes the year of its
  release (`2026`), and a thing nobody has identified has no release at
  all: its tags are `r1`, `r2`.
- **`rN` counts our packagings of one release**, from 1. It is the Debian
  revision, the RPM release, the Arch `pkgrel` and the `-rN` of Alpine and
  Gentoo [SRC 6]: the one standard answer to "the upstream did not change,
  our package did".
- **A tag that ends in `rN` never moves**, in the local store and on a
  registry. Every other tag is an alias and moves.
- **The release alone is an alias** for its newest revision. `freedos:1.4`
  is what a person types; `freedos:1.4-r2` is what a lock, a workspace or a
  Machinefile pins, beside the digest.
- **No `latest`** for media sets and firmware packages: "the newest FreeDOS"
  across releases and formats has no single answer.
- **The parts of a release are a convention, not a parser.** No program
  reads meaning out of a tag. Version, edition, regions and formats are
  fields of the title record and the config; the tag is only a name built
  from them by the author. The checker knows the grammar above and nothing
  about what `720k` means.

### 8.3. Per kind

| Kind | Repository | Tags |
| --- | --- | --- |
| Media set | `media/<platform>/<work>` under `unemu/` or `private/` | `<release>-rN`, alias `<release>` |
| Firmware package | `firmware/<platform>/<catalogue id>` under `unemu/` or `private/` | `rN`; a release only when one catalogue entry has several packagings that differ in content (§8.5) |
| Machine template | `unemu/<name>`, unchanged | Semantic versions and `latest`, unchanged (machine-images §5.5): a template is our own product and its changes have the meaning SemVer gives |
| Machine image built by a Machinefile (later) | to be decided with the Machinefile | `<release>-rN` |
| Recipe | the output's repository under `recipes/` | the output's tag (§7.3) |

`<platform>` is new in the repository path. One namespace for every
platform's media will collide as soon as a second platform exists: Contra
is an NES cartridge, an arcade board, an MSX2 cartridge and a DOS game. The
six published artifacts are deleted and pushed again for `ibm-pc` anyway
(decision-32, point 5), so the path can change in the same step at no extra
cost.

### 8.4. How a metadata change is published

As a new revision. `freedos:1.4-r2` has a new manifest and a new metadata
blob; its config and every medium are the blobs `1.4-r1` has. The alias
`1.4` moves. A client that has `r1` fetches two small blobs. It can tell
what kind of change it is by comparing the config digests: equal means "the
description changed", different means "the content changed". A workspace or
a machine pinned to `r1` is untouched.

OCI referrers are **not** used for corrections. The record must travel
inside the artifact, so that an archive, an offline store and a registry
without the Referrers API all show the same thing (decision-25, point 7
[REPO]); a correction that lives beside the artifact would make two sources
of truth. Referrers are used for **additions** that leave the record alone:
optional parts (§3.9), and the covers, manuals and scans of
registry-library-plan §3.

### 8.5. Cases the corpus raised

- **Regions without versions.** `private/media/nes/contra:usa-r1`,
  `:japan-r1`, `:europe-r1` and `:usa-rev1-r1`. The European one is titled
  Probotector; the title is a field of the release, the repository is the
  work, and `contra.work.toml` holds what the four share (§6.16).
- **A cartridge revision** is a part of the release (`usa-rev1`), spelled
  `rev`, so it cannot be mistaken for our `rN`.
- **One ROM, two chip layouts.** The IBM 5170 BIOS of 1984-01-10 is one
  64 KiB ROM that exists as two 32 KiB chips or four 16 KiB chips
  [REPO: `crates/assembly/src/firmware.rs`]. A package stores the chips, so
  the two are different artifacts of one catalogue entry: `2x32k-r1` and
  `4x16k-r1`. Today both would be `:1` with whatever digest the user's chips
  give.
- **Mixed media.** The FreeDOS LiveCD comes with a boot floppy. The release
  names the main format (`1.4-livecd`); each medium's own format is in the
  config.

## 9. What binds forever

After the freeze, each of these is a promise. The list is meant to be
short, and the lock is what keeps it short.

| # | What | Why it binds | How it is versioned |
| --- | --- | --- | --- |
| 1 | An input is its SHA-256 and its size | Every published recipe names inputs this way | Never changes. A second hash would be a new key beside `sha256`, in a new recipe version |
| 2 | The TOML 1.0 subset and the schema of each recipe version | A published recipe must parse to the same data | `recipe = "MAJOR.MINOR"`. A release reads every version from 1.0 to its own. Nothing is ever added to an existing version |
| 3 | The bytes each step writes, per recipe version | The lock pins those bytes | A fix that changes bytes is a new minor version; the old behaviour stays in the code and in the tests. A recipe opts in by raising its `recipe` |
| 4 | The payload encoding the lock names (`+zstd`: the encoder's exact output) | The lock pins stored digests | A named encoder profile (Q3) |
| 5 | Reading old configs, metadata and manifests | Needed to use any published artifact | Already bound by decision-30. **Writing** them is not bound: the lock carries the bytes |
| 6 | The tag grammar, the `rN` rule and the `recipes/` name mapping | Refs are in locks, Machinefiles and people's heads | Never changes; new kinds add repositories |
| 7 | Provider vocabularies: platform ids, media format names and what `1.44M` means, volume profiles, firmware roles, needs kinds and values | Recipes name them | Append-only. A name never changes meaning (decision-30, point 4) |
| 8 | The recipe artifact's media types and annotation keys | Registries hold them | A new major is a new media type |
| 9 | The export layout (§3.10) | Scripts outside the project depend on it | A version file in the export |

What does **not** bind, because of the lock: how a release spells TOML or
JSON, which annotations it writes and in what order, how it computes
`unemu.min-app-version`, how it derives a default file name. The author's
release decided those once, and the lock recorded the result.

What does not bind either: the plan report and its JSON (versioned, may
change), the library index (a cache), resolution order and consent rules
(they change what is found, never what is built).

**Versions.** `0.x` promises nothing: a release may drop a `0.x` version, and
no `0.x` recipe is published. From `1.0`: a minor adds keys, steps, formats
or a corrected step; a major changes the grammar or the meaning of
something existing and gets a new media type. Each row of the decision-30
table that a recipe uses (its version, each step, each format) sets the
recipe artifact's `unemu.min-app-version`.

**The conformance corpus.** unemu-media's recipes with their locks are the
test: every release builds each one from a fixture library and compares the
manifest digest. A release that cannot reproduce one is not released. This
is the only way promise 3 is kept in practice.

## 10. Platform neutrality

The engine knows files, trees, hashes, TOML, OCI and the three steps. The
steps are keyed by what they read or write, not by platform: `fat` builds
FAT volumes for the IBM PC, the Atari ST, the PC-98 and MSX-DOS alike.

The provider of the recipe's `platform` supplies, through traits beside
today's `MediaFormats` and firmware rules [REPO]:

| Seam | IBM PC example | Other platforms |
| --- | --- | --- |
| Media formats: name to config format, exact size, export extension | `1.44M`, `iso-2048` | Amiga `880K` (ADF); NES `ines`; C64 `d64` |
| Volume profile for a file-system step: geometry, BPB values, boot sector convention, name rules | The four PC/AT diskette formats and DOS 8.3 names | Atari ST: other BPB conventions and serial placement in the same FAT12 |
| Firmware roles and catalogue | `system-bios`, `video-bios`, `option-rom` | Amiga Kickstart; a console's boot ROM |
| Needs vocabulary | `cpu`, `video`, `sound` | Amiga `chipset`, `kickstart`, `memory`; NES `tv_system` |
| Input forms (later, §13) | none | NES: the cartridge without its 16-byte iNES header |

The corpus has two recipes for platforms that do not exist yet (an Amiga
disk and three NES cartridges). Both were written without a change to the
engine's part of the schema; every platform word in them is marked ASSUMED.

## 11. A Machinefile sketch

Later work, shown only to check that the family holds together. A
Machinefile starts from an image, uses media sets and firmware packages by
digest, and drives the guest with a closed set of input steps, then commits
disk layers. It is the "recipe layer" of deferred § Layered disk images
[REPO], and Packer's `boot_command` is the nearest prior art [SRC 2].

```toml
recipe = "0.1"

[machine]
name = "private/machines/ibm-pc/windows"
platform = "ibm-pc"
release = "3.0"
revision = 1

[from]
image = "unemu/open-at:1.0.0"
digest = "sha256:…"

[use.windows]
image = "private/media/ibm-pc/windows:3.0-1.2m-r1"
digest = "sha256:…"

[[step]]
insert = "windows/disk-1"
drive = "a"

[[step]]
start = true

[[step]]
wait_text = "Welcome to Setup"
budget = 400_000_000              # retired instructions, never seconds

[[step]]
key = "enter"

[[step]]
type = 'C:\WINDOWS'

[[step]]
commit = "system"
label = "Windows 3.0 installed"
```

Differences from the other two kinds, all deliberate:

- Its steps have no `id` and no operands in the dataflow sense. The machine
  is the state they act on, which is honest: an installer cannot be made
  into a pure function.
- Its inputs are artifacts by digest, not files by hash. A user builds the
  media set with its Mediafile first; the Machinefile's missing-input report
  then names sets, not files.
- Its digest is reproducible only by the emulator release and CPU core
  revision that built it (deferred: results cached by parent hash, recipe
  hash and CPU core revision). So a Machinefile pins an engine, and a later
  release either carries that engine's behaviour or says that it builds the
  image **unverified**. That is the largest open problem of the family and
  the reason the Machinefile is not in v1.

A long step list is exactly where TOML's `[[step]]` headers cost most. The
Machinefile corpus decides whether the cost is acceptable (§4.5).

## 12. Migration from unemu-media

| Today [REPO] | Recipe |
| --- | --- |
| `media.toml` `[release]`, `[[set]]` | `[media]`, `[title]`, `[license]`; one file per set |
| `[[set.disk]] path, sha256, label` | `[[medium]]` with `from` a member of an `unpack` step |
| `[[release.extra]]` | `[[extra]]` |
| `firmware.toml` | `[firmware]`, `[package]`, `[[rom]]`, `[[extra]]` |
| `fetch DEST SHA256 MAX_BYTES URL…` | `[input.x]` with `size`, `sha256`, `urls` |
| `fetch_release_assets` | Two inputs with URLs; outputs refer to them directly |
| `zip_expect_members`, `zip_extract` | `unpack` with `complete = true` and `members` |
| `fat_image` (mtools 4.0.49) | `fat`; the bytes differ from mtools', so Ken's Labyrinth is pinned anew. It was never published |
| `crlf` | `text` with `eol = "crlf"` |
| `outputs_match`, `check_outputs`, `install_outputs` | The lock and §3.8 |
| `digest` pins and the pinned `upcli` | `digest` and the lock. The pinned `upcli` stays for `recipe pin` only |
| `git_export`, `source_tar`, `iso-extract.py` | **No equivalent in v1** (below) |
| `lib/publish.py` | `recipe build`, `image push`, `recipe push` |

**The one output v1 cannot build** is the FreeDOS source archive: 69 package
zips copied out of an ISO inside a zip, four git trees at pinned commits,
and a deterministic tar over all of it. Building it in a recipe needs an
ISO 9660 reader, a tar writer and git as an input source. v1 has none of
them, and git does not fit the model at all: a commit is not a file with a
SHA-256.

The recommended answer is to treat that archive as what it is to every user
except its maker: a **prepared input**. unemu-media builds it once with the
script it has, publishes it as a release asset, and the recipe names it by
hash and size like any other input. Its corresponding-source value is in
its content, which the hash pins; how it was assembled stays documented in
unemu-media. The cost is honest: one script survives the migration, and the
recipe does not show how that archive was made. The firmware packages
already work this way: their ROMs and source archives are release assets of
the forks, and no recipe builds them [REPO: unemu-media `docs/recipes.md`].

Order of work: the checker and the corpus first; then `recipe build` for
the three firmware packages (no steps); then FreeDOS (`unpack`, `text`);
then Ken's Labyrinth (`fat`); then one private recipe end to end; then the
freeze.

Every published digest changes once, in the step that also renames the
platform to `ibm-pc` and moves the repositories (decision-32, point 5).

## 13. Later features

Not in v1. Each needs a new recipe minor version, and none may run a
program.

| Feature | What for | First needed by |
| --- | --- | --- |
| `tree` step: compose a tree from files and trees, with per-entry paths and times | A disk made of an archive's files plus our own README; transfer disks | Shareware sets with added texts |
| More `unpack` formats: ISO 9660, tar, gzip, a FAT image, LHA, ARC, 7z | Files inside CD images and old archives | The FreeDOS source archive; DOS shareware |
| `tar` step (deterministic) | Source archives built in the recipe | FreeDOS |
| `fat` for FAT16 and hard-disk images; `iso` step | Large titles, CD builds | Phase C |
| `slice`, `concat`, `pad`, `interleave` | Header removal, chip assembly | NES input forms; one artifact for both layouts of a ROM |
| `patch` (IPS, BPS, xdelta) | Official patches and translations | Later |
| Input forms from the provider: an input found through a transform (a headerless cartridge, a sector image inside an IMD, TD0 or IPF file) | Finding a dump whatever container the user keeps it in | NES; preserved floppies |
| Library containers beyond zip: 7z, FAT and ISO images in the store | Finding a file inside a stored disk | Rebuilding after the originals are gone |
| Save and load of whole artifacts, selections and repositories as OCI Image Layouts (`upc-5pw.116`, its own bead) | Moving a store's content between machines without a registry | Related to export, and distinct from it: export (§3.10, decision-37) unpacks one artifact into plain files; save and load keep artifacts as artifacts |
| Character sets for `text` and member names (CP437, Shift JIS) | Non-ASCII DOS texts and archives | PC-98 |
| `os` needs (`dos = "3.3+"`) | Suggesting a template with the right system | The library's suggestions |
| An `oci` source for an input: a layer of a published artifact, by digest | Inputs we already publish, such as the FreeDOS source archive | FreeDOS (the earlier design's answer to Q6) |
| The Machinefile | Installed systems | Phase C |

Not planned at all: a general `include`. The work file (§6.16) shares the
description; the execution part is not shared by file.

## 14. Process and freeze

As the maintainer accepted: nothing freezes before it meets reality.

1. **Corpus** (done here, as drafts): `recipe-corpus/`, nine recipes in two
   syntaxes.
2. **Throwaway checker** (done for TOML, §4.6): a small parser and the
   static checks of §6.11 for both syntaxes, run over the corpus. It
   settles the ASSUMED points of §6.1 and §3.10 and gives the first real
   error messages.
3. **Decisions** 33 to 38, each accepted or changed on its own.
4. **`0.x`**: the engine behind an experimental flag; unemu-media migrates;
   one private recipe (Windows 3.0 or AZURAMON) is built end to end from a
   library by someone who did not write it.
5. **A Machinefile corpus**, before the freeze, because it tests the syntax
   where it is weakest.
6. **The FAT builder's last free choices**, before the freeze, because the
   freeze makes them permanent. The boot sector is decided (§6.9); the
   builder's remaining defaults are reviewed once more at the freeze.
7. **Freeze 1.0**, publish recipes, and from then on §9 holds.

## 15. Risks

| Risk | Effect | Answer |
| --- | --- | --- |
| A step's bytes change by accident (a refactoring of the FAT builder) | Every pinned recipe that uses it fails, for every user | The conformance corpus in every release's tests (§9); builders are versioned code, not shared helpers |
| The Zstandard encoder's output changes with a library update | Every lock with a `+zstd` payload fails | Q3: a frozen encoder profile with test vectors, so the change is caught before a release |
| The lock makes authors lazy about the description | The recipe's fields and the artifact drift apart | §3.8 point 3 compares them field by field on every build |
| TOML proves unreadable for Machinefiles | A second syntax, or a painful one | The schema is syntax-neutral; HCL is the tested fallback (§4.5); the Machinefile corpus comes before the freeze |
| The `toml` crate accepts TOML 1.1 | Recipes that other parsers refuse | The checker refuses the four 1.1 forms (§6.1) |
| Attachments need `subject` and a referrers table that do not exist yet | Optional parts are late | They are specified in v1 and may ship after the first builds; the main artifact never depends on them |
| A recipe becomes a list of 561 hashes | A 70 KiB file nobody reads | Accepted: it is data, and `recipe inputs` and the plan are how people look at it. The limit is 100,000 entries |
| People expect "delete my originals" to be always safe | Loss of an archive's unused members or of host file times | The build prints what is kept per input (§3.10); the decision record states the limits |
| A recipe's URL fetch reveals the user's address to a third party | Privacy | Consent per recipe or publisher (R15); without it nothing is fetched |
| Renaming repositories and tags before the public release | Every pinned digest moves once more | It moves anyway for `ibm-pc`; do both in one step |
| Catalogue names and ids differ between database versions | A hint fails | Hints only narrow candidates; the hash decides, so a wrong hint costs time, never correctness |

## 16. Open questions for the maintainer

**Q1. The syntax. Answered** by the maintainer on 2026-11-17: TOML 1.0, as
specified here, kept at `0.x` until the Machinefile corpus exists; the HCL
literal subset stays as the documented fallback (§4.5, decision-33).

**Q2. Repository names. Answered** by the maintainer on 2026-11-17: (a), now, with the `ibm-pc` push. The options were: (a) `unemu/media/<platform>/<work>`,
`unemu/firmware/<platform>/<id>` and recipes under `unemu/recipes/…`.
(b) Keep `unemu/media/<work>` and add the platform only on a collision.
*Recommendation: (a)*, now, in the same step as the `ibm-pc` push. Later it
means renaming public repositories.

**Q3. Compressed payloads in a lock. Answered** by the maintainer on 2026-11-17: (a), one frozen encoder profile named in the lock, with test vectors. The question was: a `+zstd` layer's stored digest
depends on the encoder. (a) Freeze one encoder profile (the library version
and parameters in use today), name it in the lock, pin its output with test
vectors, and never change it; a new encoder is a new profile beside the old
one. (b) Store the payloads of recipe-built artifacts raw. (c) Verify by
decoded digests and let the manifest digest differ between users.
*Recommendation: (a)*. It makes explicit a dependency that
`image media import` already has today. (b) costs registry and store space
for good and gives recipe-built artifacts a different shape from imported
ones. (c) gives one tag several digests. The cost of (a) is carrying one
exact encoder for good; if that ever becomes impossible, (b) is still open
for new revisions.

**Q4. Tags. Answered** by the maintainer on 2026-11-17: (a). The options were: (a) `<release>-rN` immutable, `<release>` an alias, the
digit-first rule retired, firmware tags `rN`. (b) Today's scheme: the first
packaging is `1.4`, later ones `1.4-r2`. *Recommendation: (a)*. Under (b)
the name a person types, `freedos:1.4`, is for ever the first and worst
packaging. The cost of (a): releases 0.0.1 and 0.0.2 treat `1.4` as a set
tag that never moves, so they refuse to follow the alias after an `r2`;
that is the same class of preview-release limit decision-32 already accepts.

**Q5. Optional parts. Answered** by the maintainer on 2026-11-17: (a), in the specification now, built after the first recipes work. The options were: (a) Attachments by OCI `subject`, which needs the
manifest codec and a local referrers table (registry-library-plan, phase
3). (b) No optional parts in v1; the Windows 3.0 manuals wait.
*Recommendation: (a) in the specification, built after the first recipes
work.* The main artifact is the same either way, so nothing is lost by
ordering the work this way.

**Q6. The FreeDOS source archive. Answered** by the maintainer on 2026-11-17: a prepared input now, an `oci` source later. The options were: (a) A prepared input, built once by the
script unemu-media has, published as a release asset. (b) The same, taken
from the published set through a new `oci` source, as the earlier design
proposes. (c) Add an ISO reader, a tar writer and a git source to v1.
*Recommendation: (a) now, (b) when the `oci` source exists.* (c) triples
the step set for one output, and git is not content-addressed by SHA-256.

**Q7. The work in the metadata. Answered** by the maintainer on 2026-11-17: (a), metadata schema 2 records the work id and the synopsis. The question was: a work file's fields are merged into the
title record, so an artifact alone does not say which fields are the
work's. (a) Metadata schema 2 also records the work id and the synopsis as
fields of their own, so the library and the catalogue can show a work above
its releases. (b) Nothing extra; grouping uses the repository name alone.
*Recommendation: (a)*. It costs two fields in a schema that is being
extended anyway (needs, requirements), and without it the catalogue cannot
derive a work entry.

**Also decided** by the maintainer on 2026-11-17, after the questions:

- **decision-37, the export guarantees, as written.** The file names in
  `org.opencontainers.image.title` are therefore part of the work that
  lands before the six artifacts are pushed again (§19, A3).
- **The Zstandard profile.** The `+zstd` suffix of the version-1 media types
  is itself profile 1. No manifest byte changes; a later profile gets a new
  marker.
- **The boot sector of built disks** (§6.9): our own code that reports a
  non-system disk, waits for a key and retries through INT 19h.
- **The first `ibm-pc` needs vocabulary** (§6.7), append-only.
- **Confirmed explicitly:** the lock; a recipe tag replaceable only by a
  recipe that pins the same output digest; one artifact type per recipe
  kind; the `disclosable` guard for files under 256 bytes in published
  user-supplied recipes.

None of these questions waits for a decision now; the corpus check left
five points of its own, listed in §4.6. Two small readings are the
author's and can be corrected without touching anything else: `none` stays
a value of every choice kind in `[needs]` (§6.7), and the ASSUMED point of
§3.10 was left to A3 (§19), which measured it on 2026-11-17. The one of §6.1
was settled by the corpus check.

## 17. Differences from the earlier design

An earlier design of the same task exists as a draft (`mediafile.md` on the
branch of `upc-5pw.113`, with a reviewer's findings applied; it recommends
HCL). This document was written without reading it; the comparison below
was made afterwards.

**Reached independently in both:** inputs by SHA-256 and size; sizes as the
first filter; libraries that read zip directories; catalogues as hints;
all or nothing with an exact report; a closed step set with no variables;
one file per artifact; recipes in parallel `recipes/` repositories whose
tag may move only to the same output digest; optional parts as attachments
by `subject`; export with file names in `org.opencontainers.image.title`
and the stated limit for unpacked archives; needs as kinds with
alternatives from the provider's vocabulary, stored in metadata schema 2.
Two designs agreeing on these is some evidence that they are right.

**Taken from it, because it is better:**

| What | Where here |
| --- | --- |
| A value with a stated hash is found instead of made; an archive is only a source for its members ("leaves and sources") | §3.4 |
| Nothing but bytes is read from an archive; order and times are written, by a `scaffold` command | §3.4, §6.9, §6.15 |
| The hash of a tiny file gives it away; `disclosable` | §6.13 |
| One artifact type per recipe kind | §7.1 |
| Outputs of user-supplied recipes are named under `private/` | §7.3 |
| A differing build may be kept under a local tag (here on request, there by default) | §3.8 |
| Catalogue ids are typed by the author; no catalogue data is copied | §6.8 |
| The box's own `requirements` text; the provider judges compatibility | §6.4, §6.7 |
| The FAT boot sector must be chosen before the freeze | §14 |
| Member sizes and times of LABFULL.ZIP and the size of the LiveCD ISO, measured there | the corpus |

**Where this design differs, and why:**

1. **TOML, not HCL.** The earlier design ranks TOML second, for one reason:
   its build part is "twice the lines". That is true of *its* model, where
   obtaining an input, declaring a medium, attaching an extra and packing
   are all steps (eleven step kinds, written as `[[build]]` tables with an
   `action` key). In this model inputs and outputs are tables of their own
   and three step kinds remain, used at most three times in a recipe. On
   the corpus TOML is then 5% shorter than HCL. The earlier design itself
   says TOML "would be the cheapest choice" if its build part were
   acceptable, and lists HCL's costs as this document does: two 0.x crates,
   an expression language to refuse, a folding rule for heredocs, no date or
   hexadecimal literals. The verdict follows the model, and the model is
   the next point.
2. **Values instead of a build that mutates.** There, `format` creates a
   disk and `add` changes it; `medium`, `extra` and `pack` append to the
   artifact. Here every step is a function from named values to one named
   value, and outputs are declared, not performed. This is what lets the
   checker derive required and optional inputs without a flag, and what
   makes the step list short.
3. **A lock instead of frozen encoders.** The earlier design freezes how
   configs, metadata and manifests are written (a canonical TOML writer of
   our own, an exactly pinned JSON crate, an `encoding = "1"` named in every
   recipe, a frozen annotation set). That works, and it binds every one of
   those writers for good; adding an annotation needs a new encoding. Here
   the author's release writes those three small files once and the recipe
   carries them, so only payload builders bind. The cost is three more
   files per pinned recipe and a description that exists twice (checked,
   §3.8). Its unconditional golden tests are kept; they guard the builders.
4. **A work file instead of `include`.** The earlier design shares between
   FreeDOS's three formats with fragments that merge both parts. Here the
   shared description lives in a work file with a fixed place, one level
   and a merge rule per field (§6.16); inputs and steps are repeated and
   shared by content only. The work file was added at the maintainer's
   request after the first round, which had no sharing at all.
5. **A tag and version scheme.** The earlier design keeps today's tags and
   leaves regional titles open. §8 here designs `<release>-rN` with a
   moving release alias, and adds the platform to repository paths.
6. **Output pins live in the lock**, not as `pin` attributes on built
   outputs. One `digest` in the file; the per-blob digests are in the
   locked manifest.
7. **The store is a source in v1.** There it comes later, for want of an
   index of decoded digests. Here it is first in the list, because a user
   who deleted the originals has nothing else; the index is a rebuildable
   cache and is named as work to do.
8. **The FreeDOS source archive** comes from a release asset here and from
   the published set through an `oci` source there. Both treat it as a
   prepared input; Q6 keeps both.

## 18. Sources

1. Dockerfile reference and BuildKit documentation (Docker): instructions,
   `LABEL`, `ADD --checksum`, the `# syntax=` directive.
2. HCL Native Syntax Specification (HashiCorp); `hcl-rs` (Martin Ohmann,
   crates.io); Packer documentation, HCL2 templates and `boot_command`;
   Docker Bake file reference.
3. TOML v1.0.0 and v1.1.0 specifications (toml.io); the `toml-test`
   conformance suite; Taplo and the Even Better TOML extension (JSON Schema
   validation of TOML).
4. KDL document language specification (kdl.dev).
5. Ansible playbook keywords and task syntax; GitHub Actions workflow
   syntax (`steps`, `uses`, `run`).
6. RPM spec file format (`Source`, `%prep`, `Release`); Arch Linux PKGBUILD
   (`source`, `sha256sums`, `pkgrel`); Debian Policy Manual (version and
   revision), DEP-5 machine-readable copyright, REUSE specification; Alpine
   and Gentoo package revisions (`-rN`).
7. Homebrew Formula Cookbook (`url`, `sha256`, `revision`).
8. Nix manual (fixed-output derivations) and Nixpkgs manual (`requireFile`,
   `fetchurl`).
9. MAME documentation, software lists (`hash/*.xml`: `software`, `cloneof`,
   `part`, `rom` with `size`, `crc`, `sha1`, `sharedfeat`).
10. Logiqx DAT format as used by No-Intro, Redump and TOSEC; clrmamepro,
    RomVault and igir documentation (rebuilding sets from DATs).
11. OCI Image Specification 1.1 and Distribution Specification 1.1
    (`artifactType`, `subject`, the Referrers API and its tag fallback);
    ORAS documentation (file names in `org.opencontainers.image.title`).
12. The Cargo Book (`Cargo.lock`); Nix flakes (`flake.lock`); SLSA
    provenance.
13. RFC 8878 (Zstandard); zstd manual and release notes. The format is
    stable; this document found no statement that encoder output is stable
    across library versions, and treats it as not stable.
14. Wikidata (item identifiers as work ids).

## 19. Implementation plan

A proposal for beads, in order. Nothing here is filed; the supervisor files
and claims. Marks: **[PRE]** must land before the first public release;
**[PUSH]** must land before the six published artifacts are deleted and
pushed again (decision-32, point 5), because it changes their names, tags
or manifest bytes; **[M]** needs the maintainer (a decision or an
approval); **[MOCK]** needs an approved mock-up before any GUI code;
**[um]** is work in unemu-media.

### 19.1. Track A: the existing code, before the push

**A1. Repository paths, tags and aliases. [PUSH] [PRE]**
Scope: the platform segment in repository paths (`unemu/media/<platform>/<work>`,
`unemu/firmware/<platform>/<catalogue id>`) and `private/` beside it;
`[<release>-]rN` as the one immutable form for media sets and firmware
packages; `<release>` as an alias that moves under the index lock; the
digit-first set-tag rule retired; firmware tags `rN` (today
`firmware/<id>:1` in `pc_image::firmware_import`); the template and media
builders of `pc_image::template` and their pinned tags; the store's tag
rules and the registry client's refusal to move a tag; the push guard's
namespace rules; the short local names a pull writes; the order Remotes and
Get templates list tags in (releases, then revisions, the alias shown as
"newest"). Depends on: the change that writes `ibm-pc` (decision-32).
Must prove: an `rN` tag never moves, locally or by push, with or without
`--move`; an alias moves only by compare-and-swap and only to a newer
`rN` of the same release; a semantic-version template tag behaves as
before; a store written by 0.0.2 with `freedos:1.4` still reads, lists and
runs; every digest this repository pins is re-pinned in the same change.

**A2. The frozen Zstandard profile. [PUSH] [PRE]**
Scope: name the encoder in use (library version, level 3, one thread, the
frame options of `image_store::codec`) as profile 1; pin the crates
exactly; commit test vectors, the compressed bytes of small fixed inputs
of each payload kind; state the rule that a later encoder is a new profile
beside this one. Decided by the maintainer: the `+zstd` suffix of the
version-1 media types *is* profile 1, so no manifest byte changes, and a
later profile gets a new marker.
Depends on: nothing. Must prove: the vectors fail when the encoder's
output changes by one byte (shown by a test that perturbs a parameter);
the three FreeDOS sets and three firmware packages rebuild to the digests
A1 pinned.

**A3. Export file names on layers. [PUSH] [PRE]**
Scope: `org.opencontainers.image.title` on every medium, ROM and extra
descriptor, written by the importers and builders. decision-37 is
accepted, so this is in the pre-push set: it changes manifest bytes, and
landing it later would leave the six artifacts without file names until an
`r2`. Depends on: nothing. Must prove: releases 0.0.1 and 0.0.2 read a
manifest with the annotation (MEASURED 2026-11-17 with scratch builds of
v0.0.1 and v0.0.2 reading titled artifacts, §3.10); an ORAS pull names the
files (a test pulls each of the six artifacts by title and decodes the
`.zst` files back to the fixtures).

**A4. The push. [M] [um]**
Not a bead of this repository: the maintainer deletes the six artifacts and
unemu-media pushes them under the new names and tags (decision-32). It
waits for A1, A2 and A3.

### 19.2. Track B: recipes

**B1. A throwaway parser and checker over the whole corpus.**
The first recipe step, as the process of §14 says. Scope: a small program,
outside the product crates and not kept, that reads every `*.toml` of the
corpus, applies §6.1 (including the refusal of TOML 1.1 forms), merges
work files (§6.16) and runs the static checks of §6.11; and a written
list of what it found. Depends on: nothing; it can start today. Must
prove or disprove: every corpus recipe passes, or the specification or the
recipe is corrected; the `toml` crate can be made to refuse the four 1.1
forms; the error messages name file, line and key; the schema of §6 can be
written as one JSON Schema. Its findings change this document before any
product code is written. Done on 2026-11-17: §4.6.

**B2. Metadata schema 2. [PRE]**
Scope: the title record gains `needs`, `requirements`, the work id, the
synopsis and work ids; the manifest gains `unemu.needs`; older releases
show schema 2 read-only (decision-30, point 5); the provider seam for the
needs vocabulary and the first `ibm-pc` vocabulary; the library's warning
and template suggestion are *not* in this bead. The first vocabulary is
the accepted one of §6.7. Depends on: nothing. Must prove:
0.0.2 opens a schema-2 artifact read-only; a schema-1 artifact keeps its
bytes when rebuilt; an unknown need is refused at check, not at run.

**B3. FAT builder additions.**
Scope: in `crates/media/src/fat12`: listed order, per-entry times, a
chosen serial, a volume label with its time, strict names (refuse, never
rename), a split to exactly N volumes that reports the number it needs.
The boot sector is the accepted one of §6.9: our own code that reports a
non-system disk, waits for a key and retries through INT 19h; it replaces
the halt loop and is frozen with recipe version 1.0.
Depends on: nothing. Must prove: today's `media create` goldens change in
the boot sector only and are re-pinned once; a machine started from a built
disk shows the message and, after a key, tries to boot again (a golden
test); the boot sector holds no vendor's bytes (its source is in the
repository); new goldens for each addition; Ken's
Labyrinth's disk and AZURAMON's split build twice to the same bytes.

**B4. The recipe crate: model, parser, checks.**
Scope: a platform-neutral crate beside `image-store` with the typed model
of §6, the parser over the `toml` crate with the restrictions B1 found, the
static checks, and the provider traits it calls (formats, volume profiles,
firmware rules, needs). `upcli recipe check`. Depends on: B1. Must prove:
the whole corpus passes; one failing fixture per rule of §6.11.

**B5. Work files.**
Scope: `<id>.work.toml`, the merge rules of §6.16, `upcli recipe show`,
and the check that the repository's last component is the work id.
Depends on: B4, B2 (the work id and synopsis need schema 2). Must prove:
the three corpus folders with work files show the same merged record the
first-round recipes stated in full; each merge rule has a test, the label
union and the list replacement above all.

**B6. Libraries, the index and the plan.**
Scope: library folders in preferences and `--library`; the index cache
with sizes, change times and computed hashes; zip directories; the index
of decoded digests for the store; catalogues as hints (CSV and
`SHA256SUMS`; the Logiqx DAT may follow); resolution in the order of §3.4
including "found instead of made"; `upcli recipe plan` with `--json`;
`upcli library …`. Depends on: B4. Must prove: a scan hashes only files of
a wanted size (counted in a test); a stale index entry is dropped, never
trusted; the plan for AZURAMON finds 561 entries from 204 files; a near
miss is reported.

**B7. Steps and the draft build.**
Scope: `unpack` (zip), `text`, `fat` through B3; outputs handed to the
importers' shared plan-and-publish functions; the download cache and
`--fetch` with consent; `upcli recipe build` for drafts; `--keep-as`.
Depends on: B4, B6, B3 (for `fat`). Must prove: the three firmware recipes
and the three FreeDOS recipes build artifacts whose configs equal the ones
`image firmware import` and `image media import` make today; nothing is
tagged when an input is missing; no step can reach the network or the
host clock (a test build runs with both denied).

**B8. The lock and pinning.**
Scope: `upcli recipe pin`; the lock directory; the checks of §3.8,
including the field-by-field comparison and the work-file drift report;
building from a lock with a release other than the one that pinned it; the
conformance harness that builds pinned recipes from a fixture library and
compares digests. Depends on: B7, A2, B5. Must prove: a recipe pinned by
one build of `upcli` builds the same manifest digest with another whose
manifest writer was deliberately changed; a changed work file is reported
and does not change the result.

**B9. `scaffold` and `inputs`.**
Scope: `upcli recipe scaffold` (input tables, member lists with sizes,
hashes and times, file sets) and `upcli recipe inputs` (DAT, CSV,
checksums). Depends on: B4. Must prove: scaffolding LABFULL.ZIP and the
AZURAMON folder gives the corpus's tables byte for byte.

**B10. Recipe artifacts in the registry.**
Scope: the two artifact types and the config type; push and pull; the
bundle (work file, bundled inputs, lock); the recipe guard (nothing but
those; `disclosable`; a tag replaced only by a recipe that pins the same
output digest); `unemu.build.recipe` on local tags; the decision-30 rows
for recipe versions, steps and formats. Depends on: B8, A1. Must prove: a
recipe pulled into an empty store plans and builds with no other file;
each guard refusal has a test; a registry row can be judged from manifest
and config alone.

**B11. Export.**
Scope: `upcli image export`, `--files` with the FAT reader promoted from
test code, the per-input "kept" report of `recipe build`, and the layout
documented with a script that uses no `upcli`. Related and distinct:
`upc-5pw.116` saves and loads whole artifacts, selections and repositories
as OCI Image Layouts; export unpacks one artifact into plain files.
Depends on: A3. Must prove: the script and the command write the same files; an
exported AZURAMON tree equals the input files byte for byte.

**B12. Optional parts.**
Accepted (Q5), and built after the first recipes work. Scope: `subject` in the manifest codec, the
local referrers table, `[[attachment]]` builds and pins, the tag fallback
for registries without the Referrers API; shared with phase 3 of
registry-library-plan. Depends on: B8, B10. Must prove: the main
artifact's digest is the same with and without the optional inputs; an
attachment found later is added without rebuilding the set.

**B13. The Recipes group in Remotes. [MOCK] [M]**
Scope: first a mock-up of the group, its row states (§7.4), the missing
report and the consent to download; after approval, the implementation
over B6's plan and B10's pull. Depends on: B6, B10; the mock-up can start
at once. Must prove: every state of §7.4 has a model test and a frame of
the mock-up.

**B14. unemu-media's migration. [um]**
Scope: the three firmware packages, then the three FreeDOS sets with the
source archive as a prepared input (a release asset, built once by the
script that exists), then Ken's Labyrinth; the recipes with their locks as
the conformance corpus in CI; `build.sh`, `lib/recipe.sh` and
`lib/publish.py` retired except the one script that makes the source
archive. Depends on: B8 for building, B10 for publishing recipes. Must
prove: the recipe-built artifacts equal the ones A4 pushed, digest for
digest, or the difference is explained and they become `r2`.

**B15. One private recipe, end to end. [M]**
Scope: AZURAMON or Windows 3.0, written with `scaffold`, built from a
library by someone who did not write it. Depends on: B8, B6. Must prove:
the gate of §14; its findings are the last input before the freeze.

**B16. A Machinefile corpus.**
Documents only: three or four real installations written as Machinefiles
in TOML, to test the syntax where it is weakest. Depends on: nothing; best
done while B4 to B8 are built. Must prove: TOML is readable there, or the
fallback of decision-33 is taken before the freeze.

**B17. Freeze 1.0. [M]**
Scope: the decision record, the published JSON Schema, the promises of §9
written into the user documentation, the conformance corpus required by
the release gate. Depends on: B14, B15, B16.
Recipes are published only after it.

### 19.3. Order and what blocks the release

```text
A1, A2, A3 -> A4 the push
B1 -> B4 -> B6 -> B7 -> B8 -> B10 -> B14 -> B17
            B5 (needs B2)   B9      B13 (mock-up early)
      B3 ----^                      B12, B11
                          B15, B16 -> B17
```

Before the first public release: A1, A2, A3 and the push; B2, so that the first public release reads schema 2. The recipe
track itself need not be finished for the release: recipes stay
experimental at `0.x` and none is published before B17.
