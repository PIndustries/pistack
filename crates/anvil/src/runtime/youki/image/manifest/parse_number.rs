// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::parse_octal;

pub(in crate::runtime::youki) fn parse_number(bytes: &[u8]) -> Option<u64> {
    if bytes.first().is_some_and(|b| b & 0x80 != 0) {
        let mut n: u128 = 0;
        for (i, b) in bytes.iter().enumerate() {
            let v = if i == 0 { b & 0x7f } else { *b };
            n = (n << 8) | u128::from(v);
            if n > u128::from(u64::MAX) {
                return None;
            }
        }
        return Some(n as u64);
    }
    parse_octal(bytes)
}
