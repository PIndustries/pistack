// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Which files of an upload to take from a peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// The staging directory and `meta.json` only.
    Meta,
    /// That, and one part.
    Part(u32),
    /// That, and every part.
    All,
}
