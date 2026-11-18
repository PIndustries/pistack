// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::xml_unescape::xml_unescape;

pub(crate) fn normalize_key(raw: &str) -> String {
    xml_unescape(raw.trim())
}
