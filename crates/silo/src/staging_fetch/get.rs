// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::replica::CONNECT_TIMEOUT;

use super::stall::STALL;

/// `GET` a path from the peer at `base`, peer key and all. An answer that
/// is not a success is an error.
pub(crate) async fn get(
    client: &Client,
    base: &str,
    path: &str,
) -> Result<reqwest::Response, String> {
    let key = client.peer_key();
    let peers = client.peers().ok_or("no peers")?;
    let sent = peers.send(base, CONNECT_TIMEOUT, |client, base| {
        let req = client.get(format!("{}{path}", base.trim_end_matches('/')));
        match &key {
            Some(key) => req.header(keep::peer::PEER_HEADER, key),
            None => req,
        }
    });
    let resp = tokio::time::timeout(STALL, sent)
        .await
        .map_err(|_| "no answer".to_owned())??;
    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status().as_u16()));
    }
    Ok(resp)
}
