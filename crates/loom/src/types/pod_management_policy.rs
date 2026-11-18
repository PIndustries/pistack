// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

/// How a StatefulSet creates and deletes pods.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum PodManagementPolicy {
    /// Create/delete one ordinal at a time, waiting for Ready.
    #[default]
    OrderedReady,
    /// Create/delete every ordinal in one pass.
    Parallel,
}
