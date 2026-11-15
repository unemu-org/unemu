# vendor/at-pm-test — open AT protected-mode return test

`at-pm-test.asm` is a project-owned guest program, licensed like the rest of
the repository (AGPL-3.0-or-later). `build.sh` assembles it into
`at-pm-test-1200.img`, a bootable 1.2M floppy, entirely offline. The image and
the temporary `build/` directory are gitignored; only the source, the script
and this README are committed.

The program needs no DOS. Its boot sector loads 14 sectors (sectors 2–15 of
track 0, head 0) to `0800:0000h` and jumps there. It first checks the BIOS
extended-memory services: INT 15h AH=88h (size) and AH=87h (a block moved
above 1 MiB and back). For 16 rounds it then enters 80286 protected mode,
returns through an 8042 pulse reset and through a deliberate triple fault,
both resumed by the BIOS through the CMOS shutdown byte and `0040:0067h`,
and then switches A20 through the 8042. Last, it reports one AH=87h move
that faults inside the BIOS, for information only. It prints one line per
check, stage and round plus a failure count, on the BIOS teletype and on
COM1 (`3F8h`, 115200 baud, 8N1), then halts with interrupts disabled. What
each stage checks, and why, is in
[`backlog/docs/design/at-pm-test.md`](../../backlog/docs/design/at-pm-test.md).

## Usage and reproducibility

```sh
vendor/at-pm-test/build.sh
```

Requires Bash, nasm, `od`, `awk`, `tr`, `wc`, `mktemp`, `mv` and either
`sha256sum` or `shasum -a 256`. The project devShell provides nasm 3.01, the
version used for the pinned image. nasm's flat-binary output contains no
timestamps or host data; the image is the boot sector, the program padded to
14 sectors and zero bytes up to 1,228,800 bytes. SHA-256 of the image:

```text
7e91998468dcc07bc058148afaa5404124aecc44e40a774e9b7ae4d0519884f4
```

The script checks size, the `55AAh` boot signature and this hash before it
moves the image into place. Assembly fails if the program outgrows the 14
loaded sectors. A changed source or another nasm version can change the hash;
deliberately re-measure the goldens before updating the pin.

The CI `fixtures` gate runs this script. The goldens are in
`crates/cli/tests/at_pm_test_golden.rs`; see `backlog/docs/commands.md`.
