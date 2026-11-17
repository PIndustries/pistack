// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! How a route's path is compared with the request path.

use serde::{Deserialize, Serialize};

/// How a route's path is compared with the request path.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum PathMatch {
    /// Whole path segments: `/app` matches `/app` and `/app/x`, not `/apps`.
    #[default]
    Prefix,
    /// The request path must equal `path`.
    Exact,
}
