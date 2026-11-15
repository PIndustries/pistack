// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::consts::COMMAND_RECORD_PREFIX;

pub(crate) fn record_key(node: &str, key: &str) -> String {
    format!("{COMMAND_RECORD_PREFIX}{node}/{key}")
}
