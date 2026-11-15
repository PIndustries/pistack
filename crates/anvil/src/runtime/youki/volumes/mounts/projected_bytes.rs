// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::env::decode_b64;
use crate::runtime::youki::spec::Projected;

pub(in crate::runtime::youki) fn projected_bytes(files: &Projected, key: &str) -> Option<Vec<u8>> {
    if let Some(text) = files.data.get(key) {
        return Some(text.as_bytes().to_vec());
    }
    decode_b64(files.binary_data.get(key)?)
}
