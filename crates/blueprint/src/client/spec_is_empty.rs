// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde_json::Value;

pub(crate) fn spec_is_empty(spec: &Value) -> bool {
    spec.is_null() || spec.as_object().is_some_and(|m| m.is_empty())
}
