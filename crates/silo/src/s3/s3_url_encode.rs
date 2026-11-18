// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

/// S3 `encoding-type=url`: percent-encode UTF-8 like `urllib.parse.quote(s, safe="/")`
/// (botocore unquotes with `urllib.parse.unquote`). `/` is left intact.
const S3_URL_ENCODE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

pub(crate) fn s3_url_encode(s: &str) -> String {
    utf8_percent_encode(s, S3_URL_ENCODE).to_string()
}
