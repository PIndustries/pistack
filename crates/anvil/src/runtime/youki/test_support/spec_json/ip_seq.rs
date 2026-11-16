// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::seq_count;

pub(crate) fn ip_seq(log: &str, seq: &[&str]) -> bool {
    seq_count(log, seq) > 0
}
