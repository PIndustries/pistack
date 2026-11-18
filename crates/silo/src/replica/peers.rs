// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Object;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use super::connect_timeout::CONNECT_TIMEOUT;
use super::hex::hex;
use super::member::Member;
use super::own_member_id::own_member_id;
use super::push_wait::push_wait;
use super::source::Source;
use super::stall::STALL;

/// The other control-plane servers, as a source of object bytes.
#[derive(Clone)]
pub struct Peers {
    pub(crate) source: Source,
    /// Which address of a server is called and what its certificate is
    /// checked against.
    pub(crate) net: keep::peer::PeerNet,
    /// Where a fixed list reads the peer key from.
    pub(crate) key_from: Option<Store>,
}

impl Peers {
    /// The members of the cluster `store` belongs to, except this server.
    ///
    /// The list is read on every fetch, so a server that joins later is
    /// used without a restart. A store with no member list has no peers.
    /// Every request carries the peer key read from that store.
    pub fn from_cluster(store: Store) -> Self {
        Self {
            source: Source::Cluster {
                store: store.clone(),
                me: own_member_id(),
            },
            net: keep::peer::PeerNet::from_env(store),
            key_from: None,
        }
    }

    /// Peers at these API base URLs (`http://10.0.0.12:8080`).
    pub fn fixed(urls: Vec<String>) -> Self {
        Self {
            source: Source::Fixed(urls),
            net: keep::peer::PeerNet::plain(),
            key_from: None,
        }
    }

    /// Send the peer key of the cluster `store` belongs to. A list read
    /// from the cluster does this already.
    pub fn with_key_from(mut self, store: Store) -> Self {
        self.key_from = Some(store);
        self
    }

    pub(crate) fn peer_key(&self) -> Option<String> {
        match (&self.source, &self.key_from) {
            (Source::Cluster { store, .. }, _) => keep::peer::peer_key(store),
            (Source::Fixed(_), Some(store)) => keep::peer::peer_key(store),
            (Source::Fixed(_), None) => None,
        }
    }

    /// API base URLs to ask, the leader first.
    pub fn urls(&self) -> Vec<String> {
        self.targets().into_iter().map(|(api, _)| api).collect()
    }

    /// The peer key to send with a call, when this server has one.
    pub(crate) fn key(&self) -> Option<String> {
        self.peer_key()
    }

    /// Send one request to the peer at `base`, to its TLS listener when it
    /// has one. `request` builds it for the client and base address
    /// `PeerNet` chose.
    pub(crate) async fn send(
        &self,
        base: &str,
        connect_timeout: Duration,
        request: impl Fn(reqwest::Client, &str) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, String> {
        let tls = self
            .targets()
            .into_iter()
            .find(|(api, _)| api.trim_end_matches('/') == base.trim_end_matches('/'))
            .and_then(|(_, tls)| tls);
        self.net
            .send(base, tls.as_deref(), connect_timeout, request)
            .await
    }

    /// Each server to ask, the leader first: its API address and, when it
    /// has one, its TLS address.
    pub(crate) fn targets(&self) -> Vec<(String, Option<String>)> {
        match &self.source {
            Source::Fixed(urls) => urls.iter().map(|url| (url.clone(), None)).collect(),
            Source::Cluster { store, me } => {
                let members: Vec<Member> =
                    store.get_meta("members").ok().flatten().unwrap_or_default();
                let leader: Option<String> = store.get_meta("cluster_leader_id").ok().flatten();
                let mut others: Vec<Member> = members
                    .into_iter()
                    .filter(|m| !m.api.trim().is_empty() && Some(&m.id) != me.as_ref())
                    .collect();
                others.sort_by_key(|m| Some(&m.id) != leader.as_ref());
                others.into_iter().map(|m| (m.api, m.agent_api)).collect()
            }
        }
    }

    /// Have `wanted` other servers fetch the bytes of `obj` from this one.
    ///
    /// Each peer is asked in turn and answers once it holds bytes that
    /// match the checksum. Ok when there is no peer to ask: a cluster of
    /// one server has nowhere to put a second copy. `unavailable` when
    /// fewer than `wanted` took it and there were peers to ask.
    pub async fn push(&self, obj: &Object, wanted: usize) -> ApiResult<()> {
        let urls = self.targets();
        let wanted = wanted.min(urls.len());
        let wait = push_wait(obj.status.size_bytes);
        let mut taken = 0;
        let mut last = String::new();
        let key = self.peer_key();
        let path = format!("/silo/peer/pull/{}/{}", obj.meta.id, obj.status.sha256);
        for (base, tls) in &urls {
            if taken >= wanted {
                break;
            }
            // The same way as the fetch: at the TLS listener when the
            // server has one.
            let sent = self
                .net
                .send(base, tls.as_deref(), CONNECT_TIMEOUT, |client, base| {
                    let req = client.post(format!("{}{path}", base.trim_end_matches('/')));
                    match &key {
                        Some(key) => req.header(keep::peer::PEER_HEADER, key),
                        None => req,
                    }
                });
            match tokio::time::timeout(wait, sent).await {
                Ok(Ok(resp)) if resp.status().is_success() => taken += 1,
                Ok(Ok(resp)) => last = format!("{base}: status {}", resp.status().as_u16()),
                Ok(Err(e)) => last = format!("{base}: {e}"),
                Err(_) => last = format!("{base}: no answer in {}s", wait.as_secs()),
            }
        }
        if taken >= wanted {
            return Ok(());
        }
        tracing::warn!(object = %obj.meta.id, key = %obj.spec.key, %last, "silo: no second copy of a put");
        Err(ApiError::unavailable(format!(
            "object '{}' is stored on this server only: {} of {} other server(s) took a copy (last: {last}). \
             It is copied in the background; read before retrying",
            obj.spec.key,
            taken,
            urls.len()
        )))
    }

    /// Download the bytes with checksum `sha256` to `dest`.
    ///
    /// Each peer is tried in turn. The body goes to a temp file beside
    /// `dest` and is renamed only when its length and SHA-256 match, so a
    /// short or wrong answer never becomes the object. `not_found` when
    /// there is no peer to ask; `unavailable` when none of them had it.
    pub async fn fetch(&self, bucket: &Id, sha256: &str, size: u64, dest: &Path) -> ApiResult<()> {
        let urls = self.targets();
        if urls.is_empty() {
            return Err(ApiError::not_found("no other server holds object bytes"));
        }
        let mut last = String::new();
        for (base, tls) in &urls {
            let path = format!("/silo/peer/blobs/{bucket}/{sha256}");
            let from = (base.as_str(), tls.as_deref());
            match self.fetch_from(from, &path, sha256, size, dest).await {
                Ok(()) => {
                    tracing::debug!(%bucket, sha256, peer = %base, "silo: fetched object bytes");
                    return Ok(());
                }
                Err(e) => {
                    tracing::debug!(%bucket, sha256, peer = %base, error = %e, "silo: peer did not return the bytes");
                    last = format!("{base}: {e}");
                }
            }
        }
        Err(ApiError::unavailable(format!(
            "object bytes are not on this server and {} other server(s) did not return them (last: {last})",
            urls.len()
        )))
    }

    pub(crate) async fn fetch_from(
        &self,
        (api, tls): (&str, Option<&str>),
        path: &str,
        sha256: &str,
        size: u64,
        dest: &Path,
    ) -> Result<(), String> {
        let key = self.peer_key();
        let sent = self.net.send(api, tls, CONNECT_TIMEOUT, |client, base| {
            let req = client.get(format!("{base}{path}"));
            match &key {
                Some(key) => req.header(keep::peer::PEER_HEADER, key),
                None => req,
            }
        });
        let mut resp = tokio::time::timeout(STALL, sent)
            .await
            .map_err(|_| "no answer".to_owned())??;
        if !resp.status().is_success() {
            return Err(format!("status {}", resp.status().as_u16()));
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        let tmp = dest.with_file_name(format!(".tmp-{}", uuid::Uuid::now_v7()));
        let mut guard = crate::client::TmpGuard::new(tmp.clone());
        let mut file = tokio::fs::File::create(&tmp)
            .await
            .map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut got: u64 = 0;
        loop {
            let chunk = tokio::time::timeout(STALL, resp.chunk())
                .await
                .map_err(|_| "transfer stalled".to_owned())?
                .map_err(|e| e.to_string())?;
            let Some(chunk) = chunk else { break };
            got += chunk.len() as u64;
            if got > size {
                return Err(format!("more than the {size} bytes the record gives"));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        if got != size {
            return Err(format!("{got} bytes, the record gives {size}"));
        }
        if hex(hasher.finalize().as_slice()) != sha256 {
            return Err("checksum does not match the record".to_owned());
        }
        file.flush().await.map_err(|e| e.to_string())?;
        file.sync_all().await.map_err(|e| e.to_string())?;
        drop(file);
        tokio::fs::rename(&tmp, dest)
            .await
            .map_err(|e| e.to_string())?;
        guard.disarm();
        Ok(())
    }
}
