// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Result of [`migrate_to_vault`](super::migrate_to_vault) or [`migrate_to_software`](super::migrate_to_software).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migrated {
    /// Transit mount and key involved.
    pub mount: String,
    pub key: String,
    /// Transit key version of the wrapped key (to vault) or of the one
    /// that was removed (to software).
    pub key_version: Option<u32>,
    /// True when the store already held both keys and this only removed
    /// the stale copy.
    pub resumed: bool,
}
