// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::volume::Volume;

pub(crate) fn volume_names(volumes: &[Volume]) -> String {
    let mut names: Vec<&str> = volumes.iter().map(|v| v.meta.name.as_str()).collect();
    names.sort_unstable();
    names.join(", ")
}
