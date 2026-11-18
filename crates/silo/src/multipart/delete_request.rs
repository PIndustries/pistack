// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// One `<Object>` of a DeleteObjects request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteRequest {
    pub key: String,
    /// `<Quiet>` inside the object overrides the request-level flag.
    pub quiet: Option<bool>,
}
