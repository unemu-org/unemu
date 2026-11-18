# vendor/at-loadall-test — open AT LOADALL test

`at-loadall-test.asm` is a project-owned guest program, licensed like the
rest of the repository (AGPL-3.0-or-later). `build.sh` assembles it into
`at-loadall-test-1200.img`, a bootable 1.2M floppy, entirely offline. The
image and the temporary `build/` directory are gitignored; only the source,
the script and this README are committed.

The program needs no DOS. Its boot sector loads 4 sectors (sectors 2–5 of
track 0, head 0) to `0800:0000h` and jumps there. In real mode it turns A20
on through the 8042, then moves a 128-word block from conventional memory to
physical `150000h` and back with the undocumented 80286 LOADALL (0F 05): a
table at physical `800h` loads an ES (then a DS) descriptor cache whose base
lies above 1 MiB, and REP MOVSW runs through it, as a 286 extended-memory
manager does. The BIOS block move (INT 15h AH=87h) then reads `150000h`
back into a third buffer, which checks the block's physical place without
LOADALL. Last, it reports what 0F 04 does, for information only, with a
LOADALL table that resumes right after it, so a CPU that runs 0F 04 as
LOADALL does not loop. It prints one line per check plus a failure count, on the BIOS teletype and on
COM1 (`3F8h`, 115200 baud, 8N1), then halts with interrupts disabled. What
each line checks, and why, is in
[`backlog/docs/design/at-loadall-test.md`](../../backlog/docs/design/at-loadall-test.md).

## Usage and reproducibility

```sh
vendor/at-loadall-test/build.sh
```

Requires Bash, nasm, `od`, `awk`, `tr`, `wc`, `mktemp`, `mv` and either
`sha256sum` or `shasum -a 256`. The project devShell provides nasm 3.01, the
version used for the pinned image. nasm's flat-binary output contains no
timestamps or host data; the image is the boot sector, the program padded to
4 sectors and zero bytes up to 1,228,800 bytes. SHA-256 of the image:

```text
7b06bb0c93c13eec949222b5b2a5f8335bc6f8f5ec26f0b660c481bf516830e8
```

The script checks size, the `55AAh` boot signature and this hash before it
moves the image into place. Assembly fails if the program outgrows the 4
loaded sectors. A changed source or another nasm version can change the hash;
deliberately re-measure the golden before updating the pin.

The CI `fixtures` gate runs this script. The golden is in
`crates/pc-harness/tests/at_loadall_test_golden.rs`; see `backlog/docs/commands.md`.
