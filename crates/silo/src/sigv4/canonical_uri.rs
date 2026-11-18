// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use percent_encoding::utf8_percent_encode;

use super::path_segment::PATH_SEGMENT;

/// URI-encode each path segment per S3 SigV4 rules; keep `/` separators.
pub fn canonical_uri(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return "/".to_owned();
    }
    let trailing = path.ends_with('/');
    let mut out = String::from("/");
    let mut first = true;
    for seg in path.trim_start_matches('/').split('/') {
        if seg.is_empty() {
            continue;
        }
        if !first {
            out.push('/');
        }
        first = false;
        out.push_str(&utf8_percent_encode(seg, PATH_SEGMENT).to_string());
    }
    if trailing {
        out.push('/');
    }
    out
}
