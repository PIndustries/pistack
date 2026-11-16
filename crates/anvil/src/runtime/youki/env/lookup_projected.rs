// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::env::decode_b64;
use crate::runtime::youki::spec::Projected;

pub(in crate::runtime::youki) fn lookup_projected(
    projected: &Projected,
    key: &str,
) -> Option<String> {
    if let Some(text) = projected.data.get(key) {
        return Some(text.clone());
    }
    let bytes = decode_b64(projected.binary_data.get(key)?)?;
    String::from_utf8(bytes).ok()
}
