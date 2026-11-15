// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn arg_lines(log: &str) -> Vec<String> {
    log.lines()
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}
