# Open VGA BIOS fixtures

`fetch.sh` builds two files from the **unemu fork of the LGPL VGABIOS**, the
video BIOS used by Bochs and QEMU, changed to run on an 80286. The fork is
public at <https://github.com/unemu-org/vgabios>, a GitHub fork of
`bochs-emu/VGABIOS`. It is pinned at release tag **`v0.9d-unemu.1`** (a
signed tag), source revision `40522278d5b703e637271696783721f4b5ea541f`:

| File | Flake output | Size | SHA-256 |
|---|---|---|---|
| `vgabios-std.bin` | `#std` | 32,768 bytes | `b867f3846f44be63b1b10febc1ce79c9435f5d1233c67a0b63164ab48359a582` |
| `int10-probe-1200.img` | `#int10-probe` | 1,228,800 bytes | `7c928cec8e49c04c91d1cb7194b5f508db3d86ac89be9867a7290eb04c11f1b4` |

`vgabios-std.bin` is the standard ISA VGA build: upstream 0.9d with no VBE,
PCI, Cirrus or Banshee code and 80286 instructions only. It starts with
`55 AA 40` (32 KiB in 512-byte units) and its bytes sum to zero modulo 256.
`int10-probe-1200.img` is a bootable 1.2M floppy. It checks INT 10h in modes
03h, 0Dh and 13h by reading registers and video memory back, returns to mode
03h, prints one `name ok` or `name FAIL` per check and the failure count, and
halts. The fork's `README.unemu.md` lists the changes from upstream.

`v0.9d-unemu.1` moves the fork's base from upstream 0.8a to 0.9d (upstream's
history merged, ours not rewritten) and adds the teletype BEL: INT 10h AH=0Eh,
and AH=13h with 07h in the string, sound the PC speaker as the IBM PC/AT BIOS
does, with PIT counter 2 in mode 3 and divisor 1331 (about 896 Hz) for 31/64 s
(about 0.48 s), timed by counting the port 61h bit 4 refresh toggles.
Interrupts are enabled during the tone; port 61h and the interrupt flag are
restored. The probe gains the check `tty.bel`: counter 2 starts in mode 0 at
FFFFh, and after AX=0E07h it must have been reprogrammed, with the cursor and
port 61h bits 0–3 unchanged. How long the tone lasts is for the emulator to
check: `open_vga_bios_teletype_bel_sounds_the_speaker_for_31_64_seconds` in
`crates/cli/tests/open_vga_golden.rs` reads it from the headless speaker trace.

## Licence

The fork keeps upstream's licence. The `vgabios.c` header grants the GNU Lesser
General Public License "version 2 of the License, or (at your option) any later
version"; the `COPYING` file is LGPL version 2.1. The catalogue records
`LGPL-2.1-or-later`. The probe source carries
`SPDX-License-Identifier: LGPL-2.1-or-later`. Copyright: the LGPL VGABios
developers and the contributors to the fork. Both files are external read-only
fixtures; they are not linked into the emulator and never committed.

## Rebuild

From the repository root, with Nix (flakes), git and GNU tar on `PATH`, as in the
devShell:

```sh
vendor/vgabios/fetch.sh
```

The script runs `nix build --no-link` for both outputs of
`git+https://github.com/unemu-org/vgabios?ref=refs/tags/v0.9d-unemu.1&rev=40522278…`,
checks the ROM's size, header and byte sum, checks both SHA-256 pins, and only
then replaces the files here. The tag names the release; Nix checks out the pinned
revision, so a moved tag cannot change the sources. Nix caches the fetched
sources and the store paths, so a repeated run needs no network.

The default source is that public repository, read anonymously over HTTPS
whatever this repository's own remote is, so a clone from a mirror, or with
no remote at all, builds from the same place. Terminal prompts are disabled,
so an unreachable repository fails instead of waiting for a password.
`fetch.sh --print-flake` prints the reference it would build. Two overrides
exist:

- `UNEMU_VGABIOS_FLAKE=<flake reference>` builds that reference as given,
  without the `#output` suffix, for a mirror that carries the pinned revision
  or a work tree under development.
- `UNEMU_VGABIOS_CHECKOUT=<path>` builds the pinned revision from a local
  clone's object store. Its working tree and uncommitted changes are ignored.

If the build fails, the script says which source it used (for the default,
the repository, tag and revision) and names both overrides.

The SHA-256 checks apply in every case. A changed ROM or probe fails them;
investigate the change, then deliberately re-measure the goldens before
updating a pin.

The flake preprocesses `vgabios.c` with unwrapped gcc 14.3.0 from its nixpkgs
pin on every system, as `gcc -E -P -undef -nostdinc` (no platform macros, no
system headers), followed by dev86 1.0.1 `bcc`/`as86`; the fork's
`README.unemu.md`, "Reproducibility", lists what the build fixes. The ROM bytes
do depend on the intermediate file names, which the flake fixes too. The
fork's `nix flake check` compares both outputs with the release hashes on
whichever system runs it, and its CI runs it on x86_64-linux; this repository
has measured aarch64-darwin only (2026-11-16, both pinned hashes from the
public repository and from a local clone).

Only `fetch.sh`, `source-notice.txt` and this README are committed in this
directory. The outputs, the source archive and the temporary `build/` directory
are gitignored.

CI: the `fixtures` gate (`scripts/ci-gate.sh fixtures`) runs this script; it
needs HTTPS access to GitHub and no credentials. The `goldens` gate sets
`UNEMUPC98_REQUIRE_FIXTURES=1`, so the tests that need these files fail when
they are missing; they do not skip. Without that variable (a local run) they
print a skip line and return. See `backlog/docs/commands.md`.

## Source archive

After both files pass their checks, `fetch.sh` writes
`vgabios-0.9d-unemu.1-src.tar`, their corresponding source. It is what a
published firmware package carries as its `source` layer: the ROM is
LGPL-2.1-or-later, so it may be published only together with it. Its root
directory, `vgabios-0.9d-unemu.1-src/`, holds:

- `vgabios/`: the tree Nix built both files from (the flake's own source,
  read with `builtins.getFlake`), so a mirror or a local clone gives the same
  bytes. Nothing is left out: the fork stopped tracking `.beads/config.yaml`,
  the issue tracker's settings, which named a developer's own sync remote,
  before `v0.8a-unemu.3`; `fetch.sh` still removes that file should a
  release carry it again, since the published archive must not carry such an
  address. The release has none of upstream's prebuilt
  `VGABIOS-lgpl-latest*.bin` images, and a source archive carries no prebuilt
  image.
- `NOTICE`: [`source-notice.txt`](source-notice.txt), unchanged. It names the
  public repository (<https://github.com/unemu-org/vgabios>), the release,
  revision and licence, both file hashes and the `nix build` commands that
  rebuild the files.

The whole archive is **LGPL-2.1-or-later**. It is an uncompressed POSIX ustar
file written by GNU tar: members sorted by name, every timestamp set to the
revision's commit time (2026-11-15 23:43:12 UTC, the flake's `lastModified`),
owner and group 0 with no names, and modes 0644 or 0755. Its SHA-256 is pinned
in `fetch.sh`:

| File | SHA-256 |
|---|---|
| `vgabios-0.9d-unemu.1-src.tar` | `0f0890b6fc95156177c37725329ca8111b889abce33de73aeeaf2fc99ce86c48` |

The archive adds no network access of its own: it reads the source Nix
fetched for the build, which Nix caches (see Rebuild above), so only a first
run needs the network. `UNEMU_VGABIOS_FLAKE` pointing at any other tree (a work tree under
development, another revision) builds and archives that tree, so the archive
pin fails even where the output pins pass; it is meant for mirrors that carry
the pinned revision.

MEASURED on 2026-11-16 on aarch64-darwin with GNU tar 1.35: three runs from
the public repository (the first one measured the hash) and one with
`UNEMU_VGABIOS_CHECKOUT` build this same archive, and `nix build .#std` and
`nix build .#int10-probe` in an extracted archive's `vgabios/` directory give
both pinned file hashes. A Linux run has not been measured.

## Catalogue and machine

`FirmwareEntry::VGABIOS_0_9D_UNEMU_1_STD` identifies `vgabios-0.9d-unemu.1-std`
as an open video BIOS for `ibm-5170`, suggested at C0000h, requiring an
80286 and the register-level `vga` card. It occupies C0000h–C7FFFh, the whole
video-ROM window that both the IBM and Kiselev BIOSes scan.
`machines/at-games-open.toml` maps it there with the `vga` card; its XTIDE
option ROM follows at C8000h. The goldens in `crates/cli/tests/at_games.rs`
and `crates/cli/tests/open_vga_golden.rs` boot FreeDOS, run the probe, time
the BEL tone and pin mode 0Dh and mode 13h scanout frames with this ROM, and
the templates from 1.2.0 on carry it as their video BIOS package.

`FirmwareEntry::VGABIOS_0_8A_UNEMU_1_STD` (`vgabios-0.8a-unemu.1-std`, ROM
`7570ef9c…5389`) stays listed with the status `Superseded`, superseded by
0.9d-unemu.1: teletype BEL sounds the speaker. Its board contract is the new
entry's and it keeps the source archive pin of its own release
(`vgabios-0.8a-unemu.3-src.tar`,
`2d93fcaaa87ac0f310af097ef184d94932a0c9cb255b9e8f991cee511a1cf8eb`), so
machines and images made with it (templates up to 1.1.1) still start. This
script no longer builds it: the entry's `fetch` names this script at
revision `4f73f3d06e120b42a93bbbc0f857f3fb9a1cb0ea` of this repository, the
last one that pinned `v0.8a-unemu.3`; run it in a checkout of that revision
to rebuild the ROM and its archive. That script derives its source from the
checkout's `origin`; where that does not reach the fork, set
`UNEMU_VGABIOS_FLAKE` to the public repository at the pinned tag and revision. Its id keeps the name of `v0.8a-unemu.1`, the
release that first built those bytes; `v0.8a-unemu.2` and `v0.8a-unemu.3`
built them unchanged.

## Later: registry artifacts

A later change will fetch both files as open OCI artifacts from the default
registry's official namespace, `registry.unemu.dev/unemu/`, instead of building
them, still verified by the same SHA-256 pins (machine images decision and its
2026-11-14 amendments; firmware packages decision). Until then this script
builds from source with Nix.
