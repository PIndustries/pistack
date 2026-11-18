// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;

use super::get::get;

/// `GET /silo/peer/uploads/{bucket}/{upload}/{part}`: one part's bytes.
pub(crate) async fn fetch_part(
    client: &Client,
    base: &str,
    bucket: &str,
    upload_id: &str,
    number: u32,
) -> Result<reqwest::Response, String> {
    let path = format!("/silo/peer/uploads/{bucket}/{upload_id}/{number}");
    get(client, base, &path).await
}
