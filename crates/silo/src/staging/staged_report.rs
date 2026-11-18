// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// What one background pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StagedReport {
    /// Parts fetched from a peer.
    pub fetched: usize,
    /// Uploads removed because a peer saw them finish.
    pub removed: usize,
}
