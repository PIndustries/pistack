// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::Deserialize;

#[derive(Deserialize, Default)]
pub(crate) struct ScopeQuery {
    #[serde(default)]
    pub(crate) project: Option<Id>,
    #[serde(default)]
    pub(crate) all: Option<bool>,
}
