# The unemu VGA BIOS, an open firmware package

`unemu/firmware/ibm-pc/vgabios-0.9d-unemu.1-std:r1`. Open
(LGPL-2.1-or-later, with its source archive). Added with the first builds
(recipes.md §19.2, B7) from the firmware catalogue and
`pc_image::template::FIRMWARE`: the package the open templates bind. TOML
only, no HCL copy.

## What it exercises

- A Firmwarefile with no steps, as `unemu-at-bios`: the ROM and the
  source archive taken unmodified.
- An input without a URL: `vendor/vgabios/fetch.sh` builds both files from
  the fork's repository, so a build finds them in a library or not at all.

## Findings

1. MEASURED: the build gives the package the template build imports, the
   same manifest digest.
