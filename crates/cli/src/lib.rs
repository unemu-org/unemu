// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `upcli` command line (§3.1): argument parsing, machine runs, and the
//! image and media commands.
//!
//! `--boot`, `--bios` and `run --machine` call the machine runners of the
//! `pc-harness` crate, the same runners its golden tests pin, so the command
//! line and the goldens cannot drift apart.

pub mod about;
pub mod command;
mod image;
mod recipe;
/// The shared preferences file lives beside the store's other host locations.
pub use image_store::location::preferences;
/// Image creation lives with the PC provider, which the desktop calls too.
pub use pc_image::media_create as create;

pub use image::{
    build_templates, push_plan, registry_client, registry_probe_client, with_image_providers,
};

// The runners and stop report the commands use, reached as `crate::` paths.
use pc_harness::{
    classify_bios_stop, finish_persistence, run_bios_file, run_boot, StopReason, BIOS_IDLE_LIMIT,
    BIOS_MAX_RETIRED, BOOT_PROGRAM,
};
