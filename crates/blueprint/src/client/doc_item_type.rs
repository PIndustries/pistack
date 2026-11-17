// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde_json::Value;

/// One resource object in a stack document.
pub(crate) struct DocItem {
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) spec: Value,
}
