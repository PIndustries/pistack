// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct ImportBody {
    pub(crate) url: String,
}
