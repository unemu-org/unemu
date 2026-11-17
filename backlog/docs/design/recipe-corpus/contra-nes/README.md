# Contra and Probotector on the NES (a future platform)

`private/media/nes/contra:usa-r1`, `:japan-r1`, `:europe-r1`. Private. The NES
is not a platform of the family yet: the id `nes`, the media format `ines`
and the `tv_system` need are ASSUMED, and so are the sizes and years. The
hashes are PLACEHOLDERS: no dump was measured. The No-Intro names are the
ones that catalogue uses.

Second round: the folder has a work file, `contra.work.toml`, and a fourth
recipe, Contra (USA) (Rev 1). ILLUSTRATIVE: whether that revision exists
was not checked; it is there to show a revision as a release part
(`usa-rev1`). The `.hcl` files are the first round's and have no work file.

## What it exercises

- A **work file**: title, series, developers, publishers, genres, players,
  the label `game`, a synopsis and the work id, written once. Each recipe
  keeps its year, region, description, `tv_system`, input and medium.
- A regional title: the European recipe states `title = "Probotector"`,
  which wins over the work's `Contra`.

- Regional variants of one work: three recipes, one repository, three
  releases, one work id in `[ids]`.
- A release with no version: the region is the release.
- A title that differs by region (Probotector) under the work's repository.
- A dump id from a catalogue, and a weaker hash (`sha1`) as a hint.

## Findings

1. **The engine's part of the schema did not change** for a new platform.
   Every new word is the provider's.
2. **The header problem.** No-Intro's classic hashes are of the cartridge
   without its 16-byte iNES header; most files have the header. A recipe
   that names the file as stored cannot use those hashes even as hints, and
   two files with the same cartridge and different headers are different
   inputs. This needs provider input forms (recipes.md §13): the input is
   the headerless content, found through a transform.
3. **A cartridge revision** is a release part: `usa-rev1`. Its recipe
   differs from the USA one in the release, the input and the label only.
7. **The work id moved to the work file**, so the four recipes cannot
   disagree about which work they are.
8. **Publishers are in the work file** because they do not vary here. A
   region with another publisher states its own list, which replaces the
   work's.
4. **`software_platform`** has no NES in metadata schema 1's list.
5. **`tv_system`** shows a need that is not a PC-style feature: the European
   cartridge wants a PAL machine.
6. Whether the three regions should be one repository (as here) or three is
   a naming rule for unemu-media, not for the format.
