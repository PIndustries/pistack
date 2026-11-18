// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Condition;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IngressStatus {
    /// Addresses `relay` exposes the ingress on. Empty until relay realizes it.
    #[serde(default)]
    pub load_balancer: Vec<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}
