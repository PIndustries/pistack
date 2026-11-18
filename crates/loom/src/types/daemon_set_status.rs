// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Condition;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DaemonSetStatus {
    /// Eligible hosts the controller wants a pod on.
    #[serde(default)]
    pub desired: u32,
    /// Owned pods that are not being deleted.
    #[serde(default)]
    pub current: u32,
    #[serde(default)]
    pub ready: u32,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}
