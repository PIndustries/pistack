// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Associate {
    /// Existing port. Exactly one of `port` or `machine` is set.
    #[serde(default)]
    pub port: Option<Id>,
    /// Machine whose current IPv4 becomes the fixed address.
    #[serde(default)]
    pub machine: Option<Id>,
}
