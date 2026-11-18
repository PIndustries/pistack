// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::{Zone, ZoneKind, AUTO_LABEL};
use pistack_types::ApiResult;

/// Zones that opted in.
pub(crate) fn auto_zones(client: &Client) -> ApiResult<Vec<Zone>> {
    Ok(client
        .zones()
        .list(None)?
        .into_iter()
        .filter(|z| {
            z.spec.kind == ZoneKind::Primary
                && z.meta.labels.get(AUTO_LABEL).map(String::as_str) == Some("true")
        })
        .collect())
}
