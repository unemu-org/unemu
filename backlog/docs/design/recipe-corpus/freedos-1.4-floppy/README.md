# FreeDOS 1.4 Floppy Edition, three formats

`unemu/media/ibm-pc/freedos:1.4-r1`, `:1.4-1.2m-r1`, `:1.4-720k-r1`. Open.
Source of the data: unemu-media `recipes/freedos-1.4-floppy/` (`media.toml`,
`build.sh`), real throughout except the two points marked ASSUMED.

## What it exercises

- Three artifacts from one upstream archive, one recipe file each (one file,
  one artifact, one tag).
- `unpack`: the disks of one format out of a zip that holds three formats.
- `text`: the licence index, our own text, with DOS line ends.
- An open statement with an SPDX expression of fifteen licences, a terms
  text and a corresponding-source archive.
- Two URLs for one input, tried in order.

Second round: the recipes share `freedos.work.toml`, now shipped beside
them in `crates/pc-image/data/recipes/freedos-1.4-floppy/`, with the
title, people, genre, labels, synopsis and needs. Each recipe is 9 lines
shorter. The numbers under Sharing are the first round's.

## Sharing

The three files share by **content**, not by an include: all three name the
same edition archive, licence index and source archive by hash, so the
archive is fetched once and the two extras are one blob each in the store
and on the registry. As text, 86 of the 116 lines of the 1.44M recipe occur
in all three (MEASURED; table headers that repeat are counted). The lines
that differ are the release, the description (the disk counts are in its
prose), the member list and the media.

## Findings

1. **The source archive cannot be built by v1.** `build.sh` makes it from 69
   package zips copied out of an ISO inside a zip, four git trees at pinned
   commits and a deterministic tar: 356 lines, most of them for this one
   file. The recipe takes the archive as a prepared input by hash. ASSUMED:
   the URL of its release asset, which does not exist yet.
2. **A work file, no include.** After the work file, 77 of the 107 lines
   of the 1.44M recipe still occur in all three (MEASURED): three `[input]`
   tables, `[license]`, the `text` step, two `[[extra]]` tables and the
   headers and keys every medium has. They are the build, and the build is
   not shared by file (recipes.md §6.16).
3. **`complete` is not used here**, because each recipe takes a third of the
   archive. The check `build.sh` makes on the member list adds nothing once
   the archive's hash is pinned.
4. **The 1.44M format is not in its release** (`1.4`, not `1.4-1.44m`),
   because it is the edition's own format. That is a naming convention, not
   something the checker can know.
5. **`[needs]` is ASSUMED**, read off the description's prose.
6. The bundled input `licenses.txt` is unemu-media's file; it is not copied
   into this corpus.
