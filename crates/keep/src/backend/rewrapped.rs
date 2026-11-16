// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Result of [`rewrap`](super::rewrap).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rewrapped {
    pub from_version: Option<u32>,
    pub to_version: Option<u32>,
    /// False when the ciphertext was already on the latest version; the
    /// store is then left as it was.
    pub changed: bool,
}
