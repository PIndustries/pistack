// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::kinds;
use pistack_types::Spec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- kinds and well-known keys ------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConfigMapSpec {
    #[serde(default)]
    pub data: BTreeMap<String, String>,
    /// base64-encoded values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub binary_data: BTreeMap<String, String>,
    #[serde(default)]
    pub immutable: bool,
}

impl Spec for ConfigMapSpec {
    const KIND: &'static str = kinds::CONFIG_MAP;
}
