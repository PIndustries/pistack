// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn assert_sudo_op(lines: &[String], youki: &str, op: &str) {
    let found = lines.windows(2).enumerate().any(|(i, w)| {
        if w[0] != "-n" || w[1] != youki {
            return false;
        }
        lines[i + 2..]
            .iter()
            .take_while(|line| line.as_str() != "-n")
            .any(|line| line == op)
    });
    assert!(found, "missing sudo {op}: {lines:?}");
}
