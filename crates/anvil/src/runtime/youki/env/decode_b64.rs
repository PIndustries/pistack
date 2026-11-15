// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn decode_b64(text: &str) -> Option<Vec<u8>> {
    use base64::{
        engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
        Engine,
    };
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return None;
    }
    STANDARD
        .decode(compact.as_bytes())
        .or_else(|_| STANDARD_NO_PAD.decode(compact.as_bytes()))
        .ok()
}
