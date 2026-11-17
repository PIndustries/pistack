// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `PolicyDocument`: the serialised form of a policy.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Serialised form of a policy. `Policy` itself keeps parsed expressions,
/// so parsing happens once at load and evaluation never fails on syntax.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyDocument {
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "expr_map"
    )]
    pub rules: BTreeMap<String, String>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "expr_map"
    )]
    pub actions: BTreeMap<String, String>,
}

/// Accept `"x:y": true` as well as `"x:y": "true"`, since YAML parses a bare
/// `true` as a boolean and that is the obvious thing to write.
fn expr_map<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Scalar {
        Text(String),
        Bool(bool),
    }
    let raw: BTreeMap<String, Scalar> = BTreeMap::deserialize(d)?;
    Ok(raw
        .into_iter()
        .map(|(k, v)| {
            let s = match v {
                Scalar::Text(s) => s,
                Scalar::Bool(b) => b.to_string(),
            };
            (k, s)
        })
        .collect())
}
