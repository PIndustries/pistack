// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Find `\r\n` with incremental start; returns index of `\r`.
pub(in crate::chunked) fn find_crlf(buf: &[u8], from: usize) -> Option<usize> {
    let start = from.saturating_sub(1);
    buf[start..]
        .windows(2)
        .position(|w| w == b"\r\n")
        .map(|p| start + p)
}
