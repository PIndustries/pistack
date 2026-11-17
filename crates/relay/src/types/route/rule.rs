// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One host and path rule on an HTTP listener.

use super::path_match::PathMatch;
use crate::types::root_path::root_path;
use pistack_types::Id;
use serde::{Deserialize, Serialize};

/// One host and path rule on an HTTP listener.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Route {
    /// Exact host name, or `*.example.com` for any single leading label.
    /// Absent matches every host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Starts with `/`.
    #[serde(default = "root_path")]
    pub path: String,
    #[serde(default)]
    pub path_match: PathMatch,
    /// Pool on the same balancer that takes matching requests.
    pub pool: Id,
}
