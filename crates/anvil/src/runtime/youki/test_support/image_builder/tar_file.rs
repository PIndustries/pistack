// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::tar_header;

pub(crate) fn tar_file(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Vec::new();
    for (name, data) in entries {
        buf.extend_from_slice(&tar_header(name, data.len() as u64, b'0'));
        buf.extend_from_slice(data);
        let pad = (512 - (data.len() % 512)) % 512;
        buf.extend(std::iter::repeat_n(0u8, pad));
    }
    buf.extend(std::iter::repeat_n(0u8, 1024));
    buf
}
