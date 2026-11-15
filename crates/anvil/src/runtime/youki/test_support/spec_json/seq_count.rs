// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn seq_count(log: &str, seq: &[&str]) -> usize {
    let lines: Vec<&str> = log.lines().filter(|line| !line.is_empty()).collect();
    lines
        .windows(seq.len())
        .filter(|window| *window == seq)
        .count()
}
