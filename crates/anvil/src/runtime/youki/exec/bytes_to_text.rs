// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Keep a trailing partial UTF-8 sequence out of the lossy conversion so a
/// cap that lands mid-character does not invent a replacement character.
use crate::runtime::youki::exec::utf8_prefix_end;

pub(in crate::runtime::youki) fn bytes_to_text(buf: &[u8]) -> String {
    let end = utf8_prefix_end(buf);
    String::from_utf8_lossy(&buf[..end]).into_owned()
}
