# Azuramoon: Chapter I, a private game as a folder tree

`private/media/ibm-pc/azuramoon:2026-r1`. Private: no licence statement, no
URL. The recipe holds 561 names, sizes and hashes and our own description;
the files come from the user's library. Sizes and hashes are MEASURED on one
copy of the folder.

## What it exercises

- A **file-set input**: 561 files in 7 folders and the root, 13,352,714
  bytes.
- The same content at many paths: the 561 files have 204 distinct hashes.
  Each is found once and placed wherever the list says.
- `fat` with `volumes = 10`: one tree split over ten 1.44M disks.
- Needs taken from the game's own README: a 286 or better, VGA, 640 KB, a
  Sound Blaster if there is one.

## Findings

1. **The recipe is 72 KiB**, all of it the file list. One line per file is
   the floor in any syntax.
2. **Host file times cannot be used.** The copy's times are those of a
   download in 2026. The recipe gives every entry one stated time. ASSUMED:
   the date chosen.
3. **`volumes = 10` is ASSUMED.** The number is from the brief; no builder
   has split this tree. The step refuses any other count and names the one
   it needs, so a wrong guess costs one run.
4. **Which file is on which disk is not in the recipe.** The split
   algorithm decides, and it is part of what the recipe version freezes. The
   media labels are therefore plain (`Disk 3 of 10`).
5. **No version, edition or region**: the release is the year. ASSUMED: the
   year.
6. **`DOS 3.3+` has no place in `[needs]`** in v1 (operating-system needs
   are a later feature).
7. **`sound = ["sb", "none"]`** is how "optional Sound Blaster" is said.
8. A user who keeps the game as a zip is served by the library index, which
   looks inside zip archives; the recipe does not change.
9. **The smallest files are 768 bytes**, above the 256-byte limit under
   which a published hash would give a file away; no entry needs
   `disclosable`.
10. **Export**: `image export --files` gives the folder back, merged from the
   ten disks, with the recipe's time on every file.
