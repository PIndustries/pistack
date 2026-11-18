// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::Client;
use pistack_types::{ApiResult, Id};

pub(crate) fn resolve_network(
    client: &Client,
    project: &Id,
    name: &str,
) -> ApiResult<fabric::Network> {
    match client.networks().resolve(Some(project), name) {
        Ok(n) => Ok(n),
        Err(pistack_store::StoreError::NotFound(_)) => {
            client.networks().resolve(None, name).map_err(Into::into)
        }
        Err(e) => Err(e.into()),
    }
}
