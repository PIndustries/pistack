// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `WorkflowSpec`: an ordered list of stack names in one project.
use pistack_types::Spec;
use serde::{Deserialize, Serialize};

use super::workflow_kind::WORKFLOW_KIND;

/// An ordered list of stack names in one project.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowSpec {
    /// Name inside the spec, independent of the resource name.
    #[serde(default)]
    pub name: String,
    /// Stack names or ids, applied in this order.
    #[serde(default)]
    pub stacks: Vec<String>,
}

impl Spec for WorkflowSpec {
    const KIND: &'static str = WORKFLOW_KIND;
}
