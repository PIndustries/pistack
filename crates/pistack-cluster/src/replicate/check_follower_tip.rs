// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::group::ClusterError;

/// Check a follower's tip before serving it entries after `index`.
/// `term` is the follower's term at `index` (0 when it has no row there),
/// `commit` is our commit index, and `ours` our term at `index`. A follower
/// past our commit, or whose term at `index` differs from ours, holds entries
/// this log never had. Error text contains "diverged".
pub fn check_follower_tip(
    index: u64,
    term: u64,
    commit: u64,
    ours: Option<u64>,
) -> Result<(), ClusterError> {
    if index > commit {
        return Err(ClusterError(format!(
            "diverged: follower is at index {index}, leader commit is {commit}"
        )));
    }
    match ours {
        Some(ours) if term != 0 && ours != term => Err(ClusterError(format!(
            "diverged at index {index}: follower term {term}, leader term {ours}"
        ))),
        _ => Ok(()),
    }
}
