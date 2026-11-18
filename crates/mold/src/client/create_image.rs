// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::types::*;

/// Body of `POST /images`.
#[derive(Clone, Debug, Deserialize)]
pub struct CreateImage {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    /// Admins may create on behalf of a project (or cloud-wide with `None`).
    #[serde(default)]
    pub project: Option<Id>,
    pub spec: ImageSpec,
}
