// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The query string every handler reads: who to act as, and what to
//! restrict the list to.

use pistack_types::Id;
use serde::Deserialize;

#[derive(Deserialize, Default)]
pub(crate) struct ScopeQuery {
    /// Admin-only: act in this project instead of the token's.
    #[serde(default)]
    pub project: Option<Id>,
    /// Admin-only: list every project.
    #[serde(default)]
    pub all: Option<bool>,
    /// Listeners and pools: restrict to one balancer (name or id).
    #[serde(default)]
    pub balancer: Option<String>,
    /// `/members`: restrict to one pool (name or id).
    #[serde(default)]
    pub pool: Option<String>,
}
