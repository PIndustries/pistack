// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::probe) fn combine_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout.to_owned(),
        (true, false) => stderr.to_owned(),
        (false, false) => format!("{stderr}\n{stdout}"),
    }
}
