// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Phase`: the lifecycle phase a workload, volume or balancer reports.

use serde::{Deserialize, Serialize};

/// Lifecycle phase shared by workloads, volumes, balancers and anything
/// else that is "being made", "ready" or "broken".
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Pending,
    Creating,
    Ready,
    Updating,
    Deleting,
    Error,
}
