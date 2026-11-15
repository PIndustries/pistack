// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn has_seq(lines: &[String], seq: &[String]) -> bool {
    lines.windows(seq.len()).any(|window| window == seq)
}
