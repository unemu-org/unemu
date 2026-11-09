// SPDX-License-Identifier: AGPL-3.0-or-later
//! test386 real-mode boot smoke test (spike) (§3.1, §5, §10).
//!
//! Boots the third-party [test386](../../../vendor/test386/README.md) diagnostic
//! ROM from RAM and records how far it gets. This is a plumbing smoke test of the
//! reset → real-mode-execution → POST-capture path, **not** CPU validation: the
//! image is a 386 diagnostic and this core is 16-bit real mode only, so it is
//! expected to stop early. The value is the *honest stop point*, captured on every
//! exit path (including the error path) through the POST-port tracing seam.
//!
//! ## Observed result (this CPU revision)
//!
//! The image far-jumps from the reset vector into the POST sequence and runs the
//! real-mode init: it writes `POST 0` (`0x00`) to port `0x190`, builds the
//! real-mode IDT, loads SS/DS/ES, writes `POST 1` (`0x01`), then enters the
//! conditional-jump test. **POST stream: `[0x00, 0x01]`.**
//!
//! It stops inside **POST 1** (the conditional-jump test, `tests/jcc_m.asm`'s
//! `testJcc` macro) on the first instruction that macro uses to seed the flags
//! without the stack:
//!
//! ```text
//! F000:0083  B4 01   mov ah, 0x1
//! F000:0085  9E      sahf        <- stops here: SAHF is not lifted
//! F000:0086  73 24   jnc ...
//! ```
//!
//! i.e. **`SAHF` (opcode `0x9E`) at linear `0xF0085`** surfaces as
//! `Exit::UnknownInstruction`. (The run reports the faulting address `0x85` and
//! leaves RIP at the enclosing block start `0x83`; addresses are offsets within
//! `CS = 0xF000`.) So `SAHF`/`LAHF` is the next real-mode instruction a seam would
//! need to lift to push test386 past POST 1 into POST 2's 32-bit `mul`/`div`.
//!
//! An earlier run against a stale CPU pin stopped one instruction into the image,
//! on the reset vector's far jump — that pin predated the far-`JMP` / segment-load
//! lift. With the current pin the far jump, `mov Sreg`, and the `0x67`-prefixed
//! IDT stores all execute, so the guest now genuinely reaches POST 1.
//!
//! The assertions pin the observed byte stream and the stop *kind*
//! (`Unsupported`), never an address — NASM output shifts stop addresses with the
//! toolchain, so the faulting address lives only in the diagnostic `detail` string
//! and this comment.
//!
//! ## Pinned inputs
//!
//! - test386 image: `barotto/test386.asm` @
//!   `cfd052d1e64d5375dea5a681c1eadeed64ceda2c` (built by `vendor/test386/fetch.sh`).
//! - CPU: recorded against `x86jit-core` rev
//!   `3ff42a5265ee8f40913615d65823b16dffbc4f49`; it still passes at the rev now
//!   pinned in `Cargo.lock`.
//!   The stop point is only meaningful against a known CPU revision; re-baseline
//!   the golden if `SAHF` (or a nearer instruction) gains real-mode support.

use cli::{run_test386, StopReason};
use std::path::PathBuf;

/// Absolute path to the (gitignored) built test386 image.
fn image_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/test386/test386.bin")
}

/// The observed POST checkpoint stream: `POST 0` then `POST 1`, before execution
/// stops on the unlifted `SAHF` in the POST 1 conditional-jump test. Golden the
/// exact stream.
const GOLDEN_POST: &[u8] = &[0x00, 0x01];

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

    // End-to-end proof of the plumbing: reset → far jump → real-mode exec → POST
    // capture. The stream must reach at least POST 1 (`0x01`).
    assert!(
        run.post.contains(&0x01),
        "expected the POST stream to reach at least 0x01, got {:02x?}",
        run.post
    );

    // The observed POST byte stream on this CPU revision. Pinned exactly.
    assert_eq!(
        run.post, GOLDEN_POST,
        "POST checkpoint stream drifted from the recorded golden"
    );

    // Assert the stop KIND only — the address is toolchain-dependent and lives in
    // the detail string for the report, never in an assertion.
    assert!(
        matches!(run.stop, StopReason::Unsupported { .. }),
        "expected an unsupported-instruction stop (SAHF in the POST 1 \
         conditional-jump test), got {:?}",
        run.stop
    );
}
