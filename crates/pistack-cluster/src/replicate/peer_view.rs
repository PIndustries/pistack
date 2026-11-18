// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// What a peer said about itself on `/cluster`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerView {
    pub api: String,
    pub term: u64,
    pub leader: bool,
}
