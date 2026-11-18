// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// True when `name` is `zone` or ends in `.<zone>`. Both absolute.
pub fn in_zone(zone: &str, name: &str) -> bool {
    name == zone || name.ends_with(&format!(".{zone}"))
}
