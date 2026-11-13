// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One server's calls about multipart staging to another.
//!
//! Every call here goes the way every call between control-plane servers
//! goes: through `keep::peer::PeerNet`, so it reaches the other server's
//! TLS listener when it has one, and carries the peer key. The path is
//! written against the address `PeerNet` chose, never against the address
//! the member list named.

use crate::client::Client;
use crate::replica::CONNECT_TIMEOUT;
use crate::staging::Staged;
use std::time::Duration;

/// A peer that sends nothing for this long is given up on.
const STALL: Duration = Duration::from_secs(30);

/// `GET /silo/peer/uploads`: everything the peer at `base` stages.
pub(crate) async fn list_staged(client: &Client, base: &str) -> Result<Staged, String> {
    let resp = get(client, base, "/silo/peer/uploads").await?;
    resp.json::<Staged>().await.map_err(|e| e.to_string())
}

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

/// `GET` a path from the peer at `base`, peer key and all. An answer that
/// is not a success is an error.
async fn get(client: &Client, base: &str, path: &str) -> Result<reqwest::Response, String> {
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
