// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `ParseError`: a syntax error with the column it was found at.

/// A syntax error with the 1-based column it was found at.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("column {column}: {message}")]
pub struct ParseError {
    pub column: usize,
    pub message: String,
}

impl ParseError {
    pub(super) fn new(column: usize, message: impl Into<String>) -> Self {
        Self {
            column,
            message: message.into(),
        }
    }
}
