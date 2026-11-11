# User-supplied system ROMs

These images are proprietary. Users supply them separately; this project never
commits or distributes them. The catalogue stores identities and chip metadata
only. No default preset or machine file selects these images, and their status
is Listed: identity checks work, execution has not been qualified by goldens.

You may store parts at the repository-relative path
`vendor/user-roms/<entry id>/<part number lowercase>.<chip label lowercase>`.
These are MAME-style names, for example
`vendor/user-roms/ibm-5170-bios-1985-06-10/6480090.u27`.
These filenames are optional labels. Tests search recursively in
`vendor/user-roms/`, then in `UNEMUPC98_USER_ROMS_DIR` if set. Set that variable
to an absolute path to a folder of unpacked dumps, rather than a volume root or
home directory. Dumps must be plain files: MAME `.zip` sets and other archives
are not unpacked by discovery. Arbitrary names and nested directories work. Identity is
the catalogue SHA-256, either of a complete pre-interleaved image or of every
part in one known layout. Earlier roots take priority; parts can span both roots.
Only regular files of known sizes are hashed, once per canonical path per test
process; keep files unchanged during a test run. Root, file and directory symlinks
are followed; canonical directory tracking stops loops. A set-but-missing,
relative or non-directory external root is a configuration error, even if vendor
firmware matches. An unreadable subdirectory aborts scanning with a redacted
error instead of silently skipping.
Discovery never prints contents or hashes of user files. Display names and
discovery errors use `<UNEMUPC98_USER_ROMS_DIR>/relative/name` instead of the
private root path. Incomplete or wrong layouts are absent, without per-file hash
diagnostics from discovery. Partial layouts append only counts to the skip line,
for example `; layout 0: 1 of 2 parts found`, with no paths or hashes.
A pre-interleaved 65536-byte file works with `firmware.bios.image` or `--bios` too.

Verify each dump with `shasum -a 256 <file>` or `sha256sum <file>`.

| Entry | Board | Assembled size | Assembled SHA-256 |
|---|---|---|---|
| ibm-5170-bios-1984-01-10 | type 1, 6 MHz | 65536 | `74e7b36b4ec0adc5ac3277a887579996c1d2aa755b9892ef3afe7485c10ce04f` |
| ibm-5170-bios-1985-06-10 | type 2, 6 MHz | 65536 | `6fb653b58e678cb69b0f943a105c6c803b18a6db23d13c0abccec5b66c6ee652` |
| ibm-5170-bios-1985-11-15 | type 3, 8 MHz | 65536 | `872febf7d7034bfadf72ed1936e06600eca72456b9ded41d15b7a284de20337d` |

Offsets below are in assembled bytes. Layout A and B of 1984 are alternatives.

| Entry / layout | Chip | Part number | Lane | Offset | Size | Part SHA-256 |
|---|---|---|---|---|---|---|
| 1984-01-10 / A | U27 | 6181028 | even | 0 | 32768 | `a7d0f995c25a29f2556fce563939cbd101f9225f71103debe441e3b6458cf933` |
| 1984-01-10 / A | U47 | 6181029 | odd | 0 | 32768 | `fadb3a5c949b0096b2f753c76bac2edf73040066c1959eeee8f7d092c836b6c5` |
| 1984-01-10 / B | U17 | 6181024 | even | 0 | 16384 | `b9a9aa7a32c4d50ac16034a489962cbd06455ab14cbb78a6d2bbf519b22c8a73` |
| 1984-01-10 / B | U37 | 6181026 | odd | 0 | 16384 | `a54d9546361464489a118e6d7a7f9f959e6678013ab4f4e8a98993ac7bdc916d` |
| 1984-01-10 / B | U27 | 6181025 | even | 32768 | 16384 | `87340797ab2f2eb86185f5289bd0a2bc2e9f14e0633770a87bf4cb0624208a5c` |
| 1984-01-10 / B | U47 | 6181027 | odd | 32768 | 16384 | `a1106cc91a86251c524db72934f7eaa5abb39c2a986a5ac3477a017da7aac6e3` |
| 1985-06-10 / A | U27 | 6480090 | even | 0 | 32768 | `c8654ed7a922644db2b2cce54c540fcfa7cbbae8c514d7eb3dbfbb8187471bfd` |
| 1985-06-10 / A | U47 | 6480091 | odd | 0 | 32768 | `470ab30f9eb7d56c24f5ce10c7d307c539b9f923ea97dd536d2d5ba7e36d471b` |
| 1985-11-15 / A | U27 | 61X9266 | even | 0 | 32768 | `3b921a451d78efcd91232a01cd6936d6ba12dad4374466655aff8ef3a5dc764e` |
| 1985-11-15 / A | U47 | 61X9265 | odd | 0 | 32768 | `aec1759a8245d50be93c9b081ab1ac7de58dd8141f603c7745ae66d2d4c29e4d` |

Create a child layer in `machines/` (the parent `at-freedos.toml` stays unchanged):

```toml
schema = 1
extends = "at-freedos.toml"

[firmware.bios]
parts = [
  { image = "../vendor/user-roms/ibm-5170-bios-1985-06-10/6480090.u27", lane = "even" },
  { image = "../vendor/user-roms/ibm-5170-bios-1985-06-10/6480091.u47", lane = "odd" },
]
sha256 = "6fb653b58e678cb69b0f943a105c6c803b18a6db23d13c0abccec5b66c6ee652"
```

Each part also accepts `offset` (default 0) and an optional `sha256` pin.
The BIOS-level pin checks the assembled bytes. `image` and `parts` are mutually
exclusive. A child source replaces the complete inherited source and drops
inherited `sha256` and `provides` unless the child restates them. Known images
then use their catalogue services. Editing an existing BIOS table in place
requires removing `image` before adding `parts`, and removing `provides` or
setting it to `["floppy", "fixed-disk"]` for these IBM images.
Relative paths belong to
the layer that defines them. Only the system BIOS supports parts.

Pairs must have equal nonzero chip sizes and tile the assembled image without
gaps or overlaps. The layout with the most matching chip hashes (and the same
part count) diagnoses chips in wrong lanes or halves and names every bad dump
with its expected chip and expected/actual hashes. Exchanged lanes within pairs
are reported as swapped only when each known chip occurs exactly once.
Duplicated and missing chips are named explicitly, as are positions outside the
chosen layout. These diagnoses precede the assembled pin check;
individual part pins are checked first. A pin for another listed BIOS still
reports the assembled pin mismatch.
Unknown assembled identities report the board catalogue. The run-only
`--allow-unlisted-firmware` option relaxes membership and chip-identity diagnoses, retaining layout and pin checks.

The ignored `user_rom_ibm_1985_identity` checks the 1985-06-10 identity and chip
diagnostics when present, without executing it. It accepts parts or a complete
image. Run all such tests with `bash scripts/ci-gate.sh user-roms`; CI never calls
this lane and never has proprietary firmware. Missing dumps print:

```text
SKIPPED: user-supplied firmware <entry id> not found (set UNEMUPC98_USER_ROMS_DIR or see vendor/user-roms/README.md)
```

This also skips with `UNEMUPC98_REQUIRE_FIXTURES=1`. Set
`UNEMUPC98_REQUIRE_USER_ROMS=1` to panic with the same text on absence.
See `backlog/docs/commands.md` for the commands and shared test helper.

Goldens for proprietary firmware may store hashes and our own interpretation:
POST codes, which tests passed, retired-instruction counts and similar measured
outcomes. Never store proprietary firmware bytes or copies of its screen text
beyond short identifiers. Screenshots and full console transcripts do not belong
in these goldens. All committed discovery tests use synthetic bytes.
