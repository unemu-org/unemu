// SPDX-License-Identifier: AGPL-3.0-or-later
//! test386 real-mode boot smoke test (spike) (§3.1, §5, §10).
//!
//! Boots the third-party [test386](../../../vendor/test386/README.md) diagnostic
//! ROM from RAM and records how far it gets. This is a plumbing smoke test of the
//! reset → real-mode-execution → POST-capture path, **not** CPU validation: the
//! image is a 386 diagnostic and this core is 16-bit real mode only, so it is
//! expected to stop early. The value is the *honest stop point*, captured on
//! every exit path (including the error path) through the POST-port tracing seam.
//!
//! ## Observed result (this CPU revision)
//!
//! The run stops at the **very first instruction** — the reset vector's far jump
//! `jmp 0xF000:0x0045` (opcode `0xEA`) at linear `0xFFFF0` — with an **empty POST
//! stream**. The real-mode `x86jit` backend does not lift a far `JMP` (a control
//! transfer that loads a new `CS`), the same family of segment-loading
//! instructions the other real-mode demos work around by having the embedder seed
//! `CS`/segments directly. So execution never reaches `POST 0`; no checkpoint byte
//! is written to port `0x190`.
//!
//! This is **earlier** than the pre-run prediction (`[00,01,02,03]` then a `mov
//! fs` at POST 3): the wall is the reset far jump, before POST 0. The assertions
//! below pin the observed byte stream (empty) and the stop *kind* (`Unsupported`),
//! never an address — NASM output shifts stop addresses with the toolchain, so the
//! faulting address lives only in the diagnostic `detail` string and this comment.
//!
//! ## Pinned inputs
//!
//! - test386 image: `barotto/test386.asm` @
//!   `cfd052d1e64d5375dea5a681c1eadeed64ceda2c` (built by `vendor/test386/fetch.sh`).
//! - CPU: `x86jit-core` (branch `pc-emulator`), the rev pinned in `Cargo.lock`.
//!   The stop point is only meaningful against a known CPU revision; re-baseline
//!   the golden if the missing instruction gains real-mode support.

use cli::{run_test386, StopReason};
use std::path::PathBuf;

/// Absolute path to the (gitignored) built test386 image.
fn image_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/test386/test386.bin")
}

/// The observed POST checkpoint stream: empty, because execution stops on the
/// reset far jump before `POST 0` writes port `0x190`. Golden the exact stream.
const GOLDEN_POST: &[u8] = &[];

#[test]
fn test386_smoke_stop_point_matches_golden() {
    let path = image_path();
    let Ok(blob) = std::fs::read(&path) else {
        // Mirror the corpus-absent skip pattern: the image is fetched/built, not
        // committed, so a clean checkout without it simply skips (never fails).
        eprintln!(
            "skipping: test386 image not found at {} — run vendor/test386/fetch.sh",
            path.display()
        );
        return;
    };
    assert_eq!(
        blob.len(),
        65536,
        "test386 default config is a 64 KiB image"
    );

    let run = run_test386(&blob);

    // The observed POST byte stream (empty on this CPU revision). Pinned exactly.
    assert_eq!(
        run.post, GOLDEN_POST,
        "POST checkpoint stream drifted from the recorded golden"
    );

    // Assert the stop KIND only — the address is toolchain-dependent and lives in
    // the detail string for the report, never in an assertion.
    assert!(
        matches!(run.stop, StopReason::Unsupported { .. }),
        "expected an unsupported-instruction stop (the reset far jump the \
         real-mode core does not lift), got {:?}",
        run.stop
    );
}
