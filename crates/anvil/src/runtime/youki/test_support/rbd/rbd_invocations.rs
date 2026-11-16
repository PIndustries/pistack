// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn rbd_invocations(log: &str) -> Vec<Vec<String>> {
    log.split("\n\n")
        .map(|block| {
            block
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|block| !block.is_empty())
        .collect()
}
