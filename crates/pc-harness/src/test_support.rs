// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared PC/AT golden helpers and fixture policy; see
//! `backlog/docs/design/repo-split.md` §2.4.

#[path = "../tests/support/at_games.rs"]
pub mod at_games;
#[path = "../tests/support/fat12.rs"]
pub mod fat12;
#[path = "../tests/support/fixtures.rs"]
pub mod fixtures;
#[path = "../tests/support/session.rs"]
pub mod session;

pub use fixtures::root;
