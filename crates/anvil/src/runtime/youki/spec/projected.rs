// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// File bodies carried on the sandbox document. `binary_data` is base64.
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Projected {
    #[serde(default)]
    pub(in crate::runtime::youki) data: BTreeMap<String, String>,
    #[serde(default)]
    pub(in crate::runtime::youki) binary_data: BTreeMap<String, String>,
}
