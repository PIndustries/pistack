// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::form_urlencoded_iter::form_urlencoded_iter;

pub(crate) fn query_has(query: &str, key: &str) -> bool {
    form_urlencoded_iter(query).any(|(k, _)| k.eq_ignore_ascii_case(key))
}
