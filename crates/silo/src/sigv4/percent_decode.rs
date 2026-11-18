// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = percent_encoding::percent_decode_str(s).collect::<Vec<u8>>();
    String::from_utf8_lossy(&bytes).into_owned()
}
