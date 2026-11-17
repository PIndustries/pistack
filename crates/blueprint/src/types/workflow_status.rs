// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `WorkflowStatus`: a workflow's observed status, and the `Workflow` alias.
use serde::{Deserialize, Serialize};

use super::workflow_accepted_message::WORKFLOW_ACCEPTED_MESSAGE;
use super::workflow_phase::WorkflowPhase;
use super::workflow_spec::WorkflowSpec;
use pistack_types::Resource;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowStatus {
    pub phase: WorkflowPhase,
    pub message: String,
    /// Stack names applied by the last run before it stopped.
    #[serde(default)]
    pub applied: Vec<String>,
}

impl WorkflowStatus {
    pub fn accepted() -> Self {
        Self {
            phase: WorkflowPhase::Accepted,
            message: WORKFLOW_ACCEPTED_MESSAGE.to_owned(),
            applied: Vec::new(),
        }
    }
}

impl Default for WorkflowStatus {
    fn default() -> Self {
        Self::accepted()
    }
}

pub type Workflow = Resource<WorkflowSpec, WorkflowStatus>;
