// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct ResizeBody {
    pub(crate) size: String,
}
