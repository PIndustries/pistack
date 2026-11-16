// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::clip::clip;

pub(in crate::runtime::volfs) fn brief(output: &std::process::Output) -> String {
    let err = String::from_utf8_lossy(&output.stderr);
    let err = err.trim();
    if !err.is_empty() {
        return clip(err);
    }
    let out = String::from_utf8_lossy(&output.stdout);
    let out = out.trim();
    if !out.is_empty() {
        return clip(out);
    }
    let code = output.status.code().unwrap_or(-1);
    format!("exit {code}")
}
