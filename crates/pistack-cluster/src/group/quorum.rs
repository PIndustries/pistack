// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Majority of `voters`, at least 1.
pub fn quorum(voters: usize) -> usize {
    voters / 2 + 1
}
