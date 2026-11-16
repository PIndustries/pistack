// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(in crate::controllers::pod) struct SlabCondition {
    #[serde(rename = "type", default)]
    pub(in crate::controllers::pod) kind: String,
    #[serde(default)]
    pub(in crate::controllers::pod) status: bool,
}
