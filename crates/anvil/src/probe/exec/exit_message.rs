// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::exec::scrub_probe_text::scrub_probe_text;

pub(in crate::probe) fn exit_message(exit_code: i32, output: &str) -> String {
    let output = scrub_probe_text(output);
    if output.is_empty() {
        format!("exit {exit_code}")
    } else {
        format!("exit {exit_code}: {output}")
    }
}
