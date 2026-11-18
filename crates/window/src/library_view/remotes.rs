// SPDX-License-Identifier: AGPL-3.0-or-later
//! The library's Remotes group is drawn by `library_ui::remotes`,
//! re-exported here under its old path. Its tests drive the whole library
//! frame and its worker, and stay with the frame.

pub(crate) use library_ui::remotes::*;

// What the tests took from the module's own imports when it lived here.
#[cfg(test)]
use super::{pane, LibraryView, Request};
#[cfg(test)]
use crate::{
    registry_transfer::Accounts,
    remote_browse::{
        Action, BrowseEvent, BrowseRequest, LocalState, RegistryView, Remotes, RepositoryRow,
        State, TagRecord, TagRow,
    },
};
#[cfg(test)]
use egui::{Color32, Rect};
#[cfg(test)]
use image_registry::{reference::OFFICIAL_NAMESPACE, DEFAULT_REGISTRY};
#[cfg(test)]
use image_store::media_type as mt;
#[cfg(test)]
use library_ui::remotes::test_support::{
    action_label, namespace_key, newest_first, tree_lines, type_label, COLUMNS, ROW,
};

#[cfg(test)]
mod tests;
