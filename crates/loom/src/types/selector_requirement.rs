// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::selector_operator::SelectorOperator;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectorRequirement {
    pub key: String,
    pub operator: SelectorOperator,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}
