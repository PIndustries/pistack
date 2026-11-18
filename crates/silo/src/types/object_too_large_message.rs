// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::format_byte_limit::format_byte_limit;

/// `object is larger than …` using [`format_byte_limit`].
pub fn object_too_large_message(max: usize) -> String {
    format!("object is larger than {}", format_byte_limit(max))
}
