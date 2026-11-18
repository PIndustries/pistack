// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct MigrateBody {
    pub(crate) host: String,
}
