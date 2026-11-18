// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `upcli` program (§3.1): the platform-neutral command line of
//! `cli-core` with the platforms compiled in, which today is the PC's
//! (`cli-pc`), and the program's own `--version` and `--about`
//! (`backlog/docs/design/core-contract.md` §7.1). [`command`] composes them;
//! the executable and the tests that pin the composed command line come
//! through it.

pub mod about;
pub mod command;
