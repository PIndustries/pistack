// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn named(names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for n in names {
        let n = n.trim();
        if n.is_empty() || out.iter().any(|seen| seen == n) {
            continue;
        }
        out.push(n.to_owned());
    }
    out
}
