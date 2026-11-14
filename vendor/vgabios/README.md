# Open VGA BIOS fixtures

`fetch.sh` builds two files from the **unemu fork of the LGPL VGABIOS**, the
video BIOS used by Bochs and QEMU, changed to run on an 80286. The fork is the
`vgabios` repository in the same forge organization as this repository
(`unemu-org/vgabios`). It is pinned at release tag **`v0.8a-unemu.1`**, source
revision `8e154f04c5b1c8029d1ac3a3827f077352d579b7`:

| File | Flake output | Size | SHA-256 |
|---|---|---|---|
| `vgabios-std.bin` | `#std` | 32,768 bytes | `7570ef9c66fd386450bb35fd2b6c91aba6f8ec69a565a1d7747e6e6716ea5389` |
| `int10-probe-1200.img` | `#int10-probe` | 1,228,800 bytes | `21b9dd1db3068bb73f3fde16058578368719f98b6d4335f45ec6681eb47beb58` |

`vgabios-std.bin` is the standard ISA VGA build: upstream 0.8a with no VBE,
PCI, Cirrus or Banshee code and 80286 instructions only. It starts with
`55 AA 40` (32 KiB in 512-byte units) and its bytes sum to zero modulo 256.
`int10-probe-1200.img` is a bootable 1.2M floppy. It checks INT 10h in modes
03h, 0Dh and 13h by reading registers and video memory back, returns to mode
03h, prints one `name ok` or `name FAIL` per check and the failure count, and
halts. The fork's `README.unemu.md` lists the changes from upstream.

## Licence

The fork keeps upstream's licence. The `vgabios.c` header grants the GNU Lesser
General Public License "version 2 of the License, or (at your option) any later
version"; the `COPYING` file is LGPL version 2.1. The catalogue records
`LGPL-2.1-or-later`. The probe source carries
`SPDX-License-Identifier: LGPL-2.1-or-later`. Copyright: the LGPL VGABios
developers and the contributors to the fork. Both files are external read-only
fixtures; they are not linked into the emulator and never committed.

## Rebuild

From the repository root, with Nix (flakes) and git on `PATH`, as in the devShell:

```sh
vendor/vgabios/fetch.sh
```

The script runs `nix build --no-link` for both outputs of
`<forge>/unemu-org/vgabios?ref=refs/tags/v0.8a-unemu.1&rev=8e154f04…`, checks the
ROM's size, header and byte sum, checks both SHA-256 pins, and only then
replaces the files here. The tag names the release; Nix checks out the pinned
revision, so a moved tag cannot change the sources. Nix caches the fetched
sources and the store paths, so a repeated run needs no network.

`<forge>` is never written in a committed file. The script derives it from
this repository's `origin` remote: the same host, transport and organization,
with the repository name `vgabios` and the origin's own `.git` suffix
convention. An SSH origin (`ssh://` or scp-like `[user@]host:path`) builds over
SSH with the same keys; an HTTPS origin builds over HTTPS, anonymously unless a
git credential helper supplies a login (terminal prompts are disabled, so a
private repository fails instead of waiting). `file://` and absolute-path
origins map to a sibling directory. A clone from a mirror derives the mirror's
address, so a mirror must publish `vgabios` next to this repository.
`fetch.sh --print-flake` prints the reference it would build. Two overrides
exist:

- `UNEMU_VGABIOS_FLAKE=<flake reference>` builds that reference as given,
  without the `#output` suffix, for a mirror or a work tree under development.
- `UNEMU_VGABIOS_CHECKOUT=<path>` builds the pinned revision from a local
  clone's object store. Its working tree and uncommitted changes are ignored.

If the build fails, the script says which source it used and names both
overrides.

The SHA-256 checks apply in every case. A changed ROM or probe fails them;
investigate the change, then deliberately re-measure the goldens before
updating a pin.

The flake preprocesses `vgabios.c` with the host's `$CC -E -P`: clang 19.1.7 on
Darwin and gcc 14.3.0 on Linux, both from its nixpkgs pin, followed by dev86
1.0.1 `bcc`/`as86`. MEASURED on aarch64-darwin: the two preprocessors differ by
one leading space before the first `#asm`, and assembling gcc 14.3.0's output
with the same dev86 steps and file names gives the pinned ROM hash. The ROM
bytes do depend on the intermediate file names, which the flake fixes. A
complete x86_64-linux build has not been measured (no Linux builder was
available); a Linux hash mismatch would be a vgabios-side fix, such as pinning
one preprocessor in its flake.

Only `fetch.sh` and this README are committed in this directory. The outputs
and the temporary `build/` directory are gitignored.

CI: the `fixtures` gate (`scripts/ci-gate.sh fixtures`) runs this script. The
checkout's origin is its HTTPS URL without credentials, so the build succeeds
only while `vgabios` is anonymously readable next to this repository; otherwise
the gate fails with the message above. MEASURED on 2026-11-14: with git's
global and system configuration disabled, an HTTPS origin built both pinned
files. The `goldens` gate sets
`UNEMUPC98_REQUIRE_FIXTURES=1`, so the tests that need these files fail when
they are missing; they do not skip. Without that variable (a local run) they
print a skip line and return. See `backlog/docs/commands.md`.

## Catalogue and machine

`FirmwareEntry::VGABIOS_0_8A_UNEMU_1_STD` identifies `vgabios-0.8a-unemu.1-std`
as an open video BIOS for `ibm-5170`, suggested at C0000h, requiring an
80286 and the register-level `vga` card. It occupies C0000h–C7FFFh, the whole
video-ROM window that both the IBM and Kiselev BIOSes scan.
`machines/at-games-open.toml` maps it there with the `vga` card; its XTIDE
option ROM follows at C8000h. The goldens in `crates/cli/tests/at_games.rs`
and `crates/cli/tests/open_vga_golden.rs` boot FreeDOS, run the probe and pin
mode 0Dh and mode 13h scanout frames with this ROM.

## Later: registry artifacts

A later change will fetch both files as open OCI artifacts from the default
registry's official namespace, `registry.unemu.dev/unemu/`, instead of building
them, still verified by the same SHA-256 pins (machine images decision and its
2026-11-14 amendments; firmware packages decision). Until then this script
builds from source with Nix.
