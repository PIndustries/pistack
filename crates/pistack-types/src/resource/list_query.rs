// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `ListQuery`: the common list query parameters.

use serde::Deserialize;
use std::collections::BTreeMap;

/// Common list query parameters.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub after: Option<String>,
    /// `labels=env=prod,tier=web`
    #[serde(default)]
    pub labels: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl ListQuery {
    pub fn label_selector(&self) -> BTreeMap<String, String> {
        self.labels
            .as_deref()
            .unwrap_or("")
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .collect()
    }
}
