// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// True when this process realises tenant networks through OVN.
pub fn enabled() -> bool {
    std::env::var("PISTACK_FABRIC_BACKEND").ok().as_deref() == Some("ovn")
}
