// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::Deserialize;

#[derive(Deserialize, Default)]
pub struct ScopeQuery {
    /// Admin-only: act in this project instead of the token's.
    #[serde(default)]
    pub project: Option<Id>,
    /// Admin-only: list every project.
    #[serde(default)]
    pub all: Option<bool>,
    /// Record sets only: restrict to one zone (name or id).
    #[serde(default)]
    pub zone: Option<String>,
}
