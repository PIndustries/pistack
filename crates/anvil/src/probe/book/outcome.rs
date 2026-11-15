// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub success: bool,
    pub unsupported: bool,
    pub message: String,
}
