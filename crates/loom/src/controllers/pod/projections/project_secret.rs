// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::ProjectedFiles;
use std::collections::BTreeMap;
/// A JSON object of strings becomes one key per field. Any other payload is
/// a single `data` entry (text, or base64 when the bytes are not text).
pub(in crate::controllers::pod) fn project_secret(bytes: &[u8]) -> ProjectedFiles {
    if let Ok(text) = std::str::from_utf8(bytes) {
        if let Ok(map) = serde_json::from_str::<BTreeMap<String, String>>(text) {
            if !map.is_empty() {
                return ProjectedFiles {
                    data: map,
                    binary_data: BTreeMap::new(),
                };
            }
        }
        let mut data = BTreeMap::new();
        data.insert("data".into(), text.to_owned());
        return ProjectedFiles {
            data,
            binary_data: BTreeMap::new(),
        };
    }
    use base64::{engine::general_purpose::STANDARD, Engine};
    let mut binary_data = BTreeMap::new();
    binary_data.insert("data".into(), STANDARD.encode(bytes));
    ProjectedFiles {
        data: BTreeMap::new(),
        binary_data,
    }
}
#[cfg(test)]
#[path = "project_secret_tests.rs"]
mod project_secret_tests;
