// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// What one pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Objects whose bytes were fetched from a peer.
    pub fetched: usize,
    /// Objects still without bytes after asking every peer.
    pub missing: usize,
    /// Files removed because their record is gone.
    pub pruned: usize,
}
