// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `UpdateStack`: the PUT /stacks/{id} body.

use crate::types::StackSpec;
use serde::Deserialize;
use std::collections::BTreeMap;

/// `PUT /stacks/{id}`. Omitted fields are left as they are.
#[derive(Deserialize)]
pub struct UpdateStack {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub spec: Option<StackSpec>,
}
