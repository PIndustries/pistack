// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// What the bytes say they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectedFormat {
    Qcow2,
    /// No recognised header; raw disks have none.
    Raw,
}
