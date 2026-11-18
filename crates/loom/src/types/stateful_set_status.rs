// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Condition;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct StatefulSetStatus {
    #[serde(default)]
    pub replicas: u32,
    #[serde(default)]
    pub ready: u32,
    /// Pods whose template hash matches the current spec.
    #[serde(default)]
    pub current: u32,
    #[serde(default)]
    pub updated: u32,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}
