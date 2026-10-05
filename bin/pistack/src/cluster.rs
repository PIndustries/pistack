// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Control-plane membership.
//!
//! A machine that already has a store stays that cluster and announces it.
//! A machine with no store, on a private IPv4 listen address, sweeps its /24.
//! One cluster means adopt it. None means found one. Two means wait for
//! `--join`. The second member replicates as a learner. The third makes
//! every member a voter. A learner does not become leader.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use pistack_cluster::{
    accepts_writes, check_advertise, classify, consider_vote, join_membership, scan_targets,
    seed_membership, sweep, Announce, Ballot, Formation, JoinRequest, Member, Role, VoteRequest,
    DISCOVER_PORT,
};
use pistack_rpc::{JOIN_HEADER, REQUIRE_JOIN_ENV};
use pistack_store::{LogEntry, Store};
use pistack_types::{ApiError, Leadership};
use serde::Deserialize;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct BootRequest<'a> {
    pub data_dir: &'a Path,
    pub init: bool,
    pub join: Option<&'a str>,
    pub listen: &'a str,
    pub public_url: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootKind {
    /// `state.redb` was already here. Do not sweep and do not download.
    Keep,
    /// Found a new cluster. The store file does not exist yet.
    Init,
    /// Snapshot downloaded. This process is not the leader.
    Joined,
}

#[derive(Debug)]
pub struct Established {
    pub role: Role,
    pub leader: bool,
    pub leader_api: String,
    pub term: u64,
    pub members: Vec<Member>,
}

pub enum LocalRole {
    Ready(Established),
    /// The store belongs to a cluster this node id is not in.
    NeedJoin,
}

pub struct ClusterHandle {
    pub node_id: String,
    pub self_api: String,
    pub cluster_id: String,
    pub raft: String,
    pub store: Store,
    pub db_path: PathBuf,
    pub role: Mutex<Role>,
    pub leader: AtomicBool,
    pub leader_api: Mutex<String>,
    pub term: AtomicU64,
    pub announce: watch::Sender<Announce>,
    /// Watchable copy of [`ClusterHandle::leader`] for module reconcilers.
    pub leadership: Leadership,
}

impl ClusterHandle {
    fn set_leader(&self, leader: bool) {
        self.leader.store(leader, Ordering::Relaxed);
        self.leadership.set(leader);
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ClusterStatus {
    cluster_id: String,
    node_id: String,
    api: String,
    leader_api: String,
    leader_id: String,
    role: Role,
    leader: bool,
    members: Vec<Member>,
}

#[derive(Deserialize)]
struct LogQuery {
    #[serde(default)]
    after: u64,
}

/// Private unicast from `host:port`. `0.0.0.0`, public addresses, and names
/// return `None` so discovery never scans them.
pub fn advertise_ip(listen: &str) -> Option<IpAddr> {
    let host = listen.rsplit_once(':')?.0;
    let host = host.trim_matches(|c| c == '[' || c == ']');
    let ip: IpAddr = host.parse().ok()?;
    check_advertise(ip).ok()?;
    Some(ip)
}

pub fn load_or_create_node_id(data_dir: &Path) -> anyhow::Result<String> {
    let path = data_dir.join("node.id");
    if let Ok(text) = std::fs::read_to_string(&path) {
        let text = text.trim();
        if !text.is_empty() {
            return Ok(text.to_owned());
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let id = uuid::Uuid::now_v7().to_string();
    std::fs::write(&path, &id)?;
    Ok(id)
}

/// Make `state.redb` the right file for this boot. An existing file returns
/// [`BootKind::Keep`] without opening a socket.
pub async fn prepare_store(req: &BootRequest<'_>) -> anyhow::Result<BootKind> {
    let path = req.data_dir.join("state.redb");
    if path.exists() {
        return Ok(BootKind::Keep);
    }
    if req.init {
        return Ok(BootKind::Init);
    }
    if let Some(url) = req.join.map(str::trim).filter(|s| !s.is_empty()) {
        remember_leader(req.data_dir, url);
        fetch_snapshot(url, &path).await?;
        return Ok(BootKind::Joined);
    }
    let Some(ip) = advertise_ip(req.listen) else {
        anyhow::bail!(
            "no state in {}: pass --init to found a cluster or --join URL to adopt one. Automatic discovery only runs when --listen is a private IPv4 address",
            req.data_dir.display()
        );
    };
    let node_id = load_or_create_node_id(req.data_dir)?;
    match discover_formation(ip, &node_id, req.public_url).await? {
        Formation::Found => Ok(BootKind::Init),
        Formation::Join(ann) => {
            if ann.api.trim().is_empty() {
                anyhow::bail!("peer announced an empty api address; pass --join URL");
            }
            remember_leader(req.data_dir, ann.api.trim());
            fetch_snapshot(ann.api.trim(), &path).await?;
            Ok(BootKind::Joined)
        }
        Formation::Wait => anyhow::bail!(
            "another machine on this prefix is starting. Retry in a few seconds so the first one can publish the cluster"
        ),
        Formation::Ambiguous(ids) => {
            anyhow::bail!("heard more than one cluster ({}); pass --join URL", ids.join(", "))
        }
    }
}

async fn discover_formation(ip: IpAddr, node_id: &str, api: &str) -> anyhow::Result<Formation> {
    let IpAddr::V4(v4) = ip else {
        anyhow::bail!("discovery needs an IPv4 listen address");
    };
    let targets = discover_targets(v4)?;
    let mine = Announce {
        cluster_id: String::new(),
        node_id: node_id.to_owned(),
        api: api.to_owned(),
        raft: format!("{ip}:18446"),
    };
    let bind = SocketAddr::new(ip, DISCOVER_PORT);
    let heard = sweep(bind, &targets, &mine, Duration::from_millis(1200))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    tracing::info!(heard = heard.len(), "discovery sweep");
    if !matches!(classify(node_id, &heard), Formation::Wait) {
        return Ok(classify(node_id, &heard));
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let heard = sweep(bind, &targets, &mine, Duration::from_millis(1200))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    tracing::info!(heard = heard.len(), "discovery sweep retried");
    Ok(classify(node_id, &heard))
}

fn discover_targets(ip: Ipv4Addr) -> anyhow::Result<Vec<SocketAddr>> {
    let hosts = scan_targets(ip, 24).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(hosts
        .into_iter()
        .map(|host| SocketAddr::new(IpAddr::V4(host), DISCOVER_PORT))
        .collect())
}

/// Seed a store that is ours, or report that it belongs to someone else.
/// `allow_seed` is false after a snapshot download so a copy cannot found
/// a second cluster.
pub fn establish_local(
    store: &Store,
    node_id: &str,
    self_api: &str,
    allow_seed: bool,
) -> anyhow::Result<LocalRole> {
    store.set_record_log(false);
    let members: Vec<Member> = store.get_meta("members")?.unwrap_or_default();
    if members.is_empty() {
        if !allow_seed {
            return Ok(LocalRole::NeedJoin);
        }
        let members = seed_membership(node_id, self_api);
        let term = store.get_meta::<u64>("cluster_term")?.unwrap_or(0).max(1);
        store.put_meta("node_id", &node_id)?;
        store.put_meta("members", &members)?;
        store.put_meta("cluster_leader_id", &node_id)?;
        store.put_meta("cluster_term", &term)?;
        store.put_meta("cluster_voted_for", &Some(node_id.to_owned()))?;
        store.set_log_term(term);
        store.set_record_log(true);
        return Ok(LocalRole::Ready(Established {
            role: Role::Voter,
            leader: true,
            leader_api: self_api.to_owned(),
            term,
            members,
        }));
    }
    let Some(me) = members.iter().find(|member| member.id == node_id) else {
        return Ok(LocalRole::NeedJoin);
    };
    let voters = members
        .iter()
        .filter(|member| member.role == Role::Voter)
        .count();
    let sole = voters == 1 && me.role == Role::Voter && members.len() == 1;
    let leader_id: Option<String> = store.get_meta("cluster_leader_id")?;
    let leader = me.role == Role::Voter && (sole || leader_id.as_deref() == Some(node_id));
    let term = store.get_meta::<u64>("cluster_term")?.unwrap_or(0);
    let term = if leader { term.max(1) } else { term };
    store.put_meta("node_id", &node_id)?;
    if leader {
        store.put_meta("cluster_leader_id", &node_id)?;
        store.put_meta("cluster_term", &term)?;
        store.put_meta("cluster_voted_for", &Some(node_id.to_owned()))?;
        store.set_log_term(term);
        store.set_record_log(true);
    }
    Ok(LocalRole::Ready(Established {
        role: me.role,
        leader,
        leader_api: if leader {
            self_api.to_owned()
        } else {
            leader_id
                .as_deref()
                .and_then(|id| members.iter().find(|member| member.id == id))
                .map(|member| member.api.clone())
                .unwrap_or_else(|| self_api.to_owned())
        },
        term,
        members,
    }))
}

pub fn leader_hint(store: &Store, self_id: &str) -> anyhow::Result<Option<String>> {
    let members: Vec<Member> = store.get_meta("members")?.unwrap_or_default();
    let leader_id: Option<String> = store.get_meta("cluster_leader_id")?;
    if let Some(id) = leader_id.as_deref() {
        if id != self_id {
            if let Some(member) = members.iter().find(|member| member.id == id) {
                return Ok(Some(member.api.clone()));
            }
        }
    }
    Ok(members
        .into_iter()
        .find(|member| member.role == Role::Voter && member.id != self_id)
        .map(|member| member.api))
}

pub fn remembered_leader(data_dir: &Path) -> Option<String> {
    std::fs::read_to_string(data_dir.join("adopt.url"))
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

pub fn clear_remembered_leader(data_dir: &Path) {
    let _ = std::fs::remove_file(data_dir.join("adopt.url"));
}

fn remember_leader(data_dir: &Path, api: &str) {
    let _ = std::fs::write(data_dir.join("adopt.url"), api);
}

/// Ask `leader_api` to admit us, then record the membership it returns.
/// This process does not become leader.
pub async fn adopt(
    store: &Store,
    leader_api: &str,
    node_id: &str,
    self_api: &str,
) -> anyhow::Result<Established> {
    let url = format!("{}/cluster/join", leader_api.trim_end_matches('/'));
    let body = JoinRequest {
        node_id: node_id.to_owned(),
        api: self_api.to_owned(),
    };
    let status: ClusterStatus = post_json(&url, &body)
        .await
        .with_context(|| format!("joining {url}"))?;
    store.set_record_log(false);
    store.put_meta("members", &status.members)?;
    store.put_meta("node_id", &node_id)?;
    if !status.leader_id.is_empty() {
        store.put_meta("cluster_leader_id", &status.leader_id)?;
    }
    let role = status
        .members
        .iter()
        .find(|member| member.id == node_id)
        .map(|member| member.role)
        .unwrap_or(Role::Learner);
    let leader_api = if status.leader_api.is_empty() {
        leader_api.to_owned()
    } else {
        status.leader_api
    };
    Ok(Established {
        role,
        leader: false,
        leader_api,
        term: store.get_meta::<u64>("cluster_term")?.unwrap_or(0),
        members: status.members,
    })
}

pub fn handle_from(
    node_id: String,
    self_api: String,
    cluster_id: String,
    store: Store,
    db_path: PathBuf,
    established: &Established,
    advertise: Option<IpAddr>,
) -> ClusterHandle {
    let raft = advertise
        .map(|ip| format!("{ip}:18446"))
        .unwrap_or_else(|| "0.0.0.0:18446".into());
    let announce = Announce {
        cluster_id: cluster_id.clone(),
        node_id: node_id.clone(),
        api: established.leader_api.clone(),
        raft: raft.clone(),
    };
    let (tx, _rx) = watch::channel(announce);
    ClusterHandle {
        node_id,
        self_api,
        cluster_id,
        raft,
        store,
        db_path,
        role: Mutex::new(established.role),
        leader: AtomicBool::new(established.leader),
        leader_api: Mutex::new(established.leader_api.clone()),
        term: AtomicU64::new(established.term),
        announce: tx,
        leadership: Leadership::new(established.leader),
    }
}

pub fn spawn_background(
    handle: std::sync::Arc<ClusterHandle>,
    advertise: Option<IpAddr>,
    shutdown: CancellationToken,
) {
    publish(&handle);
    if let Some(ip) = advertise {
        let rx = handle.announce.subscribe();
        let stop = shutdown.clone();
        tokio::spawn(async move {
            if let Err(err) = beacon(ip, rx, stop).await {
                tracing::warn!(error = %err, "discovery beacon stopped");
            }
        });
    } else {
        tracing::info!("discovery beacon off; listen address is not a private unicast");
    }
    let stop = shutdown.clone();
    tokio::spawn(async move { follow(handle, stop).await });
}

pub fn router(handle: std::sync::Arc<ClusterHandle>) -> Router {
    Router::new()
        .route("/cluster", axum::routing::get(show))
        .route("/cluster/join", axum::routing::post(join))
        .route("/cluster/snapshot", axum::routing::get(snapshot))
        .route("/cluster/log", axum::routing::get(log))
        .route("/cluster/vote", axum::routing::post(vote))
        .with_state(handle)
}

pub async fn reject_follower_writes(
    State(handle): State<std::sync::Arc<ClusterHandle>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let read = method == axum::http::Method::GET
        || method == axum::http::Method::HEAD
        || method == axum::http::Method::OPTIONS;
    if read || path == "/healthz" || path == "/" || path.starts_with("/cluster") {
        return next.run(req).await;
    }
    if handle.leader.load(Ordering::Relaxed) {
        return next.run(req).await;
    }
    let leader = locked(&handle.leader_api);
    ApiError::conflict("not the leader")
        .with_detail("leader", leader)
        .into_response()
}

async fn show(
    State(handle): State<std::sync::Arc<ClusterHandle>>,
    headers: HeaderMap,
) -> Result<Json<ClusterStatus>, ApiError> {
    enforce_join(&handle.store, &headers)?;
    let members = members_of(&handle.store)?;
    Ok(Json(status_of(&handle, &members)?))
}

async fn join(
    State(handle): State<std::sync::Arc<ClusterHandle>>,
    headers: HeaderMap,
    Json(body): Json<JoinRequest>,
) -> Result<Json<ClusterStatus>, ApiError> {
    enforce_join(&handle.store, &headers)?;
    if !handle.leader.load(Ordering::Relaxed) {
        return Err(not_leader(&handle));
    }
    let node_id = body.node_id.trim();
    let api = body.api.trim();
    if node_id.is_empty() || api.is_empty() {
        return Err(ApiError::bad_request("node_id and api are required"));
    }
    let current = members_of(&handle.store)?;
    let next = join_membership(&current, node_id, api);
    handle.store.put_meta("members", &next).map_err(store_err)?;
    if let Some(me) = next.iter().find(|member| member.id == handle.node_id) {
        *guard(&handle.role) = me.role;
    }
    Ok(Json(status_of(&handle, &next)?))
}

async fn snapshot(
    State(handle): State<std::sync::Arc<ClusterHandle>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    enforce_join(&handle.store, &headers)?;
    if !handle.leader.load(Ordering::Relaxed) {
        return Err(not_leader(&handle));
    }
    let dest = handle
        .db_path
        .with_file_name(format!("snapshot-{}.redb", uuid::Uuid::now_v7()));
    let _remove = RemoveFile(dest.clone());
    handle
        .store
        .snapshot_file(&handle.db_path, &dest)
        .map_err(store_err)?;
    let bytes = std::fs::read(&dest).map_err(|err| ApiError::internal(err.to_string()))?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()))
}

async fn log(
    State(handle): State<std::sync::Arc<ClusterHandle>>,
    headers: HeaderMap,
    Query(query): Query<LogQuery>,
) -> Result<Json<Vec<LogEntry>>, ApiError> {
    enforce_join(&handle.store, &headers)?;
    let entries = handle.store.read_log(query.after, 64).map_err(store_err)?;
    Ok(Json(entries))
}

async fn vote(
    State(handle): State<std::sync::Arc<ClusterHandle>>,
    headers: HeaderMap,
    Json(req): Json<VoteRequest>,
) -> Result<Json<pistack_cluster::VoteResponse>, ApiError> {
    enforce_join(&handle.store, &headers)?;
    if *guard(&handle.role) == Role::Learner {
        let term = handle.term.load(Ordering::Relaxed);
        return Ok(Json(pistack_cluster::VoteResponse {
            term,
            granted: false,
        }));
    }
    let mut ballot = load_ballot(&handle.store).map_err(store_err)?;
    let before = ballot.current_term;
    let response = consider_vote(&mut ballot, &req);
    save_ballot(&handle.store, &ballot).map_err(store_err)?;
    handle.term.store(ballot.current_term, Ordering::Relaxed);
    if response.term > before && handle.leader.load(Ordering::Relaxed) {
        handle.set_leader(false);
        handle.store.set_record_log(false);
        if response.granted {
            *guard(&handle.leader_api) = String::new();
        }
    }
    Ok(Json(response))
}

pub async fn pull_log(leader: &str, store: &Store) -> anyhow::Result<()> {
    let after = store.commit_index()?;
    let url = format!("{}/cluster/log?after={after}", leader.trim_end_matches('/'));
    let entries: Vec<LogEntry> = get_json(&url).await?;
    if entries.is_empty() {
        return Ok(());
    }
    store.apply_entries(&entries)?;
    Ok(())
}

async fn follow(handle: std::sync::Arc<ClusterHandle>, shutdown: CancellationToken) {
    let mut failures = 0u32;
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
        if shutdown.is_cancelled() {
            break;
        }
        if handle.leader.load(Ordering::Relaxed) {
            failures = 0;
            continue;
        }
        let leader = locked(&handle.leader_api);
        let pulled = if leader.is_empty() || leader == handle.self_api {
            Err(anyhow::anyhow!("leader address is not known"))
        } else {
            refresh(&handle, &leader).await
        };
        match pulled {
            Ok(()) => failures = 0,
            Err(err) => {
                failures += 1;
                tracing::debug!(error = %err, "replication pull failed");
            }
        }
        if failures >= 3 {
            failures = 0;
            if let Err(err) = campaign(&handle).await {
                tracing::info!(error = %err, "election did not complete");
            }
        }
    }
}

async fn refresh(handle: &ClusterHandle, leader: &str) -> anyhow::Result<()> {
    let url = format!("{}/cluster", leader.trim_end_matches('/'));
    let status: ClusterStatus = get_json(&url).await?;
    if !status.leader_api.is_empty() {
        *guard(&handle.leader_api) = status.leader_api.clone();
    }
    if let Some(me) = status
        .members
        .iter()
        .find(|member| member.id == handle.node_id)
    {
        *guard(&handle.role) = me.role;
        handle.store.put_meta("members", &status.members)?;
    }
    publish(handle);
    pull_log(leader, &handle.store).await?;
    Ok(())
}

async fn campaign(handle: &ClusterHandle) -> anyhow::Result<()> {
    let role = *guard(&handle.role);
    if role != Role::Voter || handle.leader.load(Ordering::Relaxed) {
        if role == Role::Learner {
            tracing::info!("learner cannot lead");
        }
        return Ok(());
    }
    let members = members_of(&handle.store).map_err(|err| anyhow::anyhow!("{err}"))?;
    let voters: Vec<&Member> = members
        .iter()
        .filter(|member| member.role == Role::Voter)
        .collect();
    if voters.len() <= 1 {
        let term = handle.term.load(Ordering::Relaxed).max(1);
        become_leader(handle, term)?;
        return Ok(());
    }
    let mut ballot = load_ballot(&handle.store)?;
    ballot.current_term += 1;
    ballot.voted_for = Some(handle.node_id.clone());
    let term = ballot.current_term;
    let last_index = ballot.last_index;
    let last_term = ballot.last_term;
    save_ballot(&handle.store, &ballot)?;
    handle.term.store(term, Ordering::Relaxed);
    let request = VoteRequest {
        term,
        candidate_id: handle.node_id.clone(),
        last_index,
        last_term,
    };
    let mut grants = 1usize;
    for peer in &voters {
        if peer.id == handle.node_id {
            continue;
        }
        let url = format!("{}/cluster/vote", peer.api.trim_end_matches('/'));
        match post_json::<_, pistack_cluster::VoteResponse>(&url, &request).await {
            Ok(response) if response.granted && response.term == term => grants += 1,
            Ok(response) if response.term > term => {
                handle.term.store(response.term, Ordering::Relaxed);
                handle.store.put_meta("cluster_term", &response.term)?;
                return Ok(());
            }
            Ok(_) => {}
            Err(err) => tracing::debug!(peer = %peer.api, error = %err, "vote request failed"),
        }
    }
    if grants >= majority(voters.len()) && accepts_writes(Role::Voter, true) {
        become_leader(handle, term)?;
    }
    Ok(())
}

fn become_leader(handle: &ClusterHandle, term: u64) -> anyhow::Result<()> {
    handle.term.store(term, Ordering::Relaxed);
    handle.set_leader(true);
    *guard(&handle.leader_api) = handle.self_api.clone();
    *guard(&handle.role) = Role::Voter;
    handle.store.set_log_term(term);
    handle.store.set_record_log(true);
    handle
        .store
        .put_meta("cluster_leader_id", &handle.node_id)?;
    handle.store.put_meta("cluster_term", &term)?;
    handle
        .store
        .put_meta("cluster_voted_for", &Some(handle.node_id.clone()))?;
    publish(handle);
    tracing::info!(node = %handle.node_id, term, "this node is the leader");
    Ok(())
}

async fn beacon(
    ip: IpAddr,
    mut announce: watch::Receiver<Announce>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let sock = UdpSocket::bind(SocketAddr::new(ip, DISCOVER_PORT))
        .await
        .with_context(|| format!("binding discovery on {ip}:{DISCOVER_PORT}"))?;
    tracing::info!(%ip, port = DISCOVER_PORT, "discovery beacon up");
    let mut buf = [0u8; 2048];
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return Ok(()),
            rec = sock.recv_from(&mut buf) => {
                let (_n, src) = rec?;
                let current = announce.borrow_and_update().clone();
                let bytes = serde_json::to_vec(&current)?;
                if let Err(err) = sock.send_to(&bytes, src).await {
                    tracing::debug!(error = %err, "discovery reply failed");
                }
            }
        }
    }
}

fn publish(handle: &ClusterHandle) {
    let ann = Announce {
        cluster_id: handle.cluster_id.clone(),
        node_id: handle.node_id.clone(),
        api: locked(&handle.leader_api),
        raft: handle.raft.clone(),
    };
    let _ = handle.announce.send(ann);
}

fn status_of(handle: &ClusterHandle, members: &[Member]) -> Result<ClusterStatus, ApiError> {
    let role = members
        .iter()
        .find(|member| member.id == handle.node_id)
        .map(|member| member.role)
        .unwrap_or(*guard(&handle.role));
    let leader_id: String = handle
        .store
        .get_meta("cluster_leader_id")
        .map_err(store_err)?
        .unwrap_or_default();
    Ok(ClusterStatus {
        cluster_id: handle.cluster_id.clone(),
        node_id: handle.node_id.clone(),
        api: handle.self_api.clone(),
        leader_api: locked(&handle.leader_api),
        leader_id,
        role,
        leader: handle.leader.load(Ordering::Relaxed),
        members: members.to_vec(),
    })
}

fn members_of(store: &Store) -> Result<Vec<Member>, ApiError> {
    store
        .get_meta("members")
        .map_err(store_err)
        .map(|v| v.unwrap_or_default())
}

fn load_ballot(store: &Store) -> anyhow::Result<Ballot> {
    let (last_index, last_term) = tip(store)?;
    Ok(Ballot {
        current_term: store.get_meta("cluster_term")?.unwrap_or(0),
        voted_for: store.get_meta("cluster_voted_for")?.unwrap_or(None),
        last_index,
        last_term,
    })
}

fn save_ballot(store: &Store, ballot: &Ballot) -> anyhow::Result<()> {
    store.put_meta("cluster_term", &ballot.current_term)?;
    store.put_meta("cluster_voted_for", &ballot.voted_for)?;
    Ok(())
}

fn tip(store: &Store) -> anyhow::Result<(u64, u64)> {
    let index = store.commit_index()?;
    if index == 0 {
        return Ok((0, 0));
    }
    let entries = store.read_log(index - 1, 1)?;
    let term = entries.first().map(|entry| entry.term).unwrap_or(0);
    Ok((index, term))
}

fn not_leader(handle: &ClusterHandle) -> ApiError {
    ApiError::conflict("not the leader").with_detail("leader", locked(&handle.leader_api))
}

fn store_err(err: impl std::fmt::Display) -> ApiError {
    ApiError::internal(err.to_string())
}

fn enforce_join(store: &Store, headers: &HeaderMap) -> Result<(), ApiError> {
    let configured = std::env::var(REQUIRE_JOIN_ENV).ok();
    enforce_join_value(store, headers, configured.as_deref())
}

fn join_required_value(value: Option<&str>) -> bool {
    value.is_some_and(|item| item.trim() == "1")
}

/// When `required` is not exactly `1`, the route stays open. Otherwise the
/// header must name an unexpired token that is not spent. This does not
/// redeem, and it does not log the clear text.
fn enforce_join_value(
    store: &Store,
    headers: &HeaderMap,
    required: Option<&str>,
) -> Result<(), ApiError> {
    if !join_required_value(required) {
        return Ok(());
    }
    let token = match headers.get(JOIN_HEADER) {
        None => return Err(ApiError::unauthorized("missing join token")),
        Some(value) => match value.to_str() {
            Ok(text) if !text.trim().is_empty() => text.trim(),
            Ok(_) => return Err(ApiError::unauthorized("missing join token")),
            Err(_) => return Err(ApiError::unauthorized("malformed join token")),
        },
    };
    keep::authorize_join_token(store, token)
        .map(|_| ())
        .map_err(ApiError::from)
}

fn majority(voters: usize) -> usize {
    voters / 2 + 1
}

fn guard<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn locked<T: Clone>(mutex: &Mutex<T>) -> T {
    guard(mutex).clone()
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn with_join(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match std::env::var("PISTACK_JOIN_TOKEN") {
        Ok(token) if !token.trim().is_empty() => builder.header(JOIN_HEADER, token.trim()),
        _ => builder,
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> anyhow::Result<T> {
    let response = with_join(http_client().get(url))
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

async fn post_json<B, T>(url: &str, body: &B) -> anyhow::Result<T>
where
    B: serde::Serialize + ?Sized,
    T: serde::de::DeserializeOwned,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut last = None;
    while tokio::time::Instant::now() < deadline {
        match with_join(http_client().post(url)).json(body).send().await {
            Ok(response) if response.status().is_success() => return Ok(response.json().await?),
            Ok(response) if response.status().is_client_error() => {
                anyhow::bail!("{url} returned {}", response.status());
            }
            Ok(response) => {
                let status = response.status();
                last = Some(anyhow::anyhow!("{url} returned {status}"));
            }
            Err(err) => last = Some(err.into()),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("{url} failed")))
}

async fn fetch_snapshot(api: &str, dest: &Path) -> anyhow::Result<()> {
    if dest.exists() {
        anyhow::bail!("{} already exists", dest.display());
    }
    let url = format!("{}/cluster/snapshot", api.trim_end_matches('/'));
    let partial = dest.with_extension("partial");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut last = None;
    while tokio::time::Instant::now() < deadline {
        match with_join(http_client().get(&url)).send().await {
            Ok(response) if response.status().is_success() => {
                let bytes = response.bytes().await?;
                std::fs::write(&partial, &bytes)?;
                match verify_db(&partial) {
                    Ok(()) => {
                        std::fs::rename(&partial, dest)?;
                        return Ok(());
                    }
                    Err(err) => {
                        let _ = std::fs::remove_file(&partial);
                        last = Some(err);
                    }
                }
            }
            Ok(response) if response.status().is_client_error() => {
                anyhow::bail!("snapshot {url} returned {}", response.status());
            }
            Ok(response) => {
                last = Some(anyhow::anyhow!(
                    "snapshot {url} returned {}",
                    response.status()
                ));
            }
            Err(err) => last = Some(err.into()),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let _ = std::fs::remove_file(&partial);
    Err(last.unwrap_or_else(|| anyhow::anyhow!("snapshot {url} failed")))
}

fn verify_db(path: &Path) -> anyhow::Result<()> {
    let (events, _) = tokio::sync::broadcast::channel(1);
    drop(Store::open(path, events)?);
    Ok(())
}

struct RemoveFile(PathBuf);

impl Drop for RemoveFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::middleware;
    use pistack_cluster::{choose, should_discover, BootChoice};
    use std::sync::Arc;

    fn open_store(dir: &Path) -> Store {
        let (events, _) = tokio::sync::broadcast::channel(16);
        Store::open(dir.join("state.redb"), events).unwrap()
    }

    #[test]
    fn discovery_stays_on_a_private_listen_address() {
        assert_eq!(
            advertise_ip("10.66.66.1:18443").unwrap().to_string(),
            "10.66.66.1"
        );
        assert!(advertise_ip("0.0.0.0:8443").is_none());
        assert!(advertise_ip("8.8.8.8:443").is_none());
        assert!(advertise_ip("127.0.0.1:18443").is_none());
    }

    #[test]
    fn an_existing_store_stays_the_sole_voter() {
        assert!(!should_discover(true, false, Some("http://other")));
        assert_eq!(
            choose(true, true, Some("http://other"), &[]),
            BootChoice::Keep
        );
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        store
            .put_meta("cluster_id", &"keep-me".to_string())
            .unwrap();
        let established = establish_local(&store, "node-a", "http://10.1.1.1:18443", true).unwrap();
        match established {
            LocalRole::Ready(ready) => {
                assert!(ready.leader);
                assert_eq!(ready.role, Role::Voter);
                assert_eq!(ready.members.len(), 1);
                assert!(accepts_writes(ready.role, ready.leader));
            }
            LocalRole::NeedJoin => panic!("existing store tried to join"),
        }
        let id: Option<String> = store.get_meta("cluster_id").unwrap();
        assert_eq!(id.as_deref(), Some("keep-me"));
    }

    #[test]
    fn a_copied_store_does_not_take_over() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        store.put_meta("cluster_id", &"c1".to_string()).unwrap();
        store
            .put_meta("members", &seed_membership("leader", "http://leader"))
            .unwrap();
        store
            .put_meta("cluster_leader_id", &"leader".to_string())
            .unwrap();
        let established = establish_local(&store, "node-b", "http://b", false).unwrap();
        assert!(matches!(established, LocalRole::NeedJoin));
        let id: Option<String> = store.get_meta("cluster_id").unwrap();
        assert_eq!(id.as_deref(), Some("c1"));
        let leader: Option<String> = store.get_meta("cluster_leader_id").unwrap();
        assert_eq!(leader.as_deref(), Some("leader"));
    }

    #[tokio::test]
    async fn a_follower_adopts_and_a_third_node_makes_voters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.redb");
        let store = open_store(dir.path());
        store.put_meta("cluster_id", &"c1".to_string()).unwrap();
        let ready = match establish_local(&store, "node-a", "http://leader", true).unwrap() {
            LocalRole::Ready(ready) => ready,
            LocalRole::NeedJoin => panic!("founder tried to join"),
        };
        let handle = Arc::new(handle_from(
            "node-a".into(),
            "http://leader".into(),
            "c1".into(),
            store.clone(),
            path.clone(),
            &ready,
            None,
        ));
        let app = router(handle.clone())
            .route("/mutate", axum::routing::post(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                handle.clone(),
                reject_follower_writes,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let base = format!("http://{addr}");
        *guard(&handle.leader_api) = base.clone();

        handle.set_leader(false);
        let client = http_client();
        let refused = client.post(format!("{base}/mutate")).send().await.unwrap();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        let body = refused.text().await.unwrap();
        assert!(body.contains("not the leader"));
        handle.set_leader(true);
        let allowed = client.post(format!("{base}/mutate")).send().await.unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);

        let bytes = client
            .get(format!("{base}/cluster/snapshot"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let copy_dir = tempfile::tempdir().unwrap();
        let copy_path = copy_dir.path().join("state.redb");
        std::fs::write(&copy_path, &bytes).unwrap();
        let copy = open_store(copy_dir.path());
        let adopted = adopt(&copy, &base, "node-b", "http://b").await.unwrap();
        assert_eq!(adopted.role, Role::Learner);
        assert!(!adopted.leader);
        let copied_id: Option<String> = copy.get_meta("cluster_id").unwrap();
        assert_eq!(copied_id.as_deref(), Some("c1"));

        store.put_meta("seen", &"yes".to_string()).unwrap();
        pull_log(&base, &copy).await.unwrap();
        let seen: Option<String> = copy.get_meta("seen").unwrap();
        assert_eq!(seen.as_deref(), Some("yes"));

        let third: ClusterStatus = client
            .post(format!("{base}/cluster/join"))
            .json(&JoinRequest {
                node_id: "node-c".into(),
                api: "http://c".into(),
            })
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(third.members.len(), 3);
        assert!(third
            .members
            .iter()
            .all(|member| member.role == Role::Voter));
        assert!(third.members.iter().any(|member| member.id == "node-b"));
    }

    #[test]
    fn join_stays_open_until_required_and_a_redeem_spends_once() {
        assert!(!join_required_value(None));
        assert!(!join_required_value(Some("")));
        assert!(!join_required_value(Some("true")));
        assert!(join_required_value(Some("1")));
        assert!(join_required_value(Some(" 1 ")));

        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let (once, clear) = keep::create_join_token(&store, "once", "admin", false, None).unwrap();
        let open = HeaderMap::new();
        enforce_join_value(&store, &open, None).unwrap();
        enforce_join_value(&store, &open, Some("")).unwrap();

        let missing = enforce_join_value(&store, &open, Some("1")).unwrap_err();
        assert_eq!(missing.message, "missing join token");

        let mut bad = HeaderMap::new();
        bad.insert(
            JOIN_HEADER,
            axum::http::HeaderValue::from_bytes(&[0xff]).unwrap(),
        );
        let malformed = enforce_join_value(&store, &bad, Some("1")).unwrap_err();
        assert_eq!(malformed.message, "malformed join token");

        let mut unknown = HeaderMap::new();
        unknown.insert(JOIN_HEADER, "pij_nope".parse().unwrap());
        let unknown = enforce_join_value(&store, &unknown, Some("1")).unwrap_err();
        assert_eq!(unknown.message, "unknown join token");
        assert!(!unknown.message.contains("pij_nope"));

        let mut headers = HeaderMap::new();
        headers.insert(JOIN_HEADER, format!("  {clear}  ").parse().unwrap());
        enforce_join_value(&store, &headers, Some("1")).unwrap();
        assert_eq!(
            store
                .collection::<keep::JoinToken>()
                .get(&once.meta.id)
                .unwrap()
                .status
                .uses,
            0
        );

        keep::redeem_join_token(&store, &clear).unwrap();
        let spent = enforce_join_value(&store, &headers, Some("1")).unwrap_err();
        assert_eq!(spent.message, "join token already used");
        assert!(!spent.message.contains(&clear));
        assert_eq!(
            store
                .collection::<keep::JoinToken>()
                .get(&once.meta.id)
                .unwrap()
                .status
                .uses,
            1
        );

        let (old, old_clear) =
            keep::create_join_token(&store, "old", "admin", false, None).unwrap();
        store
            .collection::<keep::JoinToken>()
            .modify(&old.meta.id, |token| {
                token.spec.expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
                Ok(())
            })
            .unwrap();
        let mut expired = HeaderMap::new();
        expired.insert(JOIN_HEADER, old_clear.parse().unwrap());
        let expired = enforce_join_value(&store, &expired, Some("1")).unwrap_err();
        assert_eq!(expired.message, "join token expired");
        assert!(!expired.message.contains(&old_clear));
        assert_eq!(
            store
                .collection::<keep::JoinToken>()
                .get(&old.meta.id)
                .unwrap()
                .status
                .uses,
            0
        );

        let (many, many_clear) =
            keep::create_join_token(&store, "many", "admin", true, None).unwrap();
        keep::redeem_join_token(&store, &many_clear).unwrap();
        let mut multi = HeaderMap::new();
        multi.insert(JOIN_HEADER, many_clear.parse().unwrap());
        enforce_join_value(&store, &multi, Some("1")).unwrap();
        assert_eq!(
            store
                .collection::<keep::JoinToken>()
                .get(&many.meta.id)
                .unwrap()
                .status
                .uses,
            1
        );
    }
}
