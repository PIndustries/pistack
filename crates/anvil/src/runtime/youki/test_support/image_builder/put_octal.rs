// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn put_octal(dst: &mut [u8], val: u64) {
    let text = format!("{val:o}");
    assert!(text.len() < dst.len());
    dst[..text.len()].copy_from_slice(text.as_bytes());
    dst[text.len()] = 0;
}
