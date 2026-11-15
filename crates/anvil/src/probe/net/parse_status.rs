// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::probe) fn parse_status(buf: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(buf).ok()?;
    let line = text.lines().next()?;
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}
