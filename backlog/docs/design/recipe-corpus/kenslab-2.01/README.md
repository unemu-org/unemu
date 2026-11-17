# Ken's Labyrinth 2.01

`unemu/media/ibm-pc/kenslab:2.01-r1`. Open (freeware under the author's own
terms). Source of the data: unemu-media commit `69f6d65`,
`recipes/kenslab-2.01/` (`media.toml`, `build.sh`), real throughout except
`[needs]`. The members' sizes and times were measured from the pinned
archive for the earlier design draft and are taken from it; their sizes add
up to the 1,315,635 bytes `build.sh` states.

Second round: the folder has a work file, `kenslab.work.toml`, and a second
release, the 1.0 shareware episode, whose data is all PLACEHOLDER (the
archive was not obtained). The `.hcl` file is the first round's.

## What it exercises

- A **work file** shared by a shareware and a full release: title, people,
  genre, synopsis, needs, and the labels `game` and
  `first-person-shooter`. Each release adds its own label (`freeware`,
  `shareware`): labels are a union.
- `[needs]` in the work file: both releases need the same machine, and
  neither recipe has a `[needs]` table left.

- `unpack` with `complete = true`: the licence asks that no file of the
  archive be left out, and the recipe says so in a form the checker holds
  it to.
- `fat` with `order = "listed"`: the disk has the archive's order.
- The archive's file times, written out per member: a zip entry's time and
  a FAT entry's time have the same form, so nothing is converted. Nothing
  but bytes is read from the archive at build time.
- Members with size and hash, so the 19 files can be found loose when the
  zip is not at hand.
- A chosen volume label, serial and label time.
- An extra taken from a tree (`from = "files/README.TXT"`), which is also
  the licence's terms text.

## Findings

1. **The image will not equal the one mtools built** on the unemu-media
   branch. The set was never published, so it is simply pinned anew.
2. **The FAT builder lacks four things** this recipe needs: listed order,
   entry times, a chosen serial and a label. They are named in recipes.md
   §6.9.
3. **Hexadecimal serial**: `0x20080703` in TOML reads as the date it encodes;
   HCL has no hexadecimal literal and writes `537396995`.
4. **`time` does two jobs**: the default for entries without a time and the
   time of the volume label. Here every file has the archive's time, so only
   the label uses it. Two keys would be clearer if a recipe ever needs them
   to differ; none in the corpus does.
5. **Writing the times out costs 26 characters a member** and a `scaffold`
   command to produce them. The first draft read them from the archive at
   build time; that breaks as soon as a member is found outside the
   archive, so it was dropped.
6. `[needs]` is ASSUMED.
7. **The licence stays in each recipe.** The full version is freeware under
   the author's terms, the shareware episode under the publisher's; a work
   file may not hold `[license]`.
