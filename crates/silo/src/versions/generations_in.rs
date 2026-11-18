// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::Object;
use pistack_types::{ApiResult, Id};

/// Every generation of every key in the bucket, key order first and newest
/// generation first within a key — the order S3 lists versions in. The current
/// generation of a key is its newest one, marker included, so `latest` is
/// answered off the version rather than the flag a put leaves behind.
pub(crate) fn generations_in(c: &Client, bucket_id: &Id) -> ApiResult<Vec<Object>> {
    let mut all = c.list_all_objects(bucket_id)?;
    all.sort_by(|a, b| {
        a.spec
            .key
            .cmp(&b.spec.key)
            .then(b.status.version.cmp(&a.status.version))
    });
    Ok(all)
}
