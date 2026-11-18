// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::percent_decode::percent_decode;

/// Decode `application/x-www-form-urlencoded` pairs without treating `+` as
/// space for keys that are already percent-encoded the SigV4 way. We still
/// accept `+` as space for ordinary query values (S3 clients percent-encode).
pub(crate) fn form_urlencoded_iter(query: &str) -> impl Iterator<Item = (String, String)> + '_ {
    query.split('&').filter(|p| !p.is_empty()).map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (percent_decode(k), percent_decode(v))
    })
}
