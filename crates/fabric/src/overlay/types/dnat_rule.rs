// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;
use serde::Serialize;

/// A floating address DNATed to a guest fixed address inside the router namespace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnatRule {
    pub id: String,
    pub floating: String,
    pub fixed: String,
}
