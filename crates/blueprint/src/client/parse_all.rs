// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use serde::Deserialize;
use serde_json::Value;

/// The document as a stream of values, read the way apply reads it.
pub(crate) fn parse_all(document: &str) -> ApiResult<Vec<Value>> {
    let mut docs = Vec::new();
    for doc in serde_yaml_ng::Deserializer::from_str(document) {
        let value = serde_json::Value::deserialize(doc)
            .map_err(|e| ApiError::bad_request(format!("document is not valid YAML: {e}")))?;
        docs.push(value);
    }
    Ok(docs)
}
