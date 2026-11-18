// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::s3_url_encode::s3_url_encode;
use super::xml_escape::xml_escape;

/// XML-escape a list key-name field, optionally after `encoding-type=url`.
pub(crate) fn xml_list_value(s: &str, encode_url: bool) -> String {
    if encode_url {
        xml_escape(&s3_url_encode(s))
    } else {
        xml_escape(s)
    }
}
