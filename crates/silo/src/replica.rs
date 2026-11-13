// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Object bytes on more than one control-plane server.
//!
//! Bucket and object records replicate through the cluster store. The
//! bytes do not: a put lands on the disk of the server that took it, which
//! is the leader. This module gets them to the other servers.
//!
//! * **Peer endpoint.** `GET /silo/peer/blobs/{bucket-id}/{sha256}` serves
//!   bytes this server holds for an object record with that checksum. It
//!   only reads its own disk. It follows the same rule as `/cluster`: the
//!   caller must be a member, proved with the cluster's peer key. The
//!   call goes to the other server's TLS listener when it has one, as
//!   every call between servers does (`keep::peer::PeerNet`).
//! * **Fetch on miss.** A read that finds the record but not the bytes
//!   asks each other member of the cluster, leader first, streams the
//!   answer to a temp file, checks size and SHA-256, and renames it into
//!   place. See [`Client::open_object`].
//! * **Background pull.** [`run`] does the same for every object this
//!   server lacks, so the copies exist before a failover needs them, and
//!   on a follower removes bytes whose record is gone.
//!
//! A key can be overwritten, so a file at `{bucket}/{key}` is not proof
//! of holding the current bytes. Every put and every fetch writes a
//! marker, `{bucket}/.have/{sha256 of the key}`, holding the checksum of
//! the file and the key. A file whose marker names another checksum is
//! stale and is fetched again. A file with no marker predates this module
//! and was written by the only server there was, so it is trusted.
//!
//! * **Copy before the answer.** A put is answered only after one other
//!   server holds the bytes: the server that took it calls
//!   `POST /silo/peer/pull/{object-id}/{sha256}` on each peer in turn and
//!   the first one that answers has fetched and checked them. See
//!   [`Peers::push`]. `[modules.silo] put_copies = 1` turns the wait off.
//!
//! Multipart staging is copied by [`crate::staging`].

use crate::client::Client;
use crate::types::{Bucket, Object};
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// Seconds between background pulls when the config does not say.
pub const REPLICATE_INTERVAL_SECS: u64 = 60;

/// A follower leaves bytes with no record alone until they are this old,
/// so a record that is still on its way through the log is not mistaken
/// for a deleted one.
pub const PRUNE_GRACE: Duration = Duration::from_secs(15 * 60);

/// Servers that hold an object's bytes before a put is answered, when
/// the config does not say.
pub const PUT_COPIES: u32 = 2;

/// How long a peer asked to pull waits for the object's record to reach
/// it through the log before it gives up.
pub const RECORD_WAIT: Duration = Duration::from_secs(5);

/// A peer asked to pull is given this long, plus a second for each
/// mebibyte, before the next one is tried.
const PUSH_BASE: Duration = Duration::from_secs(30);

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// A peer that sends nothing for this long is given up on. There is no
/// limit on the whole transfer: a 5 GiB object takes as long as it takes.
const STALL: Duration = Duration::from_secs(30);

/// One row of the cluster membership list in store meta.
#[derive(Clone, Debug, Deserialize)]
struct Member {
    #[serde(default)]
    id: String,
    #[serde(default)]
    api: String,
    /// The server's TLS listener, when it has one.
    #[serde(default)]
    agent_api: Option<String>,
}

/// One other server: its API address and, when it has one, its TLS
/// address.
pub(crate) type Target = (String, Option<String>);

/// The other control-plane servers, as a source of object bytes.
#[derive(Clone)]
pub struct Peers {
    source: Source,
    /// Which address of a server is called and what its certificate is
    /// checked against.
    net: keep::peer::PeerNet,
    /// Where a fixed list reads the peer key from.
    key_from: Option<Store>,
}

#[derive(Clone)]
enum Source {
    /// Read the member list from the store each time.
    Cluster { store: Store, me: Option<String> },
    /// A fixed list of API base URLs.
    Fixed(Vec<String>),
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

    fn peer_key(&self) -> Option<String> {
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

    /// Each server to ask, the leader first: its API address and, when it
    /// has one, its TLS address.
    pub(crate) fn targets(&self) -> Vec<Target> {
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
        for to in &urls {
            if taken >= wanted {
                break;
            }
            let path = format!("/silo/peer/pull/{}/{}", obj.meta.id, obj.status.sha256);
            match self.post(to, &path, wait).await {
                Ok(()) => taken += 1,
                Err(e) => last = format!("{}: {e}", to.0),
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

    /// POST `path` to a peer with the peer key and wait up to `wait` for
    /// it to say it is done.
    pub(crate) async fn post(&self, to: &Target, path: &str, wait: Duration) -> Result<(), String> {
        let sent = self.send(to, |client, base| client.post(format!("{base}{path}")));
        match tokio::time::timeout(wait, sent).await {
            Ok(Ok(resp)) if resp.status().is_success() => Ok(()),
            Ok(Ok(resp)) => Err(format!("status {}", resp.status().as_u16())),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(format!("no answer in {}s", wait.as_secs())),
        }
    }

    /// GET `path` from a peer with the peer key. An answer that is not a
    /// success is an error.
    pub(crate) async fn get(&self, to: &Target, path: &str) -> Result<reqwest::Response, String> {
        let sent = self.send(to, |client, base| client.get(format!("{base}{path}")));
        let resp = tokio::time::timeout(STALL, sent)
            .await
            .map_err(|_| "no answer".to_owned())??;
        if !resp.status().is_success() {
            return Err(format!("status {}", resp.status().as_u16()));
        }
        Ok(resp)
    }

    /// Send one request to a peer the way every call between servers
    /// goes: at its TLS listener when it has one, with the peer key.
    async fn send(
        &self,
        (api, tls): &Target,
        request: impl Fn(reqwest::Client, &str) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, String> {
        let key = self.peer_key();
        self.net
            .send(api, tls.as_deref(), CONNECT_TIMEOUT, |client, base| {
                let req = request(client, base.trim_end_matches('/'));
                match &key {
                    Some(key) => req.header(keep::peer::PEER_HEADER, key),
                    None => req,
                }
            })
            .await
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

    async fn fetch_from(
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

/// How long a peer asked to copy `size` bytes is given.
pub(crate) fn push_wait(size: u64) -> Duration {
    PUSH_BASE + Duration::from_secs(size / (1024 * 1024))
}

/// This server's member id, from `<data_dir>/node.id`.
fn own_member_id() -> Option<String> {
    let path = crate::Silo::data_root().parent()?.join("node.id");
    let id = std::fs::read_to_string(path).ok()?;
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // Writing to a String cannot fail.
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ---- markers ----------------------------------------------------------------

/// What a marker says about the file at a key path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Marker {
    pub sha256: String,
    pub key: String,
}

fn marker_dir(root: &Path, bucket: &Id) -> PathBuf {
    root.join(bucket.to_string()).join(".have")
}

pub(crate) fn marker_path(root: &Path, bucket: &Id, key: &str) -> PathBuf {
    marker_dir(root, bucket).join(hex(Sha256::digest(key.as_bytes()).as_slice()))
}

/// Record that the file at `key` holds the bytes with checksum `sha256`.
pub(crate) async fn write_marker(
    root: &Path,
    bucket: &Id,
    key: &str,
    sha256: &str,
) -> std::io::Result<()> {
    let path = marker_path(root, bucket, key);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_file_name(format!(".tmp-{}", uuid::Uuid::now_v7()));
    tokio::fs::write(&tmp, format!("{sha256}\n{key}")).await?;
    tokio::fs::rename(&tmp, &path).await
}

async fn read_marker_file(path: &Path) -> Option<Marker> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let (sha256, key) = text.split_once('\n')?;
    Some(Marker {
        sha256: sha256.to_owned(),
        key: key.to_owned(),
    })
}

pub(crate) async fn read_marker(root: &Path, bucket: &Id, key: &str) -> Option<Marker> {
    read_marker_file(&marker_path(root, bucket, key)).await
}

pub(crate) async fn remove_marker(root: &Path, bucket: &Id, key: &str) {
    let _ = tokio::fs::remove_file(marker_path(root, bucket, key)).await;
}

// ---- background pull --------------------------------------------------------

/// What one pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Objects whose bytes were fetched from a peer.
    pub fetched: usize,
    /// Objects still without bytes after asking every peer.
    pub missing: usize,
    /// Files removed because their record is gone.
    pub pruned: usize,
}

/// Fetch the bytes of every object this server lacks.
///
/// With `prune`, also remove bytes whose record is gone and that are
/// older than `grace`. Only a follower prunes: the leader is where a put
/// writes the file before the record, and its deletes remove their own
/// bytes. A client with no peers does nothing.
pub async fn sync_once(client: &Client, prune: bool, grace: Duration) -> ApiResult<SyncReport> {
    let mut report = SyncReport::default();
    let Some(peers) = client.peers() else {
        return Ok(report);
    };
    if peers.urls().is_empty() {
        return Ok(report);
    }
    let buckets = client.store().collection::<Bucket>().list(None)?;
    let objects = client.store().collection::<Object>().list(None)?;
    for obj in &objects {
        if !buckets.iter().any(|b| b.meta.id == obj.spec.bucket) {
            continue;
        }
        match client.ensure_bytes(obj).await {
            Ok(true) => report.fetched += 1,
            Ok(false) => {}
            Err(e) => {
                report.missing += 1;
                tracing::warn!(
                    bucket = %obj.spec.bucket,
                    key = %obj.spec.key,
                    error = %e,
                    "silo: object bytes are on no server that answered"
                );
            }
        }
    }
    if prune {
        report.pruned = prune_orphans(client, &buckets, &objects, grace).await;
    }
    Ok(report)
}

/// Remove what a deleted record left on this server's disk.
async fn prune_orphans(
    client: &Client,
    buckets: &[Bucket],
    objects: &[Object],
    grace: Duration,
) -> usize {
    let mut pruned = 0;
    let Ok(mut dirs) = tokio::fs::read_dir(client.root()).await else {
        return 0;
    };
    while let Ok(Some(entry)) = dirs.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.path().is_dir() {
            continue;
        }
        let Some(bucket) = buckets.iter().find(|b| b.meta.id.to_string() == name) else {
            // The bucket was deleted on the leader.
            if older_than(&entry.path(), grace).await
                && tokio::fs::remove_dir_all(entry.path()).await.is_ok()
            {
                tracing::info!(bucket = %name, "silo: removed the bytes of a deleted bucket");
                pruned += 1;
            }
            continue;
        };
        let live: Vec<&Object> = objects
            .iter()
            .filter(|o| o.spec.bucket == bucket.meta.id)
            .collect();
        let keys: BTreeSet<&str> = live
            .iter()
            .filter(|o| o.status.latest)
            .map(|o| o.spec.key.as_str())
            .collect();
        let shas: BTreeSet<&str> = live.iter().map(|o| o.status.sha256.as_str()).collect();

        if let Ok(mut markers) =
            tokio::fs::read_dir(marker_dir(client.root(), &bucket.meta.id)).await
        {
            while let Ok(Some(m)) = markers.next_entry().await {
                let Some(marker) = read_marker_file(&m.path()).await else {
                    continue;
                };
                if keys.contains(marker.key.as_str()) || !older_than(&m.path(), grace).await {
                    continue;
                }
                if let Ok(path) = client.key_path(&bucket.meta.id, &marker.key) {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                let _ = tokio::fs::remove_file(m.path()).await;
                pruned += 1;
            }
        }
        if let Ok(mut blobs) = tokio::fs::read_dir(entry.path().join(".blobs")).await {
            while let Ok(Some(b)) = blobs.next_entry().await {
                let sha = b.file_name().to_string_lossy().into_owned();
                if shas.contains(sha.as_str()) || !older_than(&b.path(), grace).await {
                    continue;
                }
                if tokio::fs::remove_file(b.path()).await.is_ok() {
                    pruned += 1;
                }
            }
        }
    }
    pruned
}

async fn older_than(path: &Path, age: Duration) -> bool {
    let Ok(meta) = tokio::fs::metadata(path).await else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|d| d >= age)
        .unwrap_or(false)
}

/// Pull every `interval` until `shutdown`. `is_leader` is asked before
/// each pass; a follower also prunes.
pub async fn run(
    client: Client,
    interval: Duration,
    is_leader: impl Fn() -> bool + Send + 'static,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(interval) => {}
        }
        match sync_once(&client, !is_leader(), PRUNE_GRACE).await {
            Ok(report) if report != SyncReport::default() => tracing::info!(
                fetched = report.fetched,
                missing = report.missing,
                pruned = report.pruned,
                "silo: object bytes synced with the other servers"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "silo: syncing object bytes failed"),
        }
        match crate::staging::sync(&client).await {
            Ok(report) if report != crate::staging::StagedReport::default() => tracing::info!(
                fetched = report.fetched,
                removed = report.removed,
                "silo: multipart staging synced with the other servers"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "silo: syncing multipart staging failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_leader_is_asked_first_and_this_server_never() {
        let store = Store::in_memory().unwrap();
        let peers = Peers {
            source: Source::Cluster {
                store: store.clone(),
                me: Some("n2".into()),
            },
            net: keep::peer::PeerNet::plain(),
            key_from: None,
        };
        assert!(peers.urls().is_empty(), "no member list, nobody to ask");

        store
            .put_meta(
                "members",
                &serde_json::json!([
                    {"id": "n1", "api": "http://10.0.0.11:8080", "role": "voter",
                     "agent_api": "https://10.0.0.11:8444"},
                    {"id": "n2", "api": "http://10.0.0.12:8080", "role": "voter"},
                    {"id": "n3", "api": "http://10.0.0.13:8080", "role": "learner"},
                    {"id": "n4", "api": "", "role": "learner"}
                ]),
            )
            .unwrap();
        store.put_meta("cluster_leader_id", &"n3").unwrap();
        assert_eq!(
            peers.urls(),
            vec!["http://10.0.0.13:8080", "http://10.0.0.11:8080"]
        );
        // The TLS address goes with the server that has one.
        let targets = peers.targets();
        assert_eq!(targets[0], ("http://10.0.0.13:8080".to_owned(), None));
        assert_eq!(targets[1].1.as_deref(), Some("https://10.0.0.11:8444"));
    }

    #[tokio::test]
    async fn a_marker_keeps_the_checksum_and_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Id::new("bkt");
        assert!(read_marker(dir.path(), &bucket, "a/b").await.is_none());
        write_marker(dir.path(), &bucket, "a/b", "abc123")
            .await
            .unwrap();
        write_marker(dir.path(), &bucket, "a\nb", "ffff")
            .await
            .unwrap();
        assert_eq!(
            read_marker(dir.path(), &bucket, "a/b").await,
            Some(Marker {
                sha256: "abc123".into(),
                key: "a/b".into()
            })
        );
        // A key may hold a newline; everything after the first line is the key.
        assert_eq!(
            read_marker(dir.path(), &bucket, "a\nb").await.unwrap().key,
            "a\nb"
        );
        remove_marker(dir.path(), &bucket, "a/b").await;
        assert!(read_marker(dir.path(), &bucket, "a/b").await.is_none());
    }
}
