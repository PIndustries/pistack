// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::combined::combined;

pub(in crate::runtime::volfs) fn already_detached(output: &std::process::Output) -> bool {
    let text = combined(output);
    text.contains("no such device")
        || text.contains("no such file")
        || text.contains("not a loop device")
}
