// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What one listener's requests are matched against: its routes and its
//! default pool.

use crate::picker::Backend;

/// Where requests on one listener go: routes first, then the default pool.
#[derive(Default)]
pub(super) struct Table {
    pub(super) default: Vec<Backend>,
    pub(super) routes: Vec<RouteTarget>,
}

pub(super) struct RouteTarget {
    pub(super) host: Option<String>,
    pub(super) path: String,
    pub(super) exact: bool,
    pub(super) members: Vec<Backend>,
}
