// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::int_or_percent::IntOrPercent;
use super::quarter::quarter;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum DeploymentStrategy {
    RollingUpdate {
        #[serde(default = "quarter")]
        max_surge: IntOrPercent,
        #[serde(default = "quarter")]
        max_unavailable: IntOrPercent,
    },
    Recreate,
}

impl Default for DeploymentStrategy {
    fn default() -> Self {
        DeploymentStrategy::RollingUpdate {
            max_surge: quarter(),
            max_unavailable: quarter(),
        }
    }
}
