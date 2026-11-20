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
design examples and first-round HCL snapshots is in
[`recipe-corpus/`](recipe-corpus/README.md). The three firmware and three
FreeDOS TOML recipes, their locks and `freedos.work.toml` are shipped inputs
in [`crates/pc-image/data/recipes/`](../../../crates/pc-image/data/recipes/),
under the same subfolder names; the template builder embeds them. Recipe
checks read both locations.

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
the user's copies had. `recipe build` therefore prints, per input, how and
in which exported files it is kept (`kept as stored in F`, `kept as files
in F, G`, `members kept as …` for an unpacked archive, then `not kept: the
archive itself, N members unused`; B11's wording, 2026-11-19), so a user
who means to delete the originals knows what stays. A recipe that wants the
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

**After B1.** Five points needed a decision. The maintainer ruled on
O1 to O4 on 2026-11-17, and B4 implemented them (§19.2); O5 stays open.

- **O1. Software platforms of new platforms. Decided:** each platform
  provider owns its list of software platforms and its `[ids]`
  catalogues, append-only (§9, row 7). `ibm-pc` takes `DOS`,
  `Windows 3.x`, `Windows 9x`, `CP/M` and `other`, and `wikidata`,
  `mobygames` and `igdb`, within metadata schema 1's lists
  (`image_store::metadata`). A provider for the NES or the Amiga brings
  `NES` or `AmigaOS` with it.
- **O2. Four more static checks. Decided:** they are rule 10 of §6.11: a
  stated size the medium's format does not allow; a `fat` step whose
  format differs from the medium it feeds; a name in a `fat` tree that the
  volume profile refuses (§6.9 leaves the build to refuse it, but the names
  are in the recipe), and a label it would change; a ROM whose size or
  hash differs from its catalogue entry. None fires on the corpus.
- **O3. Who keeps the schema. Decided:** it is generated from the
  parser's key tables and the providers, never kept by hand; a test
  compares the published file with it, and the freeze publishes it (B17).
- **O4. A work file's version. Decided:** a recipe names a work file of
  its own recipe version only.
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
- A work file may name **text files** in its own folder, each holding the
  about text of one language (§6.16; specified, not implemented yet:
  `upc-miu`). Such a file is not a recipe file and is not TOML: a regular
  file, not a symbolic link, at most 65 536 bytes as it is on disk, UTF-8
  without a byte-order mark and without NUL.
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
| `software_platform` | string from the platform provider's list (`DOS`, `Windows 3.x`, … for `ibm-pc`; §4.6, O1) | the title's `platform` |
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
writer. The metadata's form and the annotation's encoding are in
machine-images §2.1 and §2; in the metadata a single value is written as
an array of one.

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
| `disclosable` | no | `true`: this file is under 256 bytes and its hash may be published all the same (§6.13). Also allowed on a file-set entry and on a member of an `unpack` step (§6.9). |

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
| `members` | An array of `{ path, size, sha256, time, disclosable }`; only `path` is required, `disclosable` (§6.13) is `false` by default. Only these members are taken; the tree has them in this order, each with its stated `time` or none. |
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
| `order` | `"name"` (default): in every folder its files, then its folders, each by the bytes of their 8.3 names as stored. `"listed"`: the tree's own order, which is the order its members or files are written in the recipe. |
| `time` | A local date-time for entries that have none, for folders and for the label. Default `1980-01-01T00:00:00`. |
| `volumes` | A whole number N from 2: split the tree over exactly N volumes, in folder order. |

Without `volumes` the result is a file, the image, and a tree that does not
fit is an error. With `volumes` the result is a tree with the members `1`
to `N`; a tree that needs another number of volumes is an error that states
the number. Names must already be valid for the file system (8.3 names as
stored, upper case; the folder depth DOS allows): the step refuses a bad name and never renames. No
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
   outputs or 100,000 file-set entries;
10. what the provider refuses of the platform's data (§4.6, O2): a stated
    size the medium's format does not allow; a `fat` step whose format
    differs from the medium it feeds; a name, a folder depth or a label in
    a `fat` step that its volume profile refuses or would change; a ROM
    whose size or SHA-256 differs from its catalogue entry, found by
    `catalogue_id` or by its bytes.
11. *(specified, not implemented yet: `upc-miu`)* the about text and the
    localized synopsis of the merged record
    ([about-text](about-text.md) §5.2): the shapes and limits of metadata
    schema 3 (machine-images §2.1); the guard rules R1 and R2 of §6.13; for
    each `{ file = "NAME" }` of `[title.about]`, the rules of a text file
    (§6.16), each refusal naming the work file's key and the file. It then
    prints the warnings W1 to W6 of §6.13 and the note of linked hosts, each
    with its file (the work file or the text file) and its line, and still
    passes.

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
  256 bytes, unless the entry says `disclosable = true`. A file is named by
  a file input, a file-set entry or a member of an `unpack` step that
  states its size: each publishes the file's hash (decided by the
  maintainer, 2026-11-19, for members). `recipe scaffold` never writes the
  key; the author marks what may be published. The rule and the key are
  the earlier design's.
- **About texts** (specified, not implemented yet: `upc-miu`;
  [about-text](about-text.md) §4). Every about text of metadata schema 3
  (machine-images §2.1) has, per language, a source: `own`, or `licensed`
  with a title, authors, an `https` URL and one of five Creative Commons
  licences. These rules refuse:

  | # | Where | What |
  | --- | --- | --- |
  | R1 | The metadata codec, always | A source for a language without a text; a source of the wrong shape; an about text that is not in canonical form. Writers also: a kind that is not `own` or `licensed`; a `licensed` source without `title`, `authors`, `url` or `license`; a `url` that is not a link address; a licence outside `CC0-1.0`, `CC-BY-3.0`, `CC-BY-4.0`, `CC-BY-SA-3.0`, `CC-BY-SA-4.0`; a key the kind does not take; a `url` on a Wikimedia host (`wikipedia.org`, `wikimedia.org`, `wikibooks.org` and their subdomains) whose `license` is not `CC-BY-SA-4.0` or `CC-BY-SA-3.0`. |
  | R2 | `recipe check`, `recipe build`, and any other writer that marks an artifact open | An about text without a source in an artifact stated open. The metadata layer of an open artifact is marked open, and a text nobody vouches for cannot be. |
  | R3 | The image push guard, as its rule 6 | Into `unemu/` on any host, or into a namespace or registry not marked private: every about text of every title in the pushed metadata needs a source of a kind this release knows, and the licence of each `licensed` source must be named by the metadata layer's own `unemu.license`. The refusal names the title, the language and what is missing, and says "There is no override for this rule." A namespace marked private outside `unemu/` is not checked. |
  | R4 | The recipe guard, on every `recipe push` | A bundled work file, a text file it names, or locked metadata, with an about text that has no source. A recipe artifact is public text whatever its output is (§7.3), so it has no private case. With a `licensed` text aboard, the layer that holds it says so (§7.1). |
  | R5 | The image push guard, into `unemu/` only | A `synopsis` or an `about` without an `en` entry. |

  Links add no refusal: only a link address becomes a link, in every
  reader, so a destination with another scheme is text. `recipe check`
  **warns**, prints the warning and passes, and the push dialog shows it as
  a note line:

  | # | What |
  | --- | --- |
  | W1 | An about text without a source in a recipe without `[license]`: "builds, and stays in this library and on private registries; the recipe cannot be pushed". |
  | W2 | An `own` text that reads as pasted: a bracketed reference mark (`[12]`, `[citation needed]`), "All rights reserved", a copyright sign or `(c)` before a year. The warning names the line and the pattern. |
  | W3 | A `licensed` source on a Wikimedia host whose URL names no revision (`oldid=`): "the article will change; link the revision the text was taken from". |
  | W4 | Markdown outside the subset, each with its line: "shown as typed". Also a line indented by four spaces or more, which other readers show as a code block. |
  | W5 | `[text](destination)` or `<…>` whose destination is not a link address: "not a link; shown as typed". |
  | W6 | A block quote in an `own` text: "a quoted passage; name its source in the text". |
  | Note | The hosts the text links to, one line: "links to en.wikipedia.org, www.freedos.org". Not a warning. |

  W4 and W5 stay warnings under `unemu/` too, so the public catalogue is
  not tied to one release's subset. The guard never fetches an address;
  what it cannot check (that an `own` text is the author's, that a
  `licensed` text is what the URL holds) is the publisher's review.

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
| `upcli recipe show FILE` | A media recipe's identity and its description merged with its work file (§6.16), each value from the work file marked. Refuses a recipe that does not pass `check`. |
| `upcli recipe plan FILE\|REF [--json]` | Resolves every input and prints the report of §3.5. Writes nothing. |
| `upcli recipe build FILE\|REF [--fetch] [--library DIR]…` | Builds and tags the artifact and every attachment whose inputs are found. `--fetch` is the consent of R15. |
| `upcli recipe pin FILE` | The author's build: writes `digest` into the file and the lock beside it. |
| `upcli recipe conformance FILE\|FOLDER…` | Builds every pinned recipe named into a scratch store from the libraries and compares the digests (B8). |
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
| `developers`, `publishers`, `genres` | yes | yes | The recipe's list replaces the work's. A recipe states no empty list: it leaves the key out to keep the work's |
| `labels` | yes | yes | The union: the work's, then the recipe's new ones |
| `synopsis` | yes | no | — |
| `about`, `about_source` (specified, not implemented yet: `upc-miu`) | yes | no | — |
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

**`upcli recipe show FILE`** prints the identity line (the tag, the digest
or `draft`, the platform) and the merged description record: `[title]`,
`[needs]` and `[ids]` with the work file's fields, and nothing of the
inputs, steps, media or licence. It writes them in the plain lines of
`upcli image media show`, not as a TOML document, with a comment on each
value that came from the work file (B5, §19.2).
`check` works on the merged result and names the file a bad value came from.

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

**About text and localized synopsis** (specified, not implemented yet:
`upc-miu`; [about-text](about-text.md) §5, metadata schema 3 in
machine-images §2.1). `[title]` of a work file gains two keys, and
`synopsis` a second type. All three are refused in a recipe, so each text
has one home:

| Key | Type | Content |
| --- | --- | --- |
| `synopsis` | table: language tag to string; a plain string is still read, as `{ en = "…" }` | One plain paragraph per language, at most 500 characters, no line break, no markup read. Written in the work file; it has no `file` form. |
| `about` | table: language tag to **either** a string **or** a table with the one key `file` | The Markdown source of the long text. The string is the source itself (a short text as a multi-line string, `en = """…"""`); `{ file = "NAME" }` names a text file beside the work file that holds it. Either way the text is normalised to canonical form and has the limits of machine-images §2.1, so the two forms give the same metadata bytes for the same text. |
| `about_source` | table: language tag to a table with the keys `kind`, `authors`, `title`, `url`, `license`, `changes` | The source of each language's text (machine-images §2.1). |

```toml
[title.about]
en = { file = "freedos.about.en.md" }

[title.about_source.en]
kind = "own"
authors = ["unemu contributors"]
```

A `licensed` source, an abridged article:

```toml
[title.about_source.nl]
kind = "licensed"
title = "FreeDOS"
authors = ["Wikipedia contributors"]
url = "https://nl.wikipedia.org/w/index.php?title=FreeDOS&oldid=00000000"
license = "CC-BY-SA-4.0"
changes = "abridged; converted to Markdown"
```

(The revision number is made up.) In the table "Which field lives where",
`about` and `about_source` are rows like `synopsis`: work file yes, recipe
no. A work whose only release needs an about text gets a work file.

**A text file.** `file` is a file name, not a path: one line of at most 128
characters, without `/` or `\`, not `.` or `..` (the checker's `FileName`
form), in the work file's own folder. It differs from the recipe's and the
work file's own names, from every bundled input's, and between languages.
The ending is never read; `<work id>.about.<tag>.md` is the suggested name.
The file is a regular file, not a symbolic link, at most 65 536 bytes on
disk, UTF-8 without a byte-order mark and without NUL (§6.1), then
normalised and limited as an inline text. It is pinned by the lock, not by
a hash in the work file: the lock holds the metadata blob with the
canonical text inside it. It is not an `[input]`.

| Command | With a `file` |
| --- | --- |
| `recipe check` | Rule 11 (§6.11): the name; the file is there, a regular file, within the size, UTF-8; the normalised text within its limits; the warnings with the file's own line numbers. A missing file is refused for a pinned recipe and a draft alike. For a pinned recipe it lists `title.about.en` among the differences when the file's text is no longer the lock's. |
| `recipe show` | The `about` line says where the text is ("from the work file, file freedos.about.en.md"); `--about [TAG]` prints the normalised source, the bytes a build would store. |
| `recipe build` | A draft reads the file and writes its text into the metadata; a pinned recipe builds the lock's metadata and reads the file only for the report of differences. |
| `recipe pin` | Reads the file and writes the metadata with the text; the lock keeps that blob. It writes nothing into the work file. |
| `recipe scaffold` | Unchanged: it writes no about key and no text. |
| `recipe push`, `recipe pull` | The artifact carries each text file its work file names, as a layer (§7.1); a pull writes each at its name beside the work file, after the checks. |

**Merge and description.** The merge copies `synopsis`, `about` and
`about_source` whole, every language, into each release's title record, a
`file` replaced by its text; the release's `languages` field filters
nothing. The title record's `description` is the recipe's; when the recipe
states none, the synopsis in `en`, else its first entry in byte order. It
stays one plain string. A corrected about text reaches a release when its
author pins a new revision; `check` lists the differences per language key
(`title.about.nl`, `title.about_source.nl.url`).

**Version and compatibility.** The keys are added to recipe version `0.1`.
A recipe artifact that uses them, or carries a text file, declares 0.0.4
(machine-images §5.6, the rows of schema 3). `recipe-schema-0.1.json`
gains the keys when this is implemented (`about`'s value: a string, or an
object with exactly `file`).

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

**The text-file layer** (specified, not implemented yet: `upc-miu`;
[about-text](about-text.md) §5.5). A work file's `{ file = "NAME" }`
(§6.16) travels as a layer of its own, media type
`application/vnd.unemu.recipe.text.v1`, "a text file a work file names",
titled with its file name like every other part: at most 65 536 bytes,
text by the rule bundled inputs have, its bytes pinned by the layer's
digest. It is not a bundled input, which has a size and a hash in the
recipe. The artifact holds exactly the text files its work file names; one
it does not name, or a missing one, is refused. The layers are in the
order: the work file, its text files in the order of their language tags,
the bundled inputs, the lock. The type has a row in
`image_store::compat::RECIPE_TYPES`, first read by 0.0.4, which raises the
artifact's `unemu.min-app-version` (machine-images §5.6).

With a `licensed` about text aboard (R4 of §6.13), the layer that holds it
says so: a text file's layer is marked with its source's licence alone, the
work-file layer (for a text written inline) and the lock's metadata layer
`CC0-1.0 AND <licences>`, and the artifact's
`org.opencontainers.image.licenses` names them all, where otherwise it is
`CC0-1.0`.

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

As built (`upc-5pw.113.18`), "lost" means nothing at all at the lock's
path; a lock that is there and broken is refused, `recipe check` still
reports rule 8, and the build writes no lock back.

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

**The conformance corpus.** The shipped recipes and their locks in
`crates/pc-image/data/recipes/`, plus the remaining pinned recipes in
`recipe-corpus/`, are the test: every release builds each one from a fixture
library and compares the manifest digest. A release that cannot reproduce one is not released. This
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
Done on 2026-11-17 (`upc-5pw.113.2`): the title record's `needs`,
`requirements`, `work_id`, `synopsis` and `work_ids` in metadata schema 2
(machine-images §2.1), which a writer uses only when a title sets one of
them; the compat rows of decision-30 rate schema 2 and each new field by
0.0.3 and never raise the marker; the provider seam
`image_store::needs::Vocabulary`, reached through `ConfigValidator::needs`
and `MediaFormats::needs`, with the `ibm-pc` vocabulary in
`pc_image::needs`; the import's check that the work id is the repository's
last component; the annotation `unemu.needs` (machine-images §2); the
flags of `upcli image media import` (commands.md). Release 0.0.2's decoder
shows a schema-2 catalogue read-only, measured. The checks of B4 call the
same vocabulary.

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
Done on 2026-11-17 (`upc-5pw.113.3`). The typed API the `fat` step calls
in B7 is `media::fat12::plan_volumes(entries, &Volume)`: `Entry` is a path
with `/` separators, the bytes (or none for a folder) and an optional
`DosTime`; `Volume` holds `format`, `label`, `serial` (default
`DEFAULT_SERIAL`, `0x28959855`), `order` (`Order::Name` or
`Order::Listed`), `time` (default the FAT epoch) and `volumes`
(`Some(N)` from 2). Decisions taken while building it:
- `order = "name"` (maintainer's ruling, 2026-11-17): every folder's
  directory entries, and so its clusters, are its files first, then its
  folders, each group by the bytes of the stored 8.3 names, as XCOPY /S
  writes a tree and as the split takes them. One rule for recipes,
  `media create --add` and the window's Create from files (the shared host
  builder, which before ordered files and folders together by host-name
  bytes; numeric tails are still assigned in host-name order). With the
  same stored names, sizes and default keys a volume equals
  `media create --add`'s byte for byte (a test). `"listed"` orders each
  folder's members by the first appearance of their paths.
- An entry's time is written as its creation, last-access and last-write
  time (DOS before 4.0 reads only the last); the default epoch writes
  today's bytes. A folder's `.` and `..` take the folder's time.
- Names (maintainer's ruling, 2026-11-17): every part must already be the
  8.3 name as stored, upper case; a lower-case letter is refused like any
  other bad name, as the recipe states the name as it is on the disk. Host
  files keep their conversion, upper case included. The error names the
  entry, the part and the rule (characters, lower case, periods, stem or
  extension length, a DOS device). A
  path named twice, or as both a file and a folder, is refused; a folder
  above files may still be listed once, for its time. A folder deeper than
  the 64 characters DOS keeps for a current directory, the leading
  backslash counted, is refused (64 pass, 65 do not).
- `volumes = N` splits greedily in folder order and is an error unless
  that fills exactly N volumes; the error states the number, or says that
  the files fit on one volume. An empty tree is a formatted volume.
- The boot sector lives in `crates/media/src/fat12` (`boot.asm`, with the
  bytes and a commented listing in `boot.rs`), beside the PC/AT diskette
  geometries and BPB values: that module is today's IBM PC volume profile,
  and the CLI and the window call it directly, so moving the boot code into
  `pc-image` would split one profile across two crates. When B4's volume-profile
  trait moves the profile, the boot code goes with the geometries. 100
  bytes of 8086 code and message from offset 62, each message line within
  40 columns; the test `boot_sector_source` assembles the source with nasm
  and compares. Measured against `main` before this bead for all four
  formats, labelled and not, filled disks and a split set: blank and
  labelled images and the split, whose folders were already in files-first
  order, changed in bytes 63 to 160 of sector 0 only; the filled disks
  hold the same files, folders, attributes, times and contents, with a
  folder's entries and clusters now files first. Their pins were taken
  again once. Two goldens boot a built disk, on the Xi8088 BIOS and on the
  open AT BIOS without option ROMs, see the message, take a key and see it
  again. SHA-256 pins of synthetic volumes freeze listed order, entry
  times, the serial with the label and its time, and each volume of an
  exact split.
- The user-file goldens look for the files by size and SHA-256 in the
  user-ROM roots (listed in `vendor/user-roms/README.md`), take the step's
  keys from the corpus recipe, and skip without the files. Ken's Labyrinth's disk is MEASURED
  and pinned from the author's LABFULL.ZIP; AZURAMON's split runs only
  where its files are present.

**B4. The recipe crate: model, parser, checks.**
Scope: a platform-neutral crate beside `image-store` with the typed model
of §6, the parser over the `toml` crate with the restrictions B1 found, the
static checks, and the provider traits it calls (formats, volume profiles,
firmware rules, needs). `upcli recipe check`. Depends on: B1. Must prove:
the whole corpus passes; one failing fixture per rule of §6.11.
Done on 2026-11-17 (`upc-5pw.113.4`). The crate is `crates/recipe`
(layer `neutral`): `spec` holds every table of §6 with its keys, types and
required keys, and drives both rule 1 and the JSON Schema; `model` is the
typed model, every value with its span and key path; `provider` holds the
traits a platform implements (`Provider` with `MediaProfile`,
`VolumeProfile` and `FirmwareProfile`, the needs `Vocabulary` of B2, the
software platforms and the `[ids]` catalogues); `check_paths` runs §6.11
and returns every finding, sorted and each once, with file, line, column,
rule and key path (1-based indices, `step[1].members[2].path`). The
`ibm-pc` side is `pc_image::recipe::PcRecipes`, over the importers'
formats and sizes, the FAT12 builder's strict names and folder depth, the
DOS names of extras, the ROM roles, lanes, boards and firmware catalogue
and `pc_image::needs`. `upcli recipe check FILE|FOLDER...` prints the
findings and a line per file with the split of rule 5; exit 0, 1 with
findings, 2 on a usage error (commands.md, Checking recipes). Decisions
taken while building it:
- The corpus, MEASURED: the `ibm-pc` files pass; the five `nes` and
  `amiga` recipes each get the one rule-7 finding that this build has no
  provider for their platform, and nothing behind it; with test providers
  for those two platforms, built from the corpus's ASSUMED words, all 18
  files pass (a test in `pc-image`).
- O1, as ruled: each provider owns its software platforms and `[ids]`
  catalogues, append-only. `ibm-pc` takes `DOS`, `Windows 3.x`,
  `Windows 9x`, `CP/M` and `other`, and `wikidata`, `mobygames` and
  `igdb`; a test keeps both within what metadata writers accept. A work
  file's title, ids and needs are checked against the provider of every
  recipe that names it; the findings name the work file, once.
- O2, as ruled, rule 10 is four checks: a size the medium's format does not
  allow (from an input or a member that states it), a `fat` step whose
  format differs from the medium it feeds (one volume or a volume of a
  split), a name or folder depth the volume profile refuses (and the
  `fat` step's label), a ROM whose size or SHA-256 differs from its
  catalogue entry.
- O3: the JSON Schema is written by a test, not by a command. The test in
  `pc-image` (`recipe_check`) compares `recipe-schema-0.1.json` with what
  `recipe::schema::render` writes from the key tables and `PcRecipes`,
  and writes it with `UNEMUPC_WRITE_RECIPE_SCHEMA=1`. A test needs no
  user-facing command before the freeze, and it fails on any drift; B17
  can add a command when the schema is published. The generated schema
  replaces the hand-written draft. It states every key, type and required
  key, the closed lists, the string forms, one variant of an input, a step
  and a ROM, and per platform the provider's vocabularies, needs included;
  it leaves the licence rules, the `unemu/` rule, the components of
  `name`, and the size limits of `bundle` and `disclosable` to the checker.
  MEASURED with Python `jsonschema` 4.26: it passes the 2020-12
  metaschema, accepts all 18 corpus files and refuses the shape faults of
  the rule fixtures.
- O4, as ruled: a recipe names a work file of its own recipe version only
  (rule 3, at the recipe's `recipe` key). A work file whose `[work].id` is
  not its file name's id is refused there too, and so is a recipe whose
  repository's last component is not its work id (§6.16, taken from B5).
  A file named as a work file that holds a recipe is refused (rule 2), and
  the recipe's own missing title is still reported.
- Where B1's other checks went: the kind and platform components of `name`,
  and a title stated by neither the recipe nor its work file, are rule 2; a
  missing work file, a path listed twice and a DOS name used twice are rule
  3; `disclosable` on 256 bytes or more and `players.max` below `min` are
  rule 1; a retired platform id (`ibm-pc-at`) and an extra's DOS name the
  provider refuses are rule 7; `[license]` absent under `unemu/` is rule 6.
- Cascades: a reference that fails marks the value it names as used, and
  only a failed one does, so an input read only by an unused step is
  reported with it and stays out of the split of rule 5; a step with two
  verbs or none is one finding and makes nothing it names unused or
  unknown; an input that is not a table is no file and no tree, so
  references to it say nothing more; a work file that is not readable is
  reported once; a retired platform id (`ibm-pc-at`) is reported once and
  no rule that needs the provider runs after it.
- The catalogue rules of 6 and 10 find a ROM by `catalogue_id`, or else
  by its bytes (`FirmwareProfile::identify`): a ROM whose stated SHA-256
  is a catalogue entry's, or one chip of which has an entry's chip SHA-256
  and size. A user-supplied ROM known that way takes no `[license]`
  (§6.6), and a whole ROM known by its hash has the entry's size.
- Smaller rulings of the fix round: a time's second is at most 58 (FAT
  and zip store no leap second); `[ids]` values are lines of at most 128
  characters, as the metadata writer keeps them; a `release` is at most
  107 characters, so that `<release>-rN` fits the 128 characters of an OCI
  tag with the 19 digits of the largest revision; a `fat` label is stored
  as written, so one the builder would upper-case is refused; in a
  multi-line string a CR LF is one line break, so a recipe saved with
  Windows line ends reads the same, and any other CR is refused; a folder
  walk takes `*.media.toml`, `*.firmware.toml`, `*.machine.toml` and
  `*.work.toml` and skips hidden files and folders, while a file named on
  the command line is read whatever its name.
- The volume profile judges names and labels only. The boot code and
  geometries stay in `media::fat12` until B7 builds volumes through the
  provider; `media::fat12::MAX_FOLDER_PATH` became public for the depth
  check, with no change of behaviour.

**B5. Work files.**
Scope: `<id>.work.toml`, the merge rules of §6.16, `upcli recipe show`,
and the check that the repository's last component is the work id (done
in B4).
Depends on: B4, B2 (the work id and synopsis need schema 2). Must prove:
the three corpus folders with work files show the same merged record the
first-round recipes stated in full; each merge rule has a test, the label
union and the list replacement above all.
Done on 2026-11-18 (`upc-5pw.113.5`). The merge is `recipe::merge`:
`merge_path(FILE, providers)` runs the checks on the file and gives
`Shown::Merged` for a media recipe that passes, `Shown::Refused` with the
check's report, or `Shown::NotMedia`; a file that no longer reads as it did when it was checked is
an error of its own. `Merged` holds the identity; each field the recipe and
its work file share, with the file it came from (`Sourced`, `Origin`);
the release fields, the recipe's alone, as plain values; the synopsis; and
`[ids]` and `[needs]` per key, each with its file; `Merged::title` maps it onto
`image_store::metadata::Title` and `Merged::metadata` onto the whole
metadata (the platform by its current id, the labels as the envelope's
labels, the one title). `upcli recipe show FILE` prints it (commands.md,
Checking recipes). Decisions taken while building it:
- The first-round full records are the `.hcl` copies of the corpus, which
  the corpus keeps as the first round wrote them (corpus README, second
  round); the TOML files were rewritten in place. MEASURED, field by field
  through a test that reads their `title`, `needs` and `ids` blocks: all
  seven recipes that have one (FreeDOS's three, Ken's Labyrinth 2.01,
  Contra's USA, Japan and Europe, the last with the test provider for
  `nes`) merge to the same title, series, version, edition, year,
  publishers, developers, genres, software platform, regions, players,
  description, needs and ids. The merge adds what schema 2 gives the work:
  the work id and the synopsis. Labels are the same set; Ken's Labyrinth
  2.01's are in another order (`game`, `first-person-shooter`, `freeware`
  where the first round wrote `freeware` second), because the union puts
  the work's labels first. Since labels keep their order in the metadata,
  a build of that recipe writes other metadata bytes than an import with
  the first round's order would.
- The record's `id` is the last component of `name`, as §6.4 says, so the
  work id for a recipe that names one; a test asserts it on the corpus.
  `image media import`, which B7 hands its outputs to, writes the id
  `title` for every set today; ruled 2026-11-18: in B7 the importer follows
  §6.4.
- A recipe's empty list of credits or genres (`publishers = []`) is
  refused (rule 1, with the advice to leave the key out, which keeps the
  work's list): it could only mean "none", which the metadata cannot hold,
  as it writes no empty list and absent means unknown there. The other
  lists of `[title]` may still be empty, and an empty list in a work file
  states nothing.
- The union keeps each label once: the work's in its order, then the
  recipe's it lacks, a repeat within the recipe dropped.
- `[ids]` and `[needs]` are sorted by catalogue and kind, as the metadata
  keeps them; a need's alternatives keep the author's order.
- `show` prints the record in the lines of `upcli image media show`
  rather than as one TOML document (§6.16 above), so that a recipe's
  record and an imported set's read alike: the same field names and order, with the title fields
  that command leaves out beside them, and `# from NAME.work.toml` after a
  value from the work file. It takes one media recipe; a firmware recipe
  or a work file has no title record to merge and is exit 2, as is a
  folder.

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
Done on 2026-11-18 (`upc-5pw.113.6`). The neutral code is in
`crates/recipe`: `library` (the scan, the index and its cache), `zip` and
`inflate` (a zip archive's central directory and its members' bytes),
`store_index` (the store's decoded digests) and `plan` (resolution and the
report), with `recipe::plan_path` checking a recipe before it plans it;
the commands are in `crates/cli-core` (`recipe plan`, and `library` in
its own module). `upcli recipe plan FILE [--json] [--library DIR]...
[--store DIR] [--config FILE]` prints the report of §3.5 and exits 0 when
every input the artifact needs is found or not needed, 1 when one is
missing or the recipe has findings (the check's report is printed
instead), 2 on a usage error or a file that cannot be read; `upcli library
ls|add|remove|scan` keeps the list and the index (commands.md, Planning
recipes and libraries). MEASURED: with the user's copy of the game folder as a library
(`vendor/user-roms`), AZURAMON's plan finds all 561 entries in 204 files,
one per distinct content, hashing exactly those 204 and nothing else; a
second plan hashes nothing (the test `user_rom_azuramoon_plan_finds_561_entries_from_204_files` in `cli-pc`).
Decisions taken while building it:
- Spellings, as commands.md has them: `library ls` for §6.15's `library
  list`, `library scan` for `library index`, and `library remove`, which
  §6.15 lacks. `--catalogue FILE` is not taken: a catalogue is read where
  it lies in a library, by its name (below). `recipe plan` takes a file;
  `REF` came with recipe artifacts (B10).
- Libraries are the preferences key `libraries`, absolute paths in search
  order, which `library add` and `remove` change under the preferences
  lock as every writer does; `--library DIR` (repeatable) adds folders
  after them for one run, a folder named twice searched once. A library of
  the preferences that is not there (a disk not attached) is skipped with a
  warning; one named with `--library` must be there.
- The index: one JSON file per library in the store's
  `cache/libraries/`, named by the SHA-256 of the library's absolute path,
  with each file's path, stamp and, once computed, SHA-256, and each zip
  archive's members (name, raw name bytes, size, CRC-32, method, offsets,
  a problem that keeps it from the search, SHA-256 once computed, or why
  hashing it failed) or why the archive could not be listed. The stamp is size and
  modification time, and on Unix also the inode, the device and the change
  time. An entry whose stamp no longer matches its file is dropped and the
  file hashed again when wanted, never trusted. What is detectable: on Unix
  a rewrite that keeps the size and puts the modification time back still
  moves the change time, which no ordinary program can set, so it is seen
  (a test does exactly that); elsewhere the stamp is size and modification
  time only and such a rewrite is not seen until the file is used, which
  hashes it again (R4, R9). Without a store the index lives for one run.
- A scan (`library scan`, and the listing a plan starts with) reads
  directory entries, zip central directories and catalogues and hashes
  nothing. The plan hashes a file only when a value of its size is wanted,
  and then the candidates of that size one by one in the order of §3.4
  until one matches, the path a file set gives first, so a copy of a whole
  folder is hashed once per distinct content. A test counts the hashes.
- Zip members: the central directory is read by our own code by the rules
  of §6.9; deflated members are decoded by `miniz_oxide` (ruled by the
  supervisor: the version already in the lock, MIT OR Zlib OR Apache-2.0,
  one well-tested decoder for untrusted archives), streaming, so a member
  of a wanted size is hashed in bounded memory. An archive is untrusted:
  decoding stops with an error as soon as the output passes the size the
  directory states, then the size and CRC-32 are checked; a member that
  fails is recorded as failed under the archive's stamp and not tried
  again until the archive changes (a test decodes a member that would
  expand to 258 MB from a stated 10 bytes and stops in well under half a
  second; without the bound it ran 3.5 s and failed only at the CRC). ZIP64
  is not read in v1: an archive whose directory needs it is unreadable, and
  a member whose sizes or offset hold the ZIP64 marker is listed with that
  problem, never with the marker as its size. B7's `unpack` can use the
  same module.
- Zip member names are reported as they are stored, never cleaned: the
  index keeps each name's raw bytes beside its decoded form, and a name
  that is neither ASCII nor flagged UTF-8, or that two members share, is
  listed as ambiguous and never searched. B7 must sanitise names when it
  unpacks (no `..`, no drive or leading `/`, the rules of §6.9) and must
  not guess between ambiguous members.
- Nothing is skipped silently: a folder or entry the scan cannot read
  (a folder without permission, a dangling link) is reported with its
  relative path and the error, and the scan goes on; failed hash attempts
  are counted apart from hashes. A tag of the store whose manifest cannot
  be read is left out of the store's index with a warning, and a cache
  that cannot be written is a warning; neither stops a plan. A library of
  the preferences that is not there is a warning for `library scan` and
  `recipe plan`; a folder named on the command line must be there.
- Catalogues read in v1: a CSV whose header names `path`, `size` and
  `sha256` (and optionally `crc32`, `sha1`, `md5`), and `SHA256SUMS` or
  `*.sha256` files, paths relative to the catalogue's folder. A row moves
  its file ahead in the candidates for a value it claims (or whose weaker
  hash it shares); the file is hashed all the same, and a row that leads to
  other bytes is reported as stale. The Logiqx DAT is left for later.
- The store's decoded digests are read from the layer descriptors of the
  tagged manifests (a compressed layer names its content in
  `unemu.decoded.digest` and `unemu.decoded.size`), cached per manifest in
  `cache/decoded/index.json`; no payload is opened. A store hit is trusted
  by its address for the plan, as the store verified it when it wrote it;
  the build verifies what it reads.
- Sources as built: 1 the store, 2 the download cache (none until B7
  brings downloads), 3 the bundle as a file of the input's name beside the
  recipe for `bundle = true` (a layer of the recipe artifact with B10), 4
  the libraries in order, 5 URLs listed only.
- Found instead of made, as built: the members of `unpack` steps that state
  size and SHA-256 and that an output reads are looked for first; the
  archive is looked for only when one of them is missing (the step then
  says it `opens ARCHIVE for N of M members`), and is otherwise `not
  needed: its N members were found`. The payload blobs of a locked recipe
  are looked for the same way (B8, `upc-5pw.113.18`); otherwise `fat` and
  `text` steps run when an output needs them.
- Near misses, reported and never used, at most eight per value: a file or
  member with one of the value's names and another size, one with its name
  and size and another hash, and one of its size that was hashed and holds
  other bytes.
- `recipe plan` writes nothing but the two caches. The summary line counts
  the inputs the artifact needs; inputs only an attachment uses are
  optional and do not change the exit status.

**B7. Steps and the draft build.**
Scope: `unpack` (zip), `text`, `fat` through B3; outputs handed to the
importers' shared plan-and-publish functions; the download cache and
`--fetch` with consent; `upcli recipe build` for drafts; `--keep-as`.
Depends on: B4, B6, B3 (for `fat`). Must prove: the three firmware recipes
and the three FreeDOS recipes build artifacts whose configs equal the ones
`image firmware import` and `image media import` make today; nothing is
tagged when an input is missing; no step can reach the network or the
host clock (a test build runs with both denied).
Done on 2026-11-18 (`upc-5pw.113.7`). The neutral part is in
`crates/recipe`: `steps` holds the three steps as pure functions of bytes
and keys (`unpack`, `text`, `fat`), and `build` the phases around them:
check and plan, fetch with consent, gather (every value read where the plan
found it and checked against its size and SHA-256 again), run the steps,
stage the outputs and publish them through the provider's `Publisher`. The
`ibm-pc` side is `pc_image::recipe` (the `fat` step through
`media::fat12::plan_volumes`) and `pc_image::recipe_build` (the publisher:
`media_import::plan_with` and `publish`, `firmware_import::scan` and
`publish_as`). The download is the registry crate's HTTPS transport
(`image_registry::download`). `upcli recipe build FILE [--fetch] [--library
DIR]... [--store DIR] [--config FILE] [--keep-as TAG]` exits 0 when the
artifact is tagged, 1 when it is not (findings, a missing input, a failed
download, read, step or import), 2 on a usage error (commands.md, Building
recipes). Decisions taken while building it:
- The seam of §6.12: the steps get the gathered bytes, the recipe's keys
  and the provider's volume profile, and nothing else; the fetch and every
  read of the store, the download cache, the bundle and the libraries
  happen before them. A test runs the steps twice on the same bytes, a test
  checks that the step code names no clock, network, file, environment,
  process or thread interface, and a test builds a recipe of all three
  steps twice into two stores with a fetcher that fails the test on any
  request and the library files' host times changed between the builds:
  the same manifest digest.
- `--fetch` downloads only when every input the plan reports missing has a
  URL, so a download is never made for a build that cannot complete; the
  list (inputs, sizes, SHA-256, URLs) is printed before the first request.
  A download goes to a partial file of its own,
  `cache/downloads/.partial-<sha256>-<process>-<n>`, so concurrent builds
  never share one, and is renamed to `cache/downloads/<sha256>` only with
  the pinned size and SHA-256; the plan then runs again, and the cache is
  source 2 of every later plan. Besides the per-read timeout, a whole
  download must average at least 32 KiB/s (120 s plus one second per
  32 KiB). A build starts by removing staging folders of processes that no
  longer run and partial downloads older than a day. A
  test serves the inputs from a loopback HTTP server for the `https` host
  the recipe names: only the missing input is requested, other bytes are
  not kept and nothing is tagged.
- `unpack` opens the archive only when a member an output reads is still
  missing, and then checks it whole: a member name that is absolute, holds
  a backslash in its stored bytes, names
  a drive, has an empty, `.` or `..` part, is not ASCII or flagged UTF-8,
  or is shared by two members refuses the archive; every listed member's
  stated size, hash and time must equal the archive's (its DOS date and
  time). Without the archive the tree holds the members found.
- The outputs are staged as files under their file names in the store's
  `cache/recipe-build/<process>` and handed to the importers, which take
  paths; the staging folder is removed. Medium ids are the recipe's
  (`media_import::Plan::name_media`); a firmware recipe holds one ROM, which
  the catalogue must know by its bytes, as `image firmware import` needs;
  an extra whose stated kind differs from the one the importer gives it by
  its file name is refused. Without `[license]` a package is
  user-supplied.
- `--keep-as TAG` keeps the result under another revision tag of the
  recipe's own repository, so the title id and the work id still agree;
  the recipe's own tag is refused.
- Attachments are not built: the manifest codec has no `subject` yet. The
  output names each one.
- Values are held in memory while the steps run.
- The title id (ruled 2026-11-18): the installation-set import writes the
  last component of the set's repository as the title record's id, as §6.4
  says, instead of `title`; a build hands the merged record through
  unchanged, so the two writers agree. Sets made before keep `title`; when
  one is published again with catalogue edits, its title takes the new
  repository's id and the personal data under `title` moves with it
  (`Store::publish_overlay`).
- The corpus gained the firmware recipes of the open video BIOS and the
  fixed-disk option ROM, so that the three open packages of the templates
  each have one. MEASURED: they build the packages
  `template::build_firmware` imports, manifest for manifest, and the three
  FreeDOS recipes build sets whose configs equal byte for byte those
  `template::build_media` imports, the edition archive not needed because
  its disks are found loose (`pc-image` test `recipe_build`). Ken's
  Labyrinth builds from the user's 19 files to the disk B3 pinned
  (`16992a33…`), with a licence text of the test's own in place of
  unemu-media's bundled `licenses.txt`, which the corpus lacks; AZURAMON
  builds its ten disks from the user's folder, twice to the same artifact.

**B8. The lock and pinning.**
Scope: `upcli recipe pin`; the lock directory; the checks of §3.8,
including the field-by-field comparison and the work-file drift report;
building from a lock with a release other than the one that pinned it; the
conformance harness that builds pinned recipes from a fixture library and
compares digests; `recipe show` of a pinned recipe prints the locked
metadata, which a build writes, rather than the merge with today's work file
(B5 merges the work file as it is, as a draft builds). Depends on: B7, A2, B5. Must prove: a recipe pinned by
one build of `upcli` builds the same manifest digest with another whose
manifest writer was deliberately changed; a changed work file is reported
and does not change the result.

Done on 2026-11-18 (`upc-5pw.113.8`). A2, which B8 depends on, is met: the
Zstandard encoder was frozen as profile 1 on 2026-11-17 (status.md), so a
pinned build re-encodes a `+zstd` payload with profile 1 and compares the
stored digest. The neutral code is `recipe::lock` (the lock folder, points
1 to 5 of §3.8, writing a verified lock's artifact into the store and the
tag) and `recipe::pin` (`pin_path` and the conformance harness); the build
takes the pinned path in `recipe::build::build_path`, and the `ibm-pc`
publisher tags a locked manifest with its validators
(`PcPublisher::tag_locked`). Commands: `upcli recipe pin FILE`, `upcli
recipe conformance FILE|FOLDER...`; `check`, `show` and `build` read the
lock of a pinned recipe (commands.md, Pinning recipes and Recipe
conformance). Decisions taken while building it:
- The lock is the folder `<recipe file>.lock` with `sha256/<hex>` files, as
  §7.2 says, each named by the SHA-256 of its bytes: the manifest, its
  config and every other layer of the manifest that no step made. §7.2's
  "three files" are a set's manifest, config and metadata; a firmware
  package has no metadata layer but a generated README, which is then the
  third file. The files are the bytes the store holds after the draft
  build, so the lock is deterministic. Hidden entries (a desktop's
  `.DS_Store`) are skipped; any other entry, a file over 1 MiB, a file
  whose bytes are not its name, or one the manifest does not name is an
  error.
- `recipe pin` builds and tags the draft as `recipe build` does, writes the
  lock (beside the old one, then renamed into place), then writes `digest`
  on the line after `revision` in the file's own line ends, and checks the
  file again; on failure the file is restored and the lock removed. A
  recipe that already has a `digest` is not pinned again: a changed
  artifact is a new revision. A pinned recipe whose lock is lost fails
  rule 8 (since B4); `recipe build` takes §7.2's fallback, below.
- Points 1 to 3 run in the checks (rule 8), only for a recipe with no
  other finding, so a broken lock refuses a build before any input is
  read; each disagreement is a finding at `digest`, worded `FIELD: the lock
  has X, the recipe has Y`. The fields: artifact type; platform (by
  `platform::same`); the media in order with id, label, format (the
  provider's config table, `MediaProfile::format_table`) and export file
  name; the extras in order with file name, and kind and DOS name when
  stated; the ROMs with id, role, catalogue id when stated and chip
  count, the `[license]` source archive, and the `[package]` fields the
  recipe states; and the whole title record and set labels. A title field
  differs as an error when today's value is the recipe's own, and as drift
  otherwise (the work file states it, or nothing does any more); labels
  are an error only when a label the recipe states is missing from the
  lock. A locked title with tags, cover, screenshots or launch data, which
  no recipe writes, is an error.
- Points 4 and 5 run after the steps and before anything is written: each
  output is matched with its descriptor through the locked config (media
  in order, extras by file name, a package's source archive by file name,
  ROMs and their chips in order), its decoded SHA-256 and size compared,
  then the stored ones after encoding (`codec::describe`, raw or profile
  1). Every layer must be an output or a lock file; a mismatched output is
  reported once, at the output. Only when all match are the payloads, the
  lock files and the manifest imported and the tag written last, with the
  platform's validator, which resolves and so verifies the closure once
  more. No writer of the build is called for a pinned recipe.
- A release other than the one that pinned it: the pinned path uses no
  writer of the running release, so its spelling of manifests, configs and
  metadata cannot reach the result. MEASURED (`pc-image` test
  `recipe_lock`, `a_pinned_recipe_builds_its_digest_with_another_builds_manifest_writer`):
  a publisher whose manifest writer puts an extra annotation first, out of
  sorted order, and breaks the lines, gets another digest for its draft of
  the recipe and the pinned digest, the lock's manifest byte for byte, for
  the pinned recipe. Tags follow decision-38: the pinned build tags the
  recipe's `<release>-rN` (the store moves the release alias with it), a
  tag that names another image is refused because an `rN` never moves, and
  `--keep-as TAG` keeps a differing result under `TAG` only.
- The work-file drift report: `check` prints `  drift: FIELD: the lock has
  X, NAME.work.toml has Y; …` after the file's line (not a finding), the
  build prints the same per field before its last line, and `show` prints
  the lock's record with a `# locked:` line and one `# drift:` line per
  field. A pinned build publishes the lock's metadata whatever the work
  file says (test `a_changed_work_file_is_reported_and_does_not_change_the_result`;
  the same recipe as a draft takes the edit and gets another digest).
- `recipe show` of a pinned recipe: the merged record's identity with the
  lock's title record and labels, every value marked `Origin::Lock`, so no
  `# from` comment is printed (the lock does not say which file a value
  came from).
- The conformance harness: `upcli recipe conformance` builds every pinned
  recipe of the files and folders named into a scratch store under the
  store's `cache/recipe-conformance/`, from the libraries only, and
  compares the digests; drafts are listed and skipped, work files ignored;
  exit 1 when one differs or cannot be built. The corpus's six open
  recipes are pinned, their locks beside them (the three FreeDOS floppy
  sets and the three firmware packages, whose digests are the template
  packages'. The template builder now uses these same shipped recipes
  from `crates/pc-image/data/recipes/`, so the FreeDOS set digests also
  equal the template sets', including their locked metadata). The test
  `the_pinned_corpus_builds_the_digests_it_pins` runs the harness on them
  from `vendor/` and `vendor/freedos`; unemu-media runs `upcli recipe
  conformance RECIPES --library DIR` against its own library.
- Fix round after review: the licence marks are part of point 3, because
  the push guard trusts them. Without `[license]` no descriptor is open,
  links terms or a source, or is listed in the manifest's licences; with
  it, an open payload layer is open under exactly the recipe's expression
  and an open layer only the lock holds under the generated-text licence
  `CC0-1.0` (`lock::GENERATED_LICENSE`, kept equal to the provider's by a
  test), every terms or source link names the decoded digest of the
  recipe's own terms or source extra (found through the locked config), a
  set's open payloads carry those links and a package's open ROMs its
  source link, and the manifest's licence list is its open layers'. A lock
  may be more restrictive than its recipe, never less. A package's README,
  which states the marks, is regenerated from the lock's config and marks
  through the provider (`FirmwareProfile::readme`) and compared byte for
  byte. MEASURED: the reviewer's forgery, a recipe without `[license]`
  whose lock was edited to mark every layer open and re-hashed, is refused
  by rule 8 for a set and for a package (`recipe_lock` tests), and nothing
  is tagged.
- Also from the review: a title field that nothing states any more is an
  error, drift only when today's value is the work file's; an extra's kind
  and DOS name, a package's fields and a ROM's catalogue id are compared
  with the importer's default when the recipe leaves them out (the kind by
  `MediaProfile::extra_kind`, the id by the catalogue entry its bytes
  belong to); a package's boards and machine classes, and a chip's
  position, part number, lane and offset, when stated; media formats by name
  (`MediaProfile::format_name`), not by the config's table. A build compares
  the lock again after the steps and refuses a disagreement found then. A
  pin that fails after its build puts back a lock the draft had beside it;
  the tag the build wrote stays, as it names what `recipe build` makes of
  the draft. `recipe conformance` that finds no pinned recipe exits 1.
- Second fix round: which layers are generated text is decided by the
  manifest's structure (its config, its metadata layer, the layer a
  package's config names as its README), never by which files a lock
  holds, and a lock that holds any other file, a payload above all, is
  refused when it is opened: a lock holds no payload byte. MEASURED: a
  forged lock holding an open package's ROM and source archive relabelled
  `CC0-1.0`, without the manifest's licence list, is refused and nothing
  is tagged (`recipe_lock`, `a_lock_that_holds_a_payload_is_refused`). A
  package's provenance and each ROM's required devices, which the importer
  copies from the firmware catalogue, are compared with the catalogue
  entry, so a lock cannot put text of its own into the generated config or
  README; a package without a README is refused.
- Follow-ups done on 2026-11-19 (`upc-5pw.113.18`):
  - §3.4's "found instead of made" for the payload blobs of a locked
    recipe (B6 left it to B8). Before the plan, a pinned build reads the
    lock's payloads for the outputs a step makes (`lock::payloads`: media
    in order, extras by file name, ROMs and chips in order) and looks each
    up in the store index, the download cache and the libraries, in that
    order (no bundle: an output has no name beside the recipe), before
    planning the steps
    (`plan::plan_pinned`); the first match wins. A found output is read
    and checked by size and SHA-256 like an input, and is matched with the
    lock again after the steps; the steps and inputs only it needed do not
    run and are not read (`not needed: the outputs made from it were
    found`, a step `not needed: the outputs it makes were found`). An
    output that refers to an input, or to a member whose size and hash the
    recipe states, is not looked for again: its own search is the same
    one. `recipe plan` lists `outputs found instead of made:`, and `recipe
    build` prints `  KIND ID (FILE): found WHERE, not made`. The
    conformance harness does not look outputs up: it proves the steps
    still make the pinned bytes. A store index the build cannot read
    through is no longer searched. MEASURED (`pc-image` test
    `recipe_lock`): with the outputs in the store or in a library and the
    inputs gone, the pinned recipe builds its digest; files with the
    outputs' names and sizes and other bytes are not taken.
  - §7.2's lost lock. A pinned recipe with nothing at its lock's path is
    built as a draft into a scratch store with this build's writers
    (`cache/recipe-lost-lock/`, removed after). The same digest: the files
    it wrote stand in for the lock (`lock::rebuilt`, points 1 and 2 as for
    an opened lock), and the build goes on as a pinned one and prints a
    line `lock: lost; this build's writers made the pinned digest DIGEST`.
    Another digest, from the writers or from a step that now writes other
    bytes: nothing is tagged, `--keep-as` included, and the error (`this
    build makes X, the recipe pins Y`) says the recipe needs its lock.
    `recipe plan` plans such a recipe as the build does and says the lock
    is lost; `recipe check` still reports rule 8; conformance rebuilds it
    as the build does. MEASURED
    (`recipe_lock`, `a_lost_lock_is_rebuilt_with_this_builds_writers`):
    this build rebuilds the pinned digest, byte for byte the lock's
    manifest; a build with another manifest writer is refused and tags
    nothing; `conformance_rebuilds_a_lost_lock` likewise. The firmware arm
    (`a_pinned_firmware_recipe_finds_its_rom_instead_of_unpacking_it`): a
    package whose ROM an `unpack` step takes from a zip builds from the ROM
    file alone.

**B9. `scaffold` and `inputs`.**
Scope: `upcli recipe scaffold` (input tables, member lists with sizes,
hashes and times, file sets) and `upcli recipe inputs` (DAT, CSV,
checksums). Depends on: B4. Must prove: scaffolding LABFULL.ZIP and the
AZURAMON folder gives the corpus's tables byte for byte.
Done on 2026-11-19 (`upc-5pw.113.9`): `recipe::scaffold` and
`recipe::inputs`, the commands `upcli recipe scaffold FILE|FOLDER...` and
`upcli recipe inputs FILE --format dat|csv|sha256sums` (commands.md,
Scaffolding recipes and listing their inputs). Readings taken:
- An archive is a file whose name ends in `.zip`; any other file is a
  plain input. The archive gets an `unpack` step with `complete = true`
  and every file member in central-directory order, with the DOS time
  the directory states (§6.9: the time an `unpack` compares; extended
  timestamp fields are not read), none for a member without a valid one,
  a day its month lacks included (`steps::dos_time`, which `unpack`
  shares).
  What an `unpack` step refuses when it opens an archive is refused here,
  with the same messages (`steps::member_problem`).
- A folder is a file set in the order of `order = "name"`: in every folder
  its files, then its folders, each group by the 8.3 name's 11-byte stored
  field, as `media::fat12` sorts (a test builds the same paths with it and
  reads the order back); names that are not valid 8.3 names follow, by
  bytes. The corpus's AZURAMON order is this order. No times (§6.12),
  hidden entries skipped, links followed. Rule 9 is applied before any
  file is hashed: 100,000 entries over all folders named, 4096 operands,
  64 archives.
- Ids: the name in lower case, a file's last extension dropped, other
  characters than letters, digits, `_` and `-` as `-`; the step is
  `ID-files`. The corpus's own ids (`game`, `files`) are the author's
  names; the proof compares with them renamed. The corpus's `urls` of
  LABFULL.ZIP are the author's addition and are not compared. No
  placeholder is written.
- `inputs` lists one folder: a file input at its `name`, a file-set entry
  at the set's `name` and its path; bundled inputs and `unpack` members
  are left out. Two inputs at one path with the same bytes are one row;
  with other bytes, or a file where another input needs a folder, the
  command refuses (exit 1) and names the path and both inputs. The DAT has one game named after the artifact and a
  `sha256` attribute beside the DTD's `crc`, `md5` and `sha1`.
MEASURED (`cli-pc` test `recipe_scaffold`): a synthetic zip, folder and
file scaffold to the expected text byte for byte, which parses back and
passes `recipe check` with the outputs added; `recipe inputs` gives the
three forms byte for byte, and `sha256sum -c` (coreutils 9.11) accepts
the checksum file against the files. The limit is proved on a generated
listing of 100,001 entries, refused before a file is read
(`scaffold::tests`). The B9 proof, MEASURED on the maintainer's own
files (`cli-pc` tests run with `UNEMUPC_REQUIRE_USER_ROMS=1`):
`user_rom_scaffold_of_labfull_zip_gives_the_corpus_tables` scaffolds
LABFULL.ZIP to the corpus's input table (without the author's `urls`) and
its 19 members with their times, byte for byte, and
`user_rom_scaffold_of_the_azuramon_folder_gives_the_corpus_table` the
AZURAMON folder to the corpus's 561 entries, byte for byte; ids are the
corpus's own names, `game` and `files`, for scaffold's `azuramon` and
`labfull-files`.

**B10. Recipe artifacts in the registry.**
Scope: the two artifact types and the config type; push and pull; the
bundle (work file, bundled inputs, lock); the recipe guard (nothing but
those; `disclosable`; a tag replaced only by a recipe that pins the same
output digest); `unemu.build.recipe` on local tags; the decision-30 rows
for recipe versions, steps and formats. Depends on: B8, A1. Must prove: a
recipe pulled into an empty store plans and builds with no other file;
each guard refusal has a test; a registry row can be judged from manifest
and config alone.
Done on 2026-11-19 (`upc-5pw.113.10`): `recipe::artifact` (the bundle,
the codec, the guard and the row's plan), `recipe::compat` (the rows),
`image_registry::recipe` (push, pull, row), the types in
`image_store::media_type` and their rows in `image_store::compat`, the
record `image_store::BUILD_RECIPE`; the commands `upcli recipe push FILE
REF`, `recipe pull REF [--to DIR]` and `recipe plan REF` (commands.md,
Recipes on a registry). MEASURED (`cli-pc` test `recipe_registry`, the
mock registry, no network): a pinned recipe with a work file, a bundled
text and a `text` step, pushed and pulled into an empty folder, plans and
builds into an empty store from a library holding only its image, to the
pinned digest, and the tag records the recipe's digest; every refusal of
the guard below has a test, and an allowed same-digest replacement moves
the tag; a row costs two requests, the manifest and the config; a recipe
rated 0.0.4 by a newer release's version, step or output row is refused
by pull and judged by a row from its manifest alone, with no blob
requested. No pinned digest moved: the record lives in `index.json`.
Readings taken:
- Layer types, which §7.1 does not name: the work file
  `application/vnd.unemu.recipe.work.v1+toml`, a bundled input
  `application/vnd.unemu.recipe.input.v1`, a lock file
  `application/vnd.unemu.recipe.lock.v1`, each at most 1 MiB, in that
  order (work file, inputs in recipe order, lock files by name). The
  config descriptor carries `org.opencontainers.image.title` too, the
  recipe's file name, and a lock file's title is
  `NAME.media.toml.lock/sha256/<hex>`, so the titles are the folder the
  recipe was pinned in. A recipe's file name ends in `.media.toml` or
  `.firmware.toml` by its kind.
- Pull lays the artifact out in a folder (`--to DIR`, the current folder
  by default), not in the store: a pulled recipe is no image, and a local
  tag of one would be listed as one. The recipe then plans and builds as
  any file does; `recipe build REF` is not added, pull and build the file.
  The parts are checked in a scratch folder before anything is written.
- `unemu.min-app-version` is the newest of the recipe version's row, the
  rows of its step kinds (an exhaustive match over the model's steps, a
  test that every verb of the specification and the parser's version has
  one), the rows of the artifact's own media types, and the marker of the
  locked output's manifest. Formats come through the last: the importer
  stamped the output with the provider's rows of its media formats, roles
  and catalogue entries when the recipe was pinned. All rows are 0.0.3,
  so every recipe artifact carries `0.0.3`.
- `disclosable` is judged on file inputs, file-set entries and `unpack`
  members under 256 bytes in a recipe without `[license]`; members carry
  the key too (decided by the maintainer, 2026-11-19; §6.9, §6.13).
- Bundled parts never share bytes: a recipe whose bundled inputs are
  identical, or one equal to its work file, is a finding of rule 3, as an
  artifact cannot name one blob twice.
- "Text" is UTF-8 without a NUL byte.
- A tag naming a recipe of the same output digest is replaced without
  `--move`; one naming anything else is refused, `--move` or not. The
  registry rules of the push guard apply with every part counted open;
  the store's own marks are not consulted, as a lock of a private output
  is our own text (§6.6).
- The guard also checks the place of §7.3: `NAMESPACE/recipes/` and the
  output's path after its first component, the output's tag, any
  namespace; and that the annotations say what the recipe says.
- `unemu.build.recipe` is recorded on a tag the build creates, in a
  second index write after the tag (a failure is a warning; the tag
  stands); a tag that was there keeps what it had, a pin records the
  pinned text, and an alias that moved with the revision is not recorded.

**B11. Export.**
Scope: `upcli image export`, `--files` with the FAT reader promoted from
test code, the per-input "kept" report of `recipe build`, and the layout
documented with a script that uses no `upcli`. Related and distinct:
`upc-5pw.116` saves and loads whole artifacts, selections and repositories
as OCI Image Layouts; export unpacks one artifact into plain files.
Depends on: A3. Must prove: the script and the command write the same files; an
exported AZURAMON tree equals the input files byte for byte.
Done on 2026-11-19 (`upc-5pw.113.11`): `media::fat12::reader` (the
reader, product code now, with `Volume::tree`), `cli_core::image::export`
and `upcli image export NAME:TAG --to DIR [--files] [--store DIR]`, the
kept report of `recipe::build`, and `scripts/export-layout.sh`
(commands.md, Exporting an image). Readings taken:
- The layout, version 1: every layer under its decoded name, the config
  and manifest as stored, `EXPORT-VERSION` holding `1` and a line feed,
  and `SHA256SUMS` in `sha256sum` text form (two spaces), one line per
  other file by the bytes of the names, written last. The `--files` tree
  is not listed: the images that hold it are. A machine image is refused.
- `--files` reads every diskette of a set as a FAT12 volume, in the set's
  order, and merges the trees into the folder `files`: the set has no
  record of which disks a split made, and its disks are one set in
  installer order. A folder several volumes hold is one folder with the
  first one's time; a file several hold must have the same bytes; other
  bytes or a file against a folder refuse `--files` (the plain export
  still works). A medium that does not open as a FAT12 volume (an optical
  image, another format) is reported `not read` and left out; a damaged
  FAT12 volume refuses.
- Names: the long name when the entries before the short one are complete
  and carry its checksum, else the 8.3 name as stored, its stem or
  extension in lower case when byte 12's case bits (08h, 10h) say so, as
  Windows NT, Linux and mtools write them; a short name that is not ASCII,
  or a name no host takes (`image_store::export_name::problem`), refuses.
  Folders that differ in case only are one folder, as on the hosts that
  ignore case. Names are compared by bytes, not Unicode normalization (no
  table in the program): two names that differ only in composition meet
  on macOS while the export writes, and it then removes what it wrote.
- Bounds of the reader: a name of at most 255 bytes, a folder path of
  8.3 names of at most 64 characters with its leading backslash (the
  builder's and DOS's limit: at most 32 levels), a path of host names of
  at most 260 characters (MAX_PATH); one path buffer for the walk. A
  crafted image of thousands of nested folders is refused at once, where
  the first reader took gigabytes (found in review). FF0h to FF5h are
  cluster numbers in a volume of more than 4078 clusters.
- Times: a file's and a folder's modification time is its entry's
  last-write time read as UTC, the same instant on every host; mtools
  gives the same with `TZ=UTC0`. An entry without a valid time leaves the
  host's. Attributes are not set on the host; the volume label is not a
  file.
- Overwriting: every name is checked before anything is written; a file
  with the same bytes is kept, other bytes, a folder for a file, a file
  for a folder or a symbolic link refuse. Files go through a temporary
  name and a rename.
- The kept report names the exported files that keep each input, by the
  export file names: `kept as stored in F`, `kept as files in F, G`,
  `kept as text with DOS line ends in F`; an archive an `unpack` step
  opened reads `members kept as …` for each way through the `unpack` step
  and then, unless the archive is also kept as stored (an extra from it),
  `not kept: the archive itself`, with `, N members unused` when the
  recipe left members out.
- The script reads any OCI Image Layout (a store's `images` folder, an
  `oras copy --to-oci-layout` folder) by `ref.name` or digest, checks
  every blob and decoded layer and the names as upcli does before writing,
  and uses mtools for `--files` under a UTF-8 locale and
  `MTOOLSRC=/dev/null`. It writes only into an empty or absent folder. zstd
  joined the devShell for it; the comparison fails without a tool when the
  fixtures are required.
MEASURED (`cli-pc` test `image_export`): command and script write the same
files, bytes and the unpacked files' and folders' times, for a synthetic
two-volume split set with an extra and a blank image, a synthetic firmware
package, the FreeDOS 1.4 1.44M set (seven disks with long names) and a
disk mtools wrote with case bits and a long name; `sha256sum -c` accepts
the list; each refusal leaves the folder as it was.
The reader round-trips listed order, entry times, the label, the serial
and a split, and refuses a looping chain, a chain outside the data area, a
free or bad cluster, a short chain, cross-linked clusters, a folder that
contains itself, a truncated image and FAT16 sizes, with no panic over
every byte of the boot sector, a FAT sector and two directory sectors
changed (`media` tests `fat12::reader`). The kept report's lines are
pinned (`pc-image` test
`the_build_reports_how_and_where_each_input_is_kept`). With the
maintainer's files (`UNEMUPC_REQUIRE_USER_ROMS=1`),
`user_rom_azuramoon_exported_with_files_is_the_users_folder` builds
Azuramoon from the corpus recipe and the user's folder, exports it with
`--files`, and the tree is the folder, 561 files byte for byte.

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
