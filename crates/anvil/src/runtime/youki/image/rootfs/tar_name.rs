// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cstr;

pub(in crate::runtime::youki) fn tar_name(header: &[u8]) -> String {
    let name = cstr(&header[0..100]);
    let magic = &header[257..262];
    let prefix = cstr(&header[345..500]);
    if magic == b"ustar" && !prefix.is_empty() {
        format!("{prefix}/{name}")
    } else {
        name
    }
}
