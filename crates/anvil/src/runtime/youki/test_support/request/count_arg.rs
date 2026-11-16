// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn count_arg(lines: &[String], arg: &str) -> usize {
    lines.iter().filter(|l| l.as_str() == arg).count()
}
