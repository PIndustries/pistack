// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::staging::Staged;

use super::get::get;

/// `GET /silo/peer/uploads/{bucket}/{upload}`: one upload the peer stages.
pub(crate) async fn fetch_upload(
    client: &Client,
    base: &str,
    bucket: &str,
    upload_id: &str,
) -> Result<Staged, String> {
    let path = format!("/silo/peer/uploads/{bucket}/{upload_id}");
    let resp = get(client, base, &path).await?;
    resp.json::<Staged>().await.map_err(|e| e.to_string())
}
