# test386.asm — third-party CPU-test ROM (test data)

`test386.bin` is built by [`fetch.sh`](fetch.sh) from
[barotto/test386.asm](https://github.com/barotto/test386.asm), pinned at commit
`cfd052d1e64d5375dea5a681c1eadeed64ceda2c`. It is a diagnostic ROM that exercises
an Intel 386 and reports progress by writing a checkpoint ("POST") byte to port
`0x190` at each stage.

## Provenance and licence

- Upstream: <https://github.com/barotto/test386.asm> (originally by Jeff Parsons /
  pcjs.org, extended by Marco Bortolin).
- Licence: **GPL-3.0-or-later** (see the upstream `COPYING`).
- Used here **unmodified, as read-only test data only**: it is assembled with
  `nasm` and loaded into guest RAM by the emulator's smoke test. It is not
  linked into, copied into, or distributed with this project's source.

## Not committed

Neither the upstream clone (`test386.asm/`) nor the build product
(`test386.bin`) is checked in — both are `.gitignore`d. Run `fetch.sh` (which
needs `git` and `nasm`, both in the flake devShell) to produce `test386.bin`
locally. The integration test skips itself when the image is absent.

## Why it stops early here

This emulator is a 16-bit-real-mode 80286-class core (no protected mode, no
386-only instructions beyond what real mode needs). test386 is a 386 ROM, so it
is *expected* to halt partway through its real-mode POST sequence. The smoke test
records how far it gets; it is a plumbing test, not full CPU validation.
