// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Whether a name is a legal `pw` plus 12 lowercase hex digits, and not a
//! reserved one.

pub(crate) fn is_pw(name: &str) -> bool {
    let Some(hex) = name.strip_prefix("pw") else {
        return false;
    };
    name != "wg-fleet"
        && !name.starts_with("pi")
        && hex.len() == 12
        && hex.chars().all(|c| c.is_ascii_hexdigit())
        && hex.chars().all(|c| !c.is_ascii_uppercase())
}
