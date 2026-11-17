// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `CreateWorkflow`: the POST /workflows body.

use crate::types::WorkflowSpec;
use pistack_types::Id;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
pub struct CreateWorkflow {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    #[serde(default)]
    pub project: Option<Id>,
    pub spec: WorkflowSpec,
}
