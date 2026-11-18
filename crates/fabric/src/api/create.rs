// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
pub struct Create<S> {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    /// Admins may create in any project (or none); others inherit scope.
    #[serde(default)]
    pub project: Option<Id>,
    pub spec: S,
}
