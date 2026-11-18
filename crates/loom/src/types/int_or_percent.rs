// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

/// `3` or `"25%"`, as in Kubernetes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum IntOrPercent {
    Int(u32),
    Percent(String),
}

impl IntOrPercent {
    /// Resolve against `total`. Percent strings are `"<n>%"`; anything else
    /// is treated as 0.
    pub fn resolve(&self, total: u32, round_up: bool) -> u32 {
        match self {
            IntOrPercent::Int(n) => *n,
            IntOrPercent::Percent(s) => {
                let pct: u64 = s
                    .trim()
                    .strip_suffix('%')
                    .and_then(|n| n.trim().parse().ok())
                    .unwrap_or(0);
                let num = pct * total as u64;
                let v = if round_up {
                    num.div_ceil(100)
                } else {
                    num / 100
                };
                v.min(u32::MAX as u64) as u32
            }
        }
    }
}
