// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::PathBuf;

pub(crate) struct ConcatState {
    pub(crate) parts: std::vec::IntoIter<PathBuf>,
    pub(crate) current: Option<tokio::fs::File>,
    pub(crate) buf: Vec<u8>,
}
