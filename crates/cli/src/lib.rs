// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `upcli` command line (§3.1): argument parsing, machine runs, and the
//! image and media commands.
//!
//! [`cli_core`] is the platform-neutral command line with its provider seam,
//! [`cli_pc`] the PC's part of it, and [`command`] the program that composes
//! them (`backlog/docs/design/core-contract.md` §7.1 and §7.2).
//! `--boot`, `--bios` and `run --machine` call the machine runners of the
//! `pc-harness` crate, the same runners its golden tests pin, so the command
//! line and the goldens cannot drift apart.

pub mod about;
pub mod cli_core;
pub mod cli_pc;
pub mod command;
/// The shared preferences file lives beside the store's other host locations.
pub use image_store::location::preferences;
/// Image creation lives with the PC provider, which the desktop calls too.
pub use pc_image::media_create as create;

pub use cli_core::image::registry::{registry_client, registry_probe_client};
pub use cli_pc::image::build_templates;
pub use platforms::{push_plan, with_image_providers};
