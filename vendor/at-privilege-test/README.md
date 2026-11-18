# vendor/at-privilege-test — open AT privilege-level test

`at-privilege-test.asm` is a project-owned guest program, licensed like the
rest of the repository (AGPL-3.0-or-later). `build.sh` assembles it into
`at-privilege-test-1200.img`, a bootable 1.2M floppy, entirely offline. The
image and the temporary `build/` directory are gitignored; only the source,
the script and this README are committed.

The program needs no DOS. Its boot sector loads 6 sectors (sectors 2–7 of
track 0, head 0) to `0800:0000h` and jumps there. It builds a GDT with
ring-0 and ring-3 code, data and stack segments, an IDT and a 286 TSS with
SS0:SP0, enters 80286 protected mode and loads TR. Then it returns to ring 3
with IRET (IOPL 3), comes back to ring 0 through INT 40h and a DPL-3
interrupt gate on the TSS's ring-0 stack, returns to ring 3 with IRET again
(IOPL 0, ES holding a DPL-0 segment, which the return nulls) and executes
CLI there, which raises #GP(0) to a ring-0 handler. It prints one line per
case plus a failure count, on COM1 (`3F8h`, 115200 baud, 8N1) and straight
into the text screen, then halts in protected mode with interrupts
disabled. What each line checks, and why, is in
[`backlog/docs/design/at-privilege-test.md`](../../backlog/docs/design/at-privilege-test.md).

## Usage and reproducibility

```sh
vendor/at-privilege-test/build.sh
```

Requires Bash, nasm, `od`, `awk`, `tr`, `wc`, `mktemp`, `mv` and either
`sha256sum` or `shasum -a 256`. The project devShell provides nasm 3.01, the
version used for the pinned image. nasm's flat-binary output contains no
timestamps or host data; the image is the boot sector, the program padded to
6 sectors and zero bytes up to 1,228,800 bytes. SHA-256 of the image:

```text
dad57a8b9289cc399f0c57b0eaa54916cc8d36a5b6381d6d5c4baca82d57bf60
```

The script checks size, the `55AAh` boot signature and this hash before it
moves the image into place. Assembly fails if the program outgrows the 6
loaded sectors. A changed source or another nasm version can change the hash;
deliberately re-measure the golden before updating the pin.

The CI `fixtures` gate runs this script. The golden is in
`crates/pc-harness/tests/at_privilege_test_golden.rs`; see `backlog/docs/commands.md`.
