// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use percent_encoding::utf8_percent_encode;

use super::form_urlencoded_iter::form_urlencoded_iter;
use super::query_value::QUERY_VALUE;

pub(crate) fn canonical_query(query: Option<&str>, presigned: bool) -> String {
    let Some(q) = query.filter(|s| !s.is_empty()) else {
        return String::new();
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (k, v) in form_urlencoded_iter(q) {
        if presigned && k.eq_ignore_ascii_case("X-Amz-Signature") {
            continue;
        }
        pairs.push((
            utf8_percent_encode(&k, QUERY_VALUE).to_string(),
            utf8_percent_encode(&v, QUERY_VALUE).to_string(),
        ));
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}
