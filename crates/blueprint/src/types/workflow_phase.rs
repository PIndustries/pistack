// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `WorkflowPhase`: a workflow's observed phase.
use serde::{Deserialize, Serialize};

/// Observed phase. Apply sets `Failed` when a stack returns an error.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    #[default]
    Accepted,
    Failed,
}
