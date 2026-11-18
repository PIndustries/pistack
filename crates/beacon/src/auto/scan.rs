// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use pistack_types::ApiResult;

/// Every stored resource of `kind_`, decoded as `T`. Rows that do not
/// decode are skipped with a warning rather than failing the pass.
pub(crate) fn scan<T: serde::de::DeserializeOwned>(
    client: &Client,
    kind_: &str,
) -> ApiResult<Vec<T>> {
    let mut out = Vec::new();
    for (key, bytes) in client.store().raw_scan("resources", &format!("{kind_}/"))? {
        match serde_json::from_slice::<T>(&bytes) {
            Ok(v) => out.push(v),
            Err(e) => tracing::warn!(key, error = %e, "beacon auto: unreadable resource"),
        }
    }
    Ok(out)
}
