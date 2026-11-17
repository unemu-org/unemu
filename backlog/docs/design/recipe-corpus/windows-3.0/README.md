# Microsoft Windows 3.0, installation disks required, documentation optional

`private/media/ibm-pc/windows:3.0-1.2m-r1`, built by a recipe that would be
published as `unemu/recipes/media/ibm-pc/windows:3.0-1.2m-r1`. Private. **Nothing here was
measured**: no copy of the disks was at hand. The number of disks, their
names, the hashes (zeros ending in the disk number), the catalogue ids, the
manual's size and the Wikidata item are PLACEHOLDERS. Only the size of a
1.2M image (1,228,800 bytes) is real. The recipe shows the shape of a
private recipe, not the data of this one.

## What it exercises

- Five inputs with no source but the user's library, each with a database
  id.
- Media taken straight from inputs: no step.
- An **attachment**: the printed documentation as an optional part. The
  set's digest does not depend on it; a user who has the scan gets a second
  small artifact that refers to the set.
- A work id in `[ids]`.

## Findings

1. **Two tables per disk.** Each unmodified disk is an `[input]` and a
   `[[medium]]`: ten tables for five disks. See the corpus README, point 3.
2. **The input `users-guide` is optional without saying so**: only the
   attachment uses it. `recipe check` prints the split.
3. **The same product on other disk formats is another release**
   (`3.0-720k`), and Windows 3.00a is another release again. Whether two
   dumps are one release or two is the author's judgement; the format does
   not decide it.
4. **`ids` as `{ db, set, name }`** fits TOSEC, No-Intro, Redump and MAME in
   the corpus, but the values here are invented and prove nothing.
5. **The DOS version Windows needs** has no place in `[needs]` in v1.
6. **A scan of a manual is proprietary too.** The attachment is
   user-supplied like the disks; "optional" says nothing about rights.
