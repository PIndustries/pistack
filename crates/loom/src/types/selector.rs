// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- kinds and well-known keys ------------------------------------------------

use super::selector_requirement::SelectorRequirement;

/// A Kubernetes-shaped label selector: every `match_labels` pair must match
/// and every expression must hold.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Selector {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub match_labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_expressions: Vec<SelectorRequirement>,
}
