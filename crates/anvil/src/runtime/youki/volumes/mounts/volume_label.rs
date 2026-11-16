// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::SandboxVolume;

pub(in crate::runtime::youki) fn volume_label<'a>(vol: &'a SandboxVolume, id: &'a str) -> &'a str {
    if vol.name.is_empty() {
        id
    } else {
        vol.name.as_str()
    }
}
