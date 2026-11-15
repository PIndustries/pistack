// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn count_rbd(inv: &[Vec<String>], first: &str, second: &str) -> usize {
    inv.iter()
        .filter(|args| {
            args.first().map(String::as_str) == Some(first)
                && args.get(1).map(String::as_str) == Some(second)
        })
        .count()
}
