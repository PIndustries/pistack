// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The result of one probe.

/// Result of one probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    Up,
    Down(String),
}
