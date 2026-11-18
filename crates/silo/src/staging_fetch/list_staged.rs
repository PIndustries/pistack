// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::staging::Staged;

use super::get::get;

/// `GET /silo/peer/uploads`: everything the peer at `base` stages.
pub(crate) async fn list_staged(client: &Client, base: &str) -> Result<Staged, String> {
    let resp = get(client, base, "/silo/peer/uploads").await?;
    resp.json::<Staged>().await.map_err(|e| e.to_string())
}
