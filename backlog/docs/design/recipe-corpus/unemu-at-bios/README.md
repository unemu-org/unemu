# The unemu AT BIOS, an open firmware package

`unemu/firmware/ibm-pc/unemu-at-bios-1.0.2-unemu.3:r1`. Open (GPL-3.0-or-later,
with its source archive). Real data from unemu-media
`recipes/at-bios-1.0.2-unemu.3/`. ASSUMED: `[package.fits]`, copied from the
example in machine-images §5.3.

## What it exercises

- A Firmwarefile with no steps: two release assets, taken unmodified.
- A licence statement for firmware: the expression and the source archive;
  no terms text.
- The repository is the catalogue id, so the tag is `r1` with no release.

## Findings

1. **The whole of `build.sh` and `fetch_release_assets` becomes two
   `[input]` tables.**
2. **The ROM's hash was stated twice** in the first draft, on the input and
   on `[[rom]]`. The specification now takes a single-blob ROM's size and
   hash from its value.
3. **The description, vendor, version and labels duplicate the firmware
   catalogue's and `pc_image::template::FIRMWARE`'s.** Today three places
   must agree word for word. With a lock the recipe is the one source, and
   the template build can read the package instead of repeating it.
4. The licence must equal the one the firmware catalogue records. A recipe
   cannot check that; the build does.
