// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::replica::CONNECT_TIMEOUT;
use std::time::Duration;

/// `POST /silo/peer/uploads/...`: ask the peer to take this upload's
/// `meta.json` (`part` is `None`) or one part, and wait up to `wait` for
/// it to say it is done.
pub(crate) async fn ask_peer(
    client: &Client,
    base: &str,
    bucket: &str,
    upload_id: &str,
    part: Option<u32>,
    wait: Duration,
) -> Result<(), String> {
    let mut path = format!("/silo/peer/uploads/{bucket}/{upload_id}");
    if let Some(n) = part {
        path.push_str(&format!("/{n}"));
    }
    let key = client.peer_key();
    let peers = client.peers().ok_or("no peers")?;
    let sent = peers.send(base, CONNECT_TIMEOUT, |client, base| {
        let req = client.post(format!("{}{path}", base.trim_end_matches('/')));
        match &key {
            Some(key) => req.header(keep::peer::PEER_HEADER, key),
            None => req,
        }
    });
    match tokio::time::timeout(wait, sent).await {
        Ok(Ok(resp)) if resp.status().is_success() => Ok(()),
        Ok(Ok(resp)) => Err(format!("status {}", resp.status().as_u16())),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(format!("no answer in {}s", wait.as_secs())),
    }
}
