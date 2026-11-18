// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

/// `GET /networks` and `GET /subnets` beside the usual list parameters.
#[derive(Deserialize, Default)]
pub struct NetFilter {
    /// Only what belongs to this vpc, by id or by name. `none` is what has
    /// no vpc at all.
    #[serde(default)]
    pub vpc: Option<String>,
}
