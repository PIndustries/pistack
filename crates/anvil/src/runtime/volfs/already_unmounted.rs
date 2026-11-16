// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::combined::combined;

pub(in crate::runtime::volfs) fn already_unmounted(output: &std::process::Output) -> bool {
    if output.status.code() == Some(32) {
        return true;
    }
    let text = combined(output);
    text.contains("not mounted") || text.contains("no mount point")
}
