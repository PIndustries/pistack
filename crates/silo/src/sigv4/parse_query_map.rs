// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;

use super::form_urlencoded_iter::form_urlencoded_iter;

pub(crate) fn parse_query_map(query: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (k, v) in form_urlencoded_iter(query) {
        // First wins for auth fields; last is fine for our use.
        map.insert(k, v);
    }
    map
}
