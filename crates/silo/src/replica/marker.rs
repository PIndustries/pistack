// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// What a marker says about the file at a key path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Marker {
    pub sha256: String,
    pub key: String,
}
