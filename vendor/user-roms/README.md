# User-supplied files

This folder holds your own game files and firmware dumps for the front end's
ignored user-file tests. Everything here except this README is ignored by git.
Proprietary bytes and screen text must never be committed; the repository keeps
only file names, sizes and hashes for identification.

The game-file tests search recursively in this order:

1. This repository's `vendor/user-roms/`.
2. `UNEMUPC_USER_ROMS_DIR`, if set; use an absolute path to a folder.

Files match by size and SHA-256, under any name or nested folder, and may span
both roots. Searches follow file and directory symlinks and track canonical
directories to avoid revisiting them. Missing or non-directory roots are skipped.
The simple volume and scaffold/export searches skip unreadable entries; recipe
library scans record read errors and continue.

The volume tests need unpacked files: their discovery does not open archives.
Recipe plan/build tests also search ZIP members when the archive name ends in
`.zip` (case-insensitive); Ken's Labyrinth can be built from its loose members or
from `LABFULL.ZIP`. Its scaffold test needs the archive itself. For Azuramoon's
scaffold and export tests, keep the complete original
folder tree, with `AZURAMON.COM` under that exact name: its containing folder is
the operand. These two comparisons omit dot-prefixed files and folders.

| Test and source file | Required files | Expected identities |
|---|---|---|
| `user_rom_kenslab_disk_builds_twice_to_the_same_bytes` — [user_media_volumes.rs](../../crates/pc-image/tests/user_media_volumes.rs) | Ken's Labyrinth 2.01: all 19 unpacked `LABFULL.ZIP` members | [Ken's recipe](../../backlog/docs/design/recipe-corpus/kenslab-2.01/kenslab-2.01.media.toml), `unpack` step's `members` |
| `user_rom_azuramoon_split_builds_twice_to_the_same_bytes` — [user_media_volumes.rs](../../crates/pc-image/tests/user_media_volumes.rs) | Azuramoon: Chapter I: 204 unpacked files, one per distinct content, reused for 561 entries | [Azuramoon recipe](../../backlog/docs/design/recipe-corpus/azuramon/azuramoon-2026.media.toml), `[input.game]` `files` |
| `user_rom_kenslab_builds_from_the_users_files_and_its_disk_is_the_pinned_one` — [recipe_build.rs](../../crates/pc-image/tests/recipe_build.rs) | Ken's Labyrinth 2.01: the 19 members, loose or in `LABFULL.ZIP` | [Ken's recipe](../../backlog/docs/design/recipe-corpus/kenslab-2.01/kenslab-2.01.media.toml), `[input.labfull]` and `members` |
| `user_rom_azuramoon_builds_its_ten_disks_from_the_users_folder` — [recipe_build.rs](../../crates/pc-image/tests/recipe_build.rs) | Azuramoon: Chapter I: 204 distinct contents, loose or in ZIPs, reused for 561 entries | [Azuramoon recipe](../../backlog/docs/design/recipe-corpus/azuramon/azuramoon-2026.media.toml), `[input.game]` `files` |
| `user_rom_scaffold_of_labfull_zip_gives_the_corpus_tables` — [recipe_scaffold.rs](../../crates/cli-pc/tests/recipe_scaffold.rs) | Ken's Labyrinth 2.01: `LABFULL.ZIP` itself | [Ken's recipe](../../backlog/docs/design/recipe-corpus/kenslab-2.01/kenslab-2.01.media.toml), `[input.labfull]` and `members` |
| `user_rom_scaffold_of_the_azuramon_folder_gives_the_corpus_table` — [recipe_scaffold.rs](../../crates/cli-pc/tests/recipe_scaffold.rs) | Azuramoon: Chapter I: the original 561-file folder tree | [Azuramoon recipe](../../backlog/docs/design/recipe-corpus/azuramon/azuramoon-2026.media.toml), `[input.game]` `files` |
| `user_rom_azuramoon_plan_finds_561_entries_from_204_files` — [recipe_plan.rs](../../crates/cli-pc/tests/recipe_plan.rs) | Azuramoon: Chapter I: one file per distinct content suffices (204) | [Azuramoon recipe](../../backlog/docs/design/recipe-corpus/azuramon/azuramoon-2026.media.toml), `[input.game]` `files` |
| `user_rom_azuramoon_exported_with_files_is_the_users_folder` — [image_export.rs](../../crates/cli-pc/tests/image_export.rs) | Azuramoon: Chapter I: the original 561-file folder tree | [Azuramoon recipe](../../backlog/docs/design/recipe-corpus/azuramon/azuramoon-2026.media.toml), `[input.game]` `files` |
| `user_rom_at_games_freedos_prompt` — [at_games.rs](../../crates/cli-pc/tests/at_games.rs) | `ibm-5170-bios-1985-06-10` and `ibm-vga-bios-1987-32k`; FreeDOS 1.2M boot/tools fixtures | [Core ROMs][core-roms] |
| `user_rom_at_games_ctrl_alt_del_reboot` — [at_games.rs](../../crates/cli-pc/tests/at_games.rs) | `ibm-5170-bios-1985-06-10` and `ibm-vga-bios-1987-32k`; FreeDOS 1.2M boot/tools fixtures | [Core ROMs][core-roms] |
| `user_rom_at_games_ctrl_alt_del_reboot_frame_chunks` — [at_games.rs](../../crates/cli-pc/tests/at_games.rs) | `ibm-5170-bios-1985-06-10` and `ibm-vga-bios-1987-32k`; FreeDOS 1.2M boot/tools fixtures | [Core ROMs][core-roms] |
| `user_rom_cmos_typed_bios_refuses_untyped_geometries_per_option` — [new_machine.rs](../../crates/pc-library/src/new_machine.rs) | `ibm-5170-bios-1985-06-10` | [Core ROMs][core-roms] |

Run the local opt-in lane from the repository root:

```sh
bash scripts/ci-gate.sh user-roms
```

The lane requires a clean checkout of the pinned core prepared by
`scripts/core-checkout.sh`; it verifies that checkout, exports
`UNEMUPC_FIXTURE_ROOT`, and runs the workspace's ignored `user_rom_` tests.
CI never runs this lane. Missing game files normally produce a `SKIPPED: ...`
message (sometimes with counts) and a successful return; set
`UNEMUPC_REQUIRE_USER_ROMS=1` to fail instead. Once a matching `AZURAMON.COM` is
found, an incomplete scaffold/export tree fails regardless of that variable.
The tests do not download missing game files. Scaffold/export
comparisons report verdicts; the Azuramoon volume/build tests also print generated
disk hashes and, for the volume test, a volume count. No game bytes or screen
text are printed.

Passing tests' output is captured by default. To see `SKIPPED` messages and hash
lines, run the lane's test command with `--nocapture` in the devShell:

```sh
export "$(bash scripts/core-checkout.sh --print-root)"
cargo test --locked --profile test --workspace --no-fail-fast -- --ignored user_rom_ --nocapture
```

Additional firmware tests, firmware discovery rules and ROM identity tables live
in the PC core repository, [unemupc][core-roms].

[core-roms]: https://github.com/unemu-org/unemupc/blob/main/vendor/user-roms/README.md
