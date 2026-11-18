// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// How DELETE behaves for a kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteMode {
    /// Remove from the store now.
    Direct,
    /// Set `DELETE_ANNOTATION`; a controller finishes the job.
    Annotate,
}
