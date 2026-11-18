// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;

pub(crate) fn nonempty_annotation<'a>(m: &'a Machine, key: &str) -> Option<&'a str> {
    m.meta
        .annotations
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}
