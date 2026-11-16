// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::overlay) fn refused_physnet(name: &str) -> bool {
    name.is_empty()
        || name == "wg-fleet"
        || name == "pistack0"
        || name == "simbr0"
        || name.starts_with("pi")
}
