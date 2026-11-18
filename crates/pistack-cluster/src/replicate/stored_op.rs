// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredOp {
    pub table: String,
    pub key: String,
    /// None deletes the key.
    pub value: Option<Vec<u8>>,
}
