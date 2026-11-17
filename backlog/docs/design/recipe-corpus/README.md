# Recipe corpus

Status: **drafts** for the design in [`../recipes.md`](../recipes.md)
(`upc-5pw.113`), 2026-11-17. Nine recipes, each in the chosen syntax (TOML
1.0, `*.toml`) and in the strongest alternative (an HCL literal subset,
`*.hcl`). They exist to test the syntax and the model against real cases
before anything freezes (recipes.md §14). No builder exists, so every recipe
is a draft without a `digest`, and none has been built.

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
| [`ibm-5170-bios`](ibm-5170-bios/README.md) | IBM 5170 BIOS from chip dumps | no | A ROM in parts, MAME ids |
| [`contra-nes`](contra-nes/README.md) | Contra and Probotector, NES | no | A future platform: regions, a work id |
| [`amiga-adf`](amiga-adf/README.md) | An Amiga disk image | no | A future platform: an unmodified medium, no release |

## Second round: work files, TOML only (2026-11-17)

The maintainer chose TOML after reading this corpus. From here on only the
TOML files change; the `.hcl` files are kept as they were, as the evidence
for the fallback, and no longer match the TOML of the three folders below.

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
| `kenslab-2.01/kenslab-1.0-shareware` (new) | — | 35 |
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
    AmigaOS. New platforms need schema 2 or a longer list.
12. **The ROM's hash was written twice** in the open firmware recipe (input
    and `[[rom]]`). Fixed in the specification: a `[[rom]]` with `from`
    takes size and hash from its value; only a ROM in `parts` states them.
