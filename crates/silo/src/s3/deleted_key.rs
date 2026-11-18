// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// One `<Deleted>` row of a DeleteObjects answer.
pub(crate) struct DeletedKey {
    pub(crate) key: String,
    /// True when the key is now hidden behind a delete marker rather than gone.
    pub(crate) marker: bool,
}
