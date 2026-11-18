// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::group::ClusterError;

use super::stored_entry::StoredEntry;

/// Entries with index > have, sorted by index. Error if the first is not
/// have+1 or if two consecutive indexes are not adjacent. Error text contains "gap".
pub fn entries_after(have: u64, log: &[StoredEntry]) -> Result<Vec<StoredEntry>, ClusterError> {
    let mut next: Vec<StoredEntry> = log
        .iter()
        .filter(|entry| entry.index > have)
        .cloned()
        .collect();
    next.sort_by_key(|entry| entry.index);
    let mut previous = have;
    for entry in &next {
        if entry.index != previous + 1 {
            return Err(ClusterError("gap".into()));
        }
        previous = entry.index;
    }
    Ok(next)
}
