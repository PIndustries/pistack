// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Find `\r\n\r\n` starting search near `from`; returns end index after terminator.
pub(in crate::chunked) fn find_double_crlf(buf: &[u8], from: usize) -> Option<usize> {
    let start = from.saturating_sub(3);
    let mut i = start;
    while i + 3 < buf.len() {
        if &buf[i..i + 4] == b"\r\n\r\n" {
            return Some(i + 4);
        }
        i += 1;
    }
    None
}
