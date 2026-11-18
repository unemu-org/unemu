# The XTIDE Universal BIOS, an open firmware package

`unemu/firmware/ibm-pc/xub-r638-xtp-ata1f0:r1`. Open (GPL-2.0-only, with
its source archive, the r638 Subversion trunk). Added with the first builds
(recipes.md §19.2, B7) from the firmware catalogue and
`pc_image::template::FIRMWARE`: the package the open templates bind. TOML
only, no HCL copy.

## What it exercises

- A Firmwarefile with no steps for an option ROM.
- Inputs without a URL: `vendor/xtide-universal-bios/fetch.sh` configures
  and pads the project's download and builds the source archive, so the
  bytes a recipe pins are not the bytes any server serves.

## Findings

1. MEASURED: the build gives the package the template build imports, the
   same manifest digest.
2. A step that configures and pads the download would let the recipe take
   the project's own `ide_xtp.bin`; v1 has no such step, so the recipe pins
   the configured ROM.
