// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::parse_octal;

pub(in crate::runtime::youki) fn checksum_ok(header: &[u8]) -> bool {
    let Some(stored) = parse_octal(&header[148..156]) else {
        return false;
    };
    let mut sum = 0u64;
    for (i, b) in header.iter().enumerate() {
        if (148..156).contains(&i) {
            sum += b' ' as u64;
        } else {
            sum += *b as u64;
        }
    }
    stored == sum
}
