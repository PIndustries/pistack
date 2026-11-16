// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn utf8_prefix_end(buf: &[u8]) -> usize {
    if std::str::from_utf8(buf).is_ok() {
        return buf.len();
    }
    let mut end = buf.len();
    while end > 0 && (buf[end - 1] & 0b1100_0000) == 0b1000_0000 {
        end -= 1;
    }
    if end > 0 {
        let lead = buf[end - 1];
        let width = match lead {
            0x00..=0x7F => 1,
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => 0,
        };
        let have = buf.len() - (end - 1);
        if width == 0 || have < width {
            end -= 1;
        }
    }
    end
}
