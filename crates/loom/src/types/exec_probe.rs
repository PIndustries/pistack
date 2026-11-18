// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExecProbe {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
}
