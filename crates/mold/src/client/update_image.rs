// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;
use std::collections::BTreeMap;

use crate::types::*;

/// Body of `PUT /images/{id}`: metadata and the mutable spec fields.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UpdateImage {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub min_disk_gib: Option<u64>,
    #[serde(default)]
    pub min_ram_mib: Option<u64>,
    #[serde(default)]
    pub visibility: Option<Visibility>,
}
