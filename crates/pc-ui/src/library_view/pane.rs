// SPDX-License-Identifier: AGPL-3.0-or-later
//! The library's details panes: the shared layout is [`ui_kit::pane`] and the
//! PC firmware pieces of a pane are this crate's [`crate::pane`], both
//! re-exported here under the path the library views name them by.

pub(crate) use crate::pane::*;
