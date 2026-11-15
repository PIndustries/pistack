// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::volfs) fn first_token(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let token = text.split_whitespace().next()?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_owned())
    }
}
