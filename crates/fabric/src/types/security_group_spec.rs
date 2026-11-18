// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

use super::rule::Rule;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SecurityGroupSpec {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
}
