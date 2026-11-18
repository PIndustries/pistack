// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct RollbackBody {
    /// Revision to roll back to; the previous one when omitted.
    #[serde(default)]
    pub revision: Option<u64>,
}
