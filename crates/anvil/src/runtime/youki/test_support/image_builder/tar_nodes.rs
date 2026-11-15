// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::tar_header_full;

pub(crate) fn tar_nodes(entries: &[(&str, &[u8], u8, u32, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    for (name, data, flag, mode, link) in entries {
        buf.extend_from_slice(&tar_header_full(
            name,
            data.len() as u64,
            *flag,
            *mode,
            link,
        ));
        buf.extend_from_slice(data);
        let pad = (512 - (data.len() % 512)) % 512;
        buf.extend(std::iter::repeat_n(0u8, pad));
    }
    buf.extend(std::iter::repeat_n(0u8, 1024));
    buf
}
