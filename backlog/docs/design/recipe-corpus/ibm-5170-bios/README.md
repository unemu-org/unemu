# IBM 5170 BIOS 1985-06-10, from the user's chip dumps

`private/firmware/ibm-pc/ibm-5170-bios-1985-06-10:r1`. Private. Sizes, hashes,
part numbers, lanes and the assembled ROM's hash are real, from the firmware
catalogue (`crates/assembly/src/firmware.rs`). ASSUMED: the MAME set and ROM
names used as the canonical names and ids.

## What it exercises

- A ROM in `parts`: two 32 KiB chips, even and odd bytes.
- Inputs found only in the user's library, by hash; 32,768 bytes is a size
  many files have, so the scan hashes many small candidates.
- No licence statement: the package is user-supplied.

## Findings

1. **This is what Import ROM folder… already does**, with the catalogue
   compiled into the program. A Firmwarefile is the same match written as
   data.
2. **One catalogue entry, two layouts.** The BIOS of 1984-01-10 exists as
   two 32 KiB chips or four 16 KiB chips. A package stores chips, so the
   two layouts are two artifacts; under today's tag (`:1`) they collide.
   They need a release part: `2x32k-r1`, `4x16k-r1`. The 1985-06-10 BIOS
   has one layout and needs none.
3. **A later `interleave` step** could store the assembled 64 KiB ROM and
   make one artifact from either layout. Not in v1; decision-27 stores the
   chips.
4. **A dump kept inside a MAME zip** is found by the library index, which
   reads zip directories.
