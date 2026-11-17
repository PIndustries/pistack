// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `TlsPaths`: resolved TLS file locations.

use std::path::PathBuf;

/// Resolved TLS file locations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlsPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub ca: PathBuf,
}
