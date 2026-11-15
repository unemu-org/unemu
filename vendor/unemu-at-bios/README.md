# Open AT BIOS fixture

`fetch.sh` builds `bios-unemu-at.bin`, the system BIOS of the open AT
machines, from **unemu-at-bios**, the unemu fork of Sergey Kiselev's
[8088_bios](https://github.com/skiselev/8088_bios). The fork is the
`unemu-at-bios` repository in the same forge organization as this repository
(`unemu-org/unemu-at-bios`). It is pinned at release tag **`v1.0.2-unemu.2`**,
source revision `a2cf362d05ee6cf78ef03cd3cc383ba3a19c40bd`:

| File | Flake output | Size | SHA-256 |
|---|---|---|---|
| `bios-unemu-at.bin` | `#unemu-at` | 32,768 bytes | `f8ecd4a2ad4906eb94a7595ede755019f545f40111725819eeb80e423adc8db6` |

The image is the fork's `MACHINE_UNEMU_AT` configuration: an IBM PC/AT
compatible 80286 board with two 8259s, two 8237s, an 8042 and an MC146818.
It is the raw 32 KiB image that maps read-only at physical F8000h–FFFFFh
(F000:8000); the CPU reset entry F000:FFF0 is offset 7FF0h, a far jump into
POST. Like the Xi 8088 build it starts from, it has floppy INT 13h only;
fixed disks come from the XTIDE Universal BIOS option ROM.

## What it is for

It replaces the Kiselev Xi 8088 AT-reset image
(`vendor/kiselev-bios/bios-xi8088-atreset.bin`) on the open AT machines
(`machines/at-games-open.toml`, `machines/at-ide.toml`, the `open-at`
templates), so public machine images never depend on a proprietary system
BIOS. The fork's `README.unemu.md` lists every deviation from upstream; those
that change what the guest sees on these machines:

- soft reset through the CMOS shutdown byte (0Fh), as the IBM PC/AT BIOS:
  codes 05h and 0Ah resume through 0040:0067h, 09h returns from the block
  move, 04h boots through POST;
- INT 15h AH=88h (extended memory size from CMOS 30h–31h) and AH=87h (block
  move through protected mode) as the IBM PC/AT BIOS; upstream returns AH=86h
  with CF set;
- POST measures the memory above 1 MiB in protected mode and stores the size
  in CMOS 30h–31h, as the IBM POST does. The sizing returns to real mode
  through one more 8042 CPU reset (shutdown code 09h) on every POST;
- POST checks the CMOS as the IBM PC/AT POST does: battery, the checksum over
  10h–2Dh (upstream sums 10h–20h), the equipment byte against the drives,
  coprocessor and display, the memory sizes and the clock. Errors 161–164
  stop POST at "Press F1 to continue, F2 to run BIOS setup"; a consistent
  CMOS, such as the one unemupc98 derives, prints nothing and adds no wait;
- POST identifies the 80286 without the 8088 interrupt warning and reports an
  80287 as "Intel 80287";
- the display switch (8042 input port bit 6) is read with the IBM PC/AT
  polarity: `board.display_switch = "colour"` selects the colour adapter, as
  it does for the IBM BIOS. The Xi 8088 builds read it the other way;
- A20 is disabled at the end of the 8042 initialization, so the OS boots with
  A20 off, as after the IBM PC/AT POST;
- port 70h bit 7 is the NMI mask with the IBM PC/AT polarity (set masks NMI);
- POST initialises the second 8237 (channel 4 cascade), and the scratch port
  written before the serial and parallel port detection moves from C0h to
  8Eh;
- the banner reads "unemu AT BIOS, Version 1.0.2-unemu.2".

The Kiselev Xi 8088 images stay: the unmodified one boots
`machines/at-freedos.toml` (and its YAML and preset twins) and the BIOS POST,
FreeDOS boot and DOS session goldens built on it; the AT-reset one remains a
catalogued build for tests.

## Licence and authors

The BIOS is the work of **Sergey Kiselev**; the fork keeps the upstream
history in full and his copyright notices unchanged. The changes for the
unemu machines are additions by the unemu contributors, under the same
licence: the **GNU General Public License, version 3 or (at your option) any
later version** (`GPL-3.0-or-later`, the fork's `LICENSE.txt`). The ROM is an
external read-only fixture: it is loaded into guest ROM, never linked into
the emulator, and never committed.

## Rebuild

From the repository root, with Nix (flakes), git and GNU tar on `PATH`, as in
the devShell:

```sh
vendor/unemu-at-bios/fetch.sh
```

The script runs `nix build --no-link` for
`<forge>/unemu-org/unemu-at-bios?ref=refs/tags/v1.0.2-unemu.2&rev=a2cf362d…#unemu-at`,
checks the ROM's size, the far jump at the reset entry and the SHA-256 pin,
and only then replaces the file here. The tag names the release; Nix checks
out the pinned revision, so a moved tag cannot change the sources. The fork's
flake assembles the ROM with nasm from its nixpkgs pin, the same one this
repository's devShell uses, and checks the same hash itself. Nix caches the
fetched sources and the store paths, so a repeated run needs no network.

`<forge>` is never written in a committed file. As for
[`vendor/vgabios`](../vgabios/README.md), the script derives it from this
repository's `origin` remote: the same host, transport and organization, with
the repository name `unemu-at-bios` and the origin's own `.git` suffix
convention. An SSH origin builds over SSH with the same keys; an HTTPS origin
builds over HTTPS, anonymously unless a git credential helper supplies a
login (terminal prompts are disabled, so a private repository fails instead
of waiting). `file://` and absolute-path origins map to a sibling directory.
`fetch.sh --print-flake` prints the reference it would build. Two overrides
exist:

- `UNEMU_AT_BIOS_FLAKE=<flake reference>` builds that reference as given,
  without the `#output` suffix, for a mirror or a work tree under
  development.
- `UNEMU_AT_BIOS_CHECKOUT=<path>` builds the pinned revision from a local
  clone's object store. Its working tree and uncommitted changes are ignored.

The SHA-256 checks apply in every case. A changed ROM fails them; investigate
the change, then deliberately re-measure the goldens before updating the pin.

Only `fetch.sh`, `source-notice.txt` and this README are committed in this
directory. The ROM, the source archive and the temporary `build/` directory
are gitignored.

CI: the `fixtures` gate (`scripts/ci-gate.sh fixtures`) runs this script. From
the checkout's credential-less HTTPS origin the build succeeds only while
`unemu-at-bios` is anonymously readable next to this repository; otherwise
the gate fails with the message above. The `goldens` gate sets
`UNEMUPC98_REQUIRE_FIXTURES=1`, so the tests that need the ROM fail when it is
missing; without that variable they print a skip line and return. See
`backlog/docs/commands.md`.

## Source archive

After the ROM passes its checks, `fetch.sh` writes
`unemu-at-bios-1.0.2-unemu.2-src.tar`, its corresponding source. It is what a
published firmware package carries as its `source` layer: the ROM is
GPL-3.0-or-later, so it may be published only together with it. Its root
directory, `unemu-at-bios-1.0.2-unemu.2-src/`, holds:

- `unemu-at-bios/`: the tree Nix built the ROM from (the flake's own source,
  read with `builtins.getFlake`), so a mirror or a local clone gives the same
  bytes. `LICENSE.txt` and `README.unemu.md` are in it. Upstream's prebuilt
  images are left out: `binaries/` (other boards' BIOS and flash images) and
  `xtide_r625/` (XTIDE Universal BIOS r625 binaries whose source the archive
  does not carry) are build outputs that the ROM does not contain. Upstream's
  flash-image targets that combine the BIOS with those binaries (the
  `*-xtide*.rom` images in `src/Makefile` and `CMakeLists.txt`) therefore do
  not build from the archive; the 32 KiB images do. Everything else is
  unchanged.
- `NOTICE`: [`source-notice.txt`](source-notice.txt), unchanged. It names the
  release, revision and licence, the ROM hash, the paths left out and the
  `nix build` command that rebuilds the ROM. It names the repository
  `unemu-org/unemu-at-bios` but no forge address.

Before it writes the archive, the script builds `#unemu-at` from the staged
tree itself (`nix build path:<staged tree>#unemu-at`) and checks the ROM hash,
so the archive always builds what we ship.

The whole archive is **GPL-3.0-or-later**. It is an uncompressed POSIX ustar
file written by GNU tar: members sorted by name, every timestamp set to the
revision's commit time (2026-11-15 09:10:22 UTC, the flake's
`lastModified`), owner and group 0 with no names, and modes 0644 or 0755. Its
SHA-256 is pinned in `fetch.sh`:

| File | SHA-256 |
|---|---|
| `unemu-at-bios-1.0.2-unemu.2-src.tar` | `6c7aaace8c2bc35661ced2596bcad2fd26e77778985aab45742cee560e0fa47e` |

MEASURED on 2026-11-15 on aarch64-darwin with GNU tar 1.35: two runs from the
origin and one with `UNEMU_AT_BIOS_CHECKOUT` write this same archive and the
same ROM, and the staged tree rebuilds the pinned ROM hash. A Linux run has
not been measured.

## Catalogue and machines

`UNEMU_AT_BIOS_1_0_2_UNEMU_2` in `crates/assembly/src/firmware.rs` identifies
`unemu-at-bios-1.0.2-unemu.2` as an open system BIOS for `ibm-5170`,
providing the floppy service and requiring the 8042, the RTC/CMOS and the
cascaded 8259s, with an 80286 at least. `machines/at-games-open.toml` and
`machines/at-ide.toml` boot it, and the `open-at` templates carry it as their
system BIOS package.

## Later: registry artifacts

A later change will fetch the ROM as an open OCI artifact from the default
registry's official namespace, `registry.unemu.dev/unemu/`, instead of
building it, still verified by the same SHA-256 pin. Until then this script
builds from source with Nix.
