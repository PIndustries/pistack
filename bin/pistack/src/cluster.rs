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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use pistack_cluster::{
    accepts_writes, check_advertise, check_follower_tip, classify, consider_pre_vote,
    consider_vote, election_seed, election_timeout, follow_candidate, in_leader_lease,
    join_membership, lost_quorum, pick_leader, scan_targets, seed_membership, should_campaign,
    sweep, Announce, Ballot, Formation, JoinRequest, Member, PeerView, Role, VoteRequest,
    VoteResponse, DISCOVER_PORT,
};
use pistack_rpc::{JOIN_HEADER, REQUIRE_JOIN_ENV};
use pistack_store::{LogEntry, Store};
use pistack_types::{ApiError, Leadership};
use serde::Deserialize;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

const FOLLOW_TICK: Duration = Duration::from_millis(200);
const RPC_TIMEOUT: Duration = Duration::from_secs(2);
const RPC_CONNECT_TIMEOUT: Duration = Duration::from_millis(400);
/// A peer status probe is best effort. A dark peer must not stall a tick.
const PROBE_TIMEOUT: Duration = Duration::from_millis(600);

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
    /// Shared with ModuleContext so controllers idle on followers.
    pub leadership: Leadership,
    /// Last successful pull, incoming current-term vote, or win.
    pub last_heard: Mutex<Instant>,
    /// How long a follower waits after [`Self::last_heard`] before campaigning.
    pub election_timeout: Mutex<Duration>,
    /// Last proof a leader is alive: a pull from it on a follower, a voter
    /// majority answering on the leader. Votes inside this lease are refused.
    pub leader_contact: Mutex<Option<Instant>>,
    /// Serialises read-modify-write of the stored term and vote.
    pub ballot: Mutex<()>,
    /// Last replication error, so a stuck follower logs it once per change.
    pub pull_error: Mutex<Option<String>>,
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
    #[serde(default)]
    term: u64,
    #[serde(default)]
    commit: u64,
}

#[derive(Deserialize)]
struct LogQuery {
    #[serde(default)]
    after: u64,
    /// The follower's term at `after`, 0 when it has no row there.
    #[serde(default)]
    term: u64,
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
    // Only a lone voter resumes leadership from disk. With more voters, a
    // leader that was stopped may have been replaced while it was down, so it
    // starts as a follower and finds the current leader or wins a new term.
    let lone_voter = voters == 1 && me.role == Role::Voter;
    let sole = lone_voter && members.len() == 1;
    let leader_id: Option<String> = store.get_meta("cluster_leader_id")?;
    let leader = sole || (lone_voter && leader_id.as_deref() == Some(node_id));
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
        // Empty means unknown: the follow loop asks the voters who leads.
        leader_api: if leader {
            self_api.to_owned()
        } else {
            leader_id
                .as_deref()
                .filter(|id| *id != node_id)
                .and_then(|id| members.iter().find(|member| member.id == id))
                .map(|member| member.api.clone())
                .unwrap_or_default()
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

#[allow(clippy::too_many_arguments)]
pub fn handle_from(
    node_id: String,
    self_api: String,
    cluster_id: String,
    store: Store,
    db_path: PathBuf,
    established: &Established,
    advertise: Option<IpAddr>,
    leadership: Leadership,
) -> ClusterHandle {
    let raft = advertise
        .map(|ip| format!("{ip}:18446"))
        .unwrap_or_else(|| "0.0.0.0:18446".into());
    let announce = Announce {
        cluster_id: cluster_id.clone(),
        node_id: node_id.clone(),
        api: announced_api(&established.leader_api, &self_api),
        raft: raft.clone(),
    };
    let (tx, _rx) = watch::channel(announce);
    leadership.set(established.leader);
    let timeout = election_timeout(election_seed(&node_id, 0, now_nanos()));
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
        leadership,
        last_heard: Mutex::new(Instant::now()),
        election_timeout: Mutex::new(timeout),
        leader_contact: Mutex::new(None),
        ballot: Mutex::new(()),
        pull_error: Mutex::new(None),
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
    if query.after > 0 {
        let commit = handle.store.commit_index().map_err(store_err)?;
        let ours = handle.store.log_term_at(query.after).map_err(store_err)?;
        check_follower_tip(query.after, query.term, commit, ours)
            .map_err(|err| ApiError::conflict(err.to_string()))?;
    }
    let entries = handle.store.read_log(query.after, 64).map_err(store_err)?;
    Ok(Json(entries))
}

/// Pre-vote and vote. While this node has fresh proof of a live leader it
/// refuses both and keeps its term, so a candidate that cannot reach the
/// leader does not depose it. A real vote is only asked for after a voter
/// majority said yes to the pre-vote.
async fn vote(
    State(handle): State<std::sync::Arc<ClusterHandle>>,
    headers: HeaderMap,
    Json(req): Json<VoteRequest>,
) -> Result<Json<VoteResponse>, ApiError> {
    enforce_join(&handle.store, &headers)?;
    let refuse = |handle: &ClusterHandle| {
        Json(VoteResponse {
            term: handle.term.load(Ordering::Relaxed),
            granted: false,
        })
    };
    if *guard(&handle.role) == Role::Learner {
        return Ok(refuse(&handle));
    }
    if in_leader_lease(since_contact(&handle)) {
        tracing::debug!(
            candidate = %req.candidate_id,
            term = req.term,
            pre = req.pre,
            "vote refused: the leader is live"
        );
        return Ok(refuse(&handle));
    }
    let (before, response) = {
        let _ballot = guard(&handle.ballot);
        let mut ballot = load_ballot(&handle.store).map_err(store_err)?;
        if req.pre {
            return Ok(Json(consider_pre_vote(&ballot, &req)));
        }
        let before = ballot.current_term;
        let response = consider_vote(&mut ballot, &req);
        save_ballot(&handle.store, &ballot).map_err(store_err)?;
        handle.term.store(ballot.current_term, Ordering::Relaxed);
        (before, response)
    };
    if response.term > before && handle.leader.load(Ordering::Relaxed) {
        stand_down(&handle, "a candidate started a newer term");
    }
    if response.granted {
        *guard(&handle.last_heard) = Instant::now();
        if let Some(api) = follow_candidate(&members_of(&handle.store)?, &req.candidate_id) {
            *guard(&handle.leader_api) = api;
            publish(&handle);
        }
    }
    Ok(Json(response))
}

/// Pull entries after our commit index. The leader checks our term at that
/// index first and answers 409 when our log forked from its own.
pub async fn pull_log(leader: &str, store: &Store) -> anyhow::Result<()> {
    let after = store.commit_index()?;
    let term = store.log_term_at(after)?.unwrap_or(0);
    let url = format!(
        "{}/cluster/log?after={after}&term={term}",
        leader.trim_end_matches('/')
    );
    let response = with_join(rpc_client().get(&url))
        .timeout(RPC_TIMEOUT)
        .send()
        .await?;
    if response.status().as_u16() == StatusCode::CONFLICT.as_u16() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("leader refused the pull: {body}");
    }
    let entries: Vec<LogEntry> = response.error_for_status()?.json().await?;
    if entries.is_empty() {
        return Ok(());
    }
    store.apply_entries(&entries)?;
    Ok(())
}

/// One loop per node.
///
/// A leader asks the other voters for their status each tick. It steps down
/// when one reports a newer term, or when no voter majority has answered for
/// a full election window.
///
/// A follower checks that its leader still leads a term at least as new as
/// its own, then pulls. When that fails it asks every voter and follows the
/// newest leader it hears, so a node that comes back, or that granted a vote
/// to a candidate that lost, finds the real leader instead of campaigning.
/// Hearing from the leader resets the election timer even when a pull
/// fails; a replication error is logged, it does not start an election.
/// Elections run a pre-vote first, so a failed attempt does not raise terms.
async fn follow(handle: std::sync::Arc<ClusterHandle>, shutdown: CancellationToken) {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(FOLLOW_TICK) => {}
        }
        if shutdown.is_cancelled() {
            break;
        }
        if handle.leader.load(Ordering::Relaxed) {
            lead(&handle).await;
            continue;
        }
        let known = locked(&handle.leader_api);
        let mut heard = false;
        if !known.is_empty() && known != handle.self_api {
            match refresh(&handle, &known).await {
                Ok(()) => heard = true,
                Err(err) => tracing::debug!(leader = %known, error = %err, "leader check failed"),
            }
        }
        if !heard {
            if let Some(found) = find_leader(&handle).await {
                match refresh(&handle, &found).await {
                    Ok(()) => {
                        if found != known {
                            tracing::info!(
                                node = %handle.node_id,
                                leader = %found,
                                term = handle.term.load(Ordering::Relaxed),
                                "following the current leader"
                            );
                        }
                    }
                    Err(err) => {
                        tracing::debug!(leader = %found, error = %err, "leader check failed")
                    }
                }
            }
        }
        let silent = guard(&handle.last_heard).elapsed();
        let timeout = *guard(&handle.election_timeout);
        if should_campaign(silent, timeout) {
            if let Err(err) = campaign(&handle).await {
                tracing::info!(error = %err, "election did not complete");
            }
            *guard(&handle.last_heard) = Instant::now();
            *guard(&handle.election_timeout) = election_timeout(election_seed(
                &handle.node_id,
                handle.term.load(Ordering::Relaxed),
                now_nanos(),
            ));
        }
    }
}

async fn lead(handle: &ClusterHandle) {
    *guard(&handle.last_heard) = Instant::now();
    let voters = members_of(&handle.store)
        .map(|members| {
            members
                .iter()
                .filter(|member| member.role == Role::Voter)
                .count()
        })
        .unwrap_or(1);
    if voters <= 1 {
        *guard(&handle.leader_contact) = Some(Instant::now());
        return;
    }
    let term = handle.term.load(Ordering::Relaxed);
    let peers = probe_voters(handle).await;
    if !handle.leader.load(Ordering::Relaxed) {
        return;
    }
    if let Some(newer) = peers
        .iter()
        .filter(|peer| peer.term > term)
        .max_by_key(|peer| peer.term)
    {
        if let Err(err) = adopt_term(handle, newer.term) {
            tracing::warn!(error = %err, "could not store the newer term");
        }
        stand_down(handle, "a voter reports a newer term");
        if newer.leader {
            *guard(&handle.leader_api) = newer.api.clone();
            publish(handle);
        }
        return;
    }
    let answered = 1 + peers.iter().filter(|peer| peer.term <= term).count();
    if answered >= majority(voters) {
        *guard(&handle.leader_contact) = Some(Instant::now());
        return;
    }
    let since = since_contact(handle).unwrap_or(Duration::MAX);
    if lost_quorum(since) {
        stand_down(handle, "no voter majority answered");
    }
}

/// The newest leader any voter claims to be, at our term or newer.
async fn find_leader(handle: &ClusterHandle) -> Option<String> {
    let term = handle.term.load(Ordering::Relaxed);
    let views: Vec<PeerView> = probe_voters(handle)
        .await
        .into_iter()
        .map(|status| PeerView {
            api: status.api,
            term: status.term,
            leader: status.leader,
        })
        .collect();
    pick_leader(term, &views).map(|view| view.api.clone())
}

/// `/cluster` from every other voter, in parallel. Silent peers are left out.
async fn probe_voters(handle: &ClusterHandle) -> Vec<ClusterStatus> {
    let members = members_of(&handle.store).unwrap_or_default();
    let mut probes = tokio::task::JoinSet::new();
    for member in members
        .into_iter()
        .filter(|member| member.role == Role::Voter && member.id != handle.node_id)
    {
        let url = format!("{}/cluster", member.api.trim_end_matches('/'));
        probes.spawn(async move {
            match tokio::time::timeout(PROBE_TIMEOUT, rpc_get_json::<ClusterStatus>(&url)).await {
                Ok(Ok(status)) if status.node_id == member.id => Some(status),
                _ => None,
            }
        });
    }
    let mut heard = Vec::new();
    while let Some(joined) = probes.join_next().await {
        if let Ok(Some(status)) = joined {
            heard.push(status);
        }
    }
    heard
}

/// Follow `leader` when it leads a term at least as new as ours. That proof
/// renews the lease and the election timer; a failed pull after it does not
/// undo either.
async fn refresh(handle: &ClusterHandle, leader: &str) -> anyhow::Result<()> {
    let url = format!("{}/cluster", leader.trim_end_matches('/'));
    let status: ClusterStatus = rpc_get_json(&url).await?;
    if !status.leader {
        anyhow::bail!("peer is not the leader");
    }
    let term = handle.term.load(Ordering::Relaxed);
    if status.term < term {
        anyhow::bail!(
            "peer leads term {} but this node is at term {term}",
            status.term
        );
    }
    adopt_term(handle, status.term)?;
    let now = Instant::now();
    *guard(&handle.last_heard) = now;
    *guard(&handle.leader_contact) = Some(now);
    *guard(&handle.leader_api) = if status.api.is_empty() {
        leader.to_owned()
    } else {
        status.api.clone()
    };
    let leader_id = if status.node_id.is_empty() {
        status.leader_id.clone()
    } else {
        status.node_id.clone()
    };
    if !leader_id.is_empty() {
        handle.store.put_meta("cluster_leader_id", &leader_id)?;
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
    let pulled = pull_log(leader, &handle.store).await;
    note_pull(handle, leader, pulled);
    Ok(())
}

fn note_pull(handle: &ClusterHandle, leader: &str, pulled: anyhow::Result<()>) {
    let mut last = guard(&handle.pull_error);
    match pulled {
        Ok(()) => {
            if last.take().is_some() {
                tracing::info!(leader = %leader, "replication caught up");
            }
        }
        Err(err) => {
            let text = format!("{err:#}");
            if last.as_deref() == Some(text.as_str()) {
                tracing::debug!(leader = %leader, error = %text, "replication pull failed");
            } else if text.contains("diverged") {
                tracing::warn!(
                    leader = %leader,
                    error = %text,
                    "replication stopped: this store has entries the leader never had. Stop this node, remove state.redb (keep node.id), and start it with --join <leader api>"
                );
            } else {
                tracing::warn!(leader = %leader, error = %text, "replication pull failed");
            }
            *last = Some(text);
        }
    }
}

fn since_contact(handle: &ClusterHandle) -> Option<Duration> {
    guard(&handle.leader_contact).map(|at| at.elapsed())
}

/// Move to `term` when it is newer than ours and clear our vote.
fn adopt_term(handle: &ClusterHandle, term: u64) -> anyhow::Result<bool> {
    let _ballot = guard(&handle.ballot);
    let mut ballot = load_ballot(&handle.store)?;
    if term <= ballot.current_term {
        return Ok(false);
    }
    ballot.current_term = term;
    ballot.voted_for = None;
    save_ballot(&handle.store, &ballot)?;
    handle.term.store(term, Ordering::Relaxed);
    Ok(true)
}

/// Stop leading. Writes and controllers stop at once; the follow loop finds
/// who leads now.
fn stand_down(handle: &ClusterHandle, reason: &str) {
    if !handle.leader.swap(false, Ordering::Relaxed) {
        return;
    }
    handle.leadership.set(false);
    handle.store.set_record_log(false);
    *guard(&handle.leader_contact) = None;
    *guard(&handle.leader_api) = String::new();
    *guard(&handle.last_heard) = Instant::now();
    publish(handle);
    tracing::warn!(
        node = %handle.node_id,
        term = handle.term.load(Ordering::Relaxed),
        reason,
        "this node is no longer the leader"
    );
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
    let need = majority(voters.len());
    let peers: Vec<String> = voters
        .iter()
        .filter(|peer| peer.id != handle.node_id)
        .map(|peer| format!("{}/cluster/vote", peer.api.trim_end_matches('/')))
        .collect();
    let ballot = {
        let _ballot = guard(&handle.ballot);
        load_ballot(&handle.store)?
    };
    let current = ballot.current_term;
    let term = current + 1;
    let mut request = VoteRequest {
        term,
        candidate_id: handle.node_id.clone(),
        last_index: ballot.last_index,
        last_term: ballot.last_term,
        pre: true,
    };
    let (grants, newest) = ask_votes(&peers, &request, need).await;
    if newest > current {
        adopt_term(handle, newest)?;
        return Ok(());
    }
    if grants < need {
        tracing::info!(
            node = %handle.node_id,
            term,
            grants,
            voters = voters.len(),
            "pre-vote did not win a majority; term unchanged"
        );
        return Ok(());
    }
    {
        let _ballot = guard(&handle.ballot);
        let mut ballot = load_ballot(&handle.store)?;
        if ballot.current_term != current || handle.leader.load(Ordering::Relaxed) {
            return Ok(());
        }
        ballot.current_term = term;
        ballot.voted_for = Some(handle.node_id.clone());
        save_ballot(&handle.store, &ballot)?;
        handle.term.store(term, Ordering::Relaxed);
    }
    request.pre = false;
    tracing::info!(node = %handle.node_id, term, "starting election");
    let (grants, newest) = ask_votes(&peers, &request, need).await;
    if newest > term {
        adopt_term(handle, newest)?;
        return Ok(());
    }
    if handle.term.load(Ordering::Relaxed) != term || handle.leader.load(Ordering::Relaxed) {
        return Ok(());
    }
    if grants >= need && accepts_writes(Role::Voter, true) {
        become_leader(handle, term)?;
    } else {
        tracing::info!(
            node = %handle.node_id,
            term,
            grants,
            voters = voters.len(),
            "election did not win a majority"
        );
    }
    Ok(())
}

/// Send `request` to every peer at once. Returns the grants including our
/// own, and the newest term any peer reported. Stops waiting once `need`
/// grants are in.
async fn ask_votes(peers: &[String], request: &VoteRequest, need: usize) -> (usize, u64) {
    let mut grants = 1usize;
    let mut newest = 0u64;
    let mut votes = tokio::task::JoinSet::new();
    for url in peers {
        let url = url.clone();
        let request = request.clone();
        votes.spawn(async move {
            let result: anyhow::Result<VoteResponse> = rpc_post_json(&url, &request).await;
            (url, result)
        });
    }
    while let Some(joined) = votes.join_next().await {
        let Ok((url, result)) = joined else {
            continue;
        };
        match result {
            Ok(response) => {
                newest = newest.max(response.term);
                if response.granted && (request.pre || response.term == request.term) {
                    grants += 1;
                }
            }
            Err(err) => tracing::debug!(peer = %url, error = %err, "vote request failed"),
        }
        if grants >= need {
            votes.abort_all();
            break;
        }
    }
    (grants, newest)
}

fn become_leader(handle: &ClusterHandle, term: u64) -> anyhow::Result<()> {
    handle.term.store(term, Ordering::Relaxed);
    handle.leader.store(true, Ordering::Relaxed);
    handle.leadership.set(true);
    *guard(&handle.leader_api) = handle.self_api.clone();
    *guard(&handle.last_heard) = Instant::now();
    *guard(&handle.leader_contact) = Some(Instant::now());
    *guard(&handle.pull_error) = None;
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
        api: announced_api(&locked(&handle.leader_api), &handle.self_api),
        raft: handle.raft.clone(),
    };
    let _ = handle.announce.send(ann);
}

/// Discovery never announces an empty api. While the leader is unknown a
/// node names itself; a joiner that reaches a follower is told who leads.
fn announced_api(leader_api: &str, self_api: &str) -> String {
    if leader_api.is_empty() {
        self_api.to_owned()
    } else {
        leader_api.to_owned()
    }
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
        term: handle.term.load(Ordering::Relaxed),
        commit: handle.store.commit_index().map_err(store_err)?,
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
    Ok((index, store.log_term_at(index)?.unwrap_or(0)))
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

fn rpc_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(RPC_TIMEOUT)
        .connect_timeout(RPC_CONNECT_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

async fn rpc_get_json<T: serde::de::DeserializeOwned>(url: &str) -> anyhow::Result<T> {
    let response = with_join(rpc_client().get(url))
        .timeout(RPC_TIMEOUT)
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

async fn rpc_post_json<B, T>(url: &str, body: &B) -> anyhow::Result<T>
where
    B: serde::Serialize + ?Sized,
    T: serde::de::DeserializeOwned,
{
    let response = with_join(rpc_client().post(url))
        .timeout(RPC_TIMEOUT)
        .json(body)
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
            Leadership::always(),
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

        handle.leader.store(false, Ordering::Relaxed);
        let client = http_client();
        let refused = client.post(format!("{base}/mutate")).send().await.unwrap();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        let body = refused.text().await.unwrap();
        assert!(body.contains("not the leader"));
        handle.leader.store(true, Ordering::Relaxed);
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

    struct LiveVoter {
        handle: Arc<ClusterHandle>,
        api: String,
        addr: std::net::SocketAddr,
        /// Stops the HTTP server: the node goes dark to its peers.
        serve_stop: CancellationToken,
        /// Stops the follow loop.
        loop_stop: CancellationToken,
        dir: tempfile::TempDir,
    }

    impl LiveVoter {
        fn kill(&self) {
            self.serve_stop.cancel();
            self.loop_stop.cancel();
        }

        fn leads(&self) -> bool {
            self.handle.leader.load(Ordering::Relaxed)
        }

        fn term(&self) -> u64 {
            self.handle.term.load(Ordering::Relaxed)
        }
    }

    async fn serve_voter(
        id: &str,
        listener: tokio::net::TcpListener,
        members: &[Member],
        leader: bool,
        term: u64,
        leader_api: &str,
    ) -> LiveVoter {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        store.put_meta("cluster_id", &"c1".to_string()).unwrap();
        store.put_meta("node_id", &id.to_string()).unwrap();
        store.put_meta("members", &members.to_vec()).unwrap();
        store
            .put_meta("cluster_leader_id", &"sim".to_string())
            .unwrap();
        store.put_meta("cluster_term", &term).unwrap();
        store
            .put_meta("cluster_voted_for", &Some("sim".to_string()))
            .unwrap();
        store.set_log_term(term);
        if leader {
            store.set_record_log(true);
        }
        let ready = Established {
            role: Role::Voter,
            leader,
            leader_api: leader_api.to_owned(),
            term,
            members: members.to_vec(),
        };
        start_voter(id, listener, store, dir, &ready)
    }

    fn start_voter(
        id: &str,
        listener: tokio::net::TcpListener,
        store: Store,
        dir: tempfile::TempDir,
        ready: &Established,
    ) -> LiveVoter {
        let addr = listener.local_addr().unwrap();
        let api = format!("http://{addr}");
        let path = dir.path().join("state.redb");
        let handle = Arc::new(handle_from(
            id.into(),
            api.clone(),
            "c1".into(),
            store,
            path,
            ready,
            None,
            Leadership::new(ready.leader),
        ));
        let serve_stop = CancellationToken::new();
        let app = router(handle.clone())
            .route("/mutate", axum::routing::post(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                handle.clone(),
                reject_follower_writes,
            ));
        let stop = serve_stop.clone();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { stop.cancelled().await })
                .await
                .ok();
        });
        LiveVoter {
            handle,
            api,
            addr,
            serve_stop,
            loop_stop: CancellationToken::new(),
            dir,
        }
    }

    fn run(voter: &LiveVoter) {
        spawn_background(voter.handle.clone(), None, voter.loop_stop.clone());
    }

    /// Three voters at `term`, `sim` leading, every loop running.
    async fn three_voters(term: u64) -> (LiveVoter, LiveVoter, LiveVoter) {
        let first = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let third = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sim_api = format!("http://{}", first.local_addr().unwrap());
        let sim2_api = format!("http://{}", second.local_addr().unwrap());
        let sim3_api = format!("http://{}", third.local_addr().unwrap());
        let members = vec![
            Member::voter("sim", &sim_api),
            Member::voter("sim2", &sim2_api),
            Member::voter("sim3", &sim3_api),
        ];
        let sim = serve_voter("sim", first, &members, true, term, &sim_api).await;
        let sim2 = serve_voter("sim2", second, &members, false, term, &sim_api).await;
        let sim3 = serve_voter("sim3", third, &members, false, term, &sim_api).await;
        *guard(&sim2.handle.election_timeout) = Duration::from_millis(80);
        *guard(&sim3.handle.election_timeout) = Duration::from_millis(700);
        run(&sim);
        run(&sim2);
        run(&sim3);
        (sim, sim2, sim3)
    }

    async fn wait_for(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
        tokio::time::timeout(limit, async {
            while !done() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting: {what}"));
    }

    fn marker(store: &Store, key: &str) -> Option<String> {
        store.get_meta(key).unwrap()
    }

    #[tokio::test]
    async fn followers_elect_when_the_leader_rpc_goes_dark() {
        let term = 6u64;
        let (sim, sim2, sim3) = three_voters(term).await;

        let client = http_client();
        let allowed = client
            .post(format!("{}/mutate", sim.api))
            .send()
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
        tokio::time::sleep(Duration::from_millis(250)).await;

        sim.kill();
        tokio::time::sleep(Duration::from_millis(50)).await;

        wait_for(
            "followers elect after the leader RPC went dark",
            Duration::from_secs(8),
            || sim2.leads() || sim3.leads(),
        )
        .await;

        let (winner, winner_api) = if sim2.leads() {
            (&sim2, sim2.api.as_str())
        } else {
            (&sim3, sim3.api.as_str())
        };
        assert!(!(sim2.leads() && sim3.leads()));
        assert!(winner.term() > term);
        assert_ne!(winner.handle.node_id, "sim");

        let dead = client.post(format!("{}/mutate", sim.api)).send().await;
        assert!(
            dead.is_err() || dead.unwrap().status() != StatusCode::OK,
            "the stopped leader must not accept writes"
        );

        let write = client
            .post(format!("{winner_api}/mutate"))
            .send()
            .await
            .unwrap();
        assert_eq!(write.status(), StatusCode::OK);

        let status: ClusterStatus = client
            .get(format!("{winner_api}/cluster"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(status.leader);
        assert_ne!(status.leader_id, "sim");
        assert_eq!(status.term, winner.term());
        assert_eq!(status.members.len(), 3);
    }

    /// The lab sequence: stop the leader, elect, leave it down, start it
    /// again from the same store. One leader the whole time, terms stay put,
    /// the old leader follows and catches up, writes work on the winner.
    #[tokio::test]
    async fn a_returning_leader_follows_the_winner_and_terms_settle() {
        let term = 6u64;
        let (sim, sim2, sim3) = three_voters(term).await;
        sim.handle
            .store
            .put_meta("before", &"one".to_string())
            .unwrap();
        wait_for(
            "followers copy the first write",
            Duration::from_secs(3),
            || {
                marker(&sim2.handle.store, "before").is_some()
                    && marker(&sim3.handle.store, "before").is_some()
            },
        )
        .await;

        sim.kill();
        wait_for("a survivor wins", Duration::from_secs(8), || {
            sim2.leads() || sim3.leads()
        })
        .await;
        let (winner, loser) = if sim2.leads() {
            (&sim2, &sim3)
        } else {
            (&sim3, &sim2)
        };
        winner
            .handle
            .store
            .put_meta("lab-ha-check", &"yes".to_string())
            .unwrap();
        wait_for(
            "the loser copies the marker",
            Duration::from_secs(3),
            || marker(&loser.handle.store, "lab-ha-check").is_some(),
        )
        .await;

        // The old leader stays down. Survivors hold one term.
        let settled = winner.term();
        assert_eq!(loser.term(), settled);
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert!(
                winner.leads(),
                "the winner lost leadership with a stable majority"
            );
            assert!(!loser.leads());
        }
        assert_eq!(winner.term(), settled, "survivors kept electing");
        assert_eq!(loser.term(), settled, "survivors kept electing");

        // Restart the old leader on its own store and address. Recording and
        // the log term are process memory, so a new process starts without them.
        let store = sim.handle.store.clone();
        store.set_record_log(false);
        store.set_log_term(0);
        let ready = match establish_local(&store, "sim", &sim.api, true).unwrap() {
            LocalRole::Ready(ready) => ready,
            LocalRole::NeedJoin => panic!("the old leader lost its membership"),
        };
        assert!(
            !ready.leader,
            "a returning voter must not resume leadership"
        );
        assert_eq!(
            ready.leader_api, "",
            "the old leader does not know who leads"
        );
        let LiveVoter { addr, dir, .. } = sim;
        let listener = loop {
            match tokio::net::TcpListener::bind(addr).await {
                Ok(listener) => break listener,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        let back = start_voter("sim", listener, store, dir, &ready);
        run(&back);

        for _ in 0..120 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            let leaders = [&back, &sim2, &sim3]
                .iter()
                .filter(|voter| voter.leads())
                .count();
            assert_eq!(leaders, 1, "exactly one node may claim leadership");
            assert!(
                winner.leads(),
                "leadership flapped after the old leader returned"
            );
        }
        assert_eq!(winner.term(), settled);
        assert_eq!(loser.term(), settled);
        assert_eq!(back.term(), settled);
        assert_eq!(locked(&back.handle.leader_api), winner.api);
        assert_eq!(
            marker(&back.handle.store, "lab-ha-check").as_deref(),
            Some("yes")
        );
        assert_eq!(
            back.handle.store.commit_index().unwrap(),
            winner.handle.store.commit_index().unwrap()
        );

        let client = http_client();
        let write = client
            .post(format!("{}/mutate", winner.api))
            .send()
            .await
            .unwrap();
        assert_eq!(write.status(), StatusCode::OK);
        let refused = client
            .post(format!("{}/mutate", back.api))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert!(refused.text().await.unwrap().contains(&winner.api));
    }

    /// A leader whose API went dark but whose process kept running learns the
    /// newer term from the voters, stops leading, and follows the winner.
    #[tokio::test]
    async fn a_leader_that_was_dark_steps_down_for_a_newer_term() {
        let (sim, sim2, sim3) = three_voters(6).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        sim.serve_stop.cancel();
        wait_for("a survivor wins", Duration::from_secs(8), || {
            sim2.leads() || sim3.leads()
        })
        .await;
        let winner = if sim2.leads() { &sim2 } else { &sim3 };
        wait_for(
            "the dark leader stands down",
            Duration::from_secs(3),
            || !sim.leads(),
        )
        .await;
        assert!(!sim.handle.leadership.is_leader());
        wait_for("the dark leader follows", Duration::from_secs(3), || {
            locked(&sim.handle.leader_api) == winner.api
        })
        .await;
        assert_eq!(sim.term(), winner.term());
        assert!(winner.leads());
        sim.loop_stop.cancel();
    }

    /// A pre-vote moves no term, and a node with a live leader refuses it.
    #[tokio::test]
    async fn a_live_leader_lease_refuses_votes_without_moving_the_term() {
        let (sim, sim2, sim3) = three_voters(6).await;
        *guard(&sim2.handle.election_timeout) = Duration::from_secs(60);
        *guard(&sim3.handle.election_timeout) = Duration::from_secs(60);
        wait_for("followers hear the leader", Duration::from_secs(3), || {
            since_contact(&sim3.handle).is_some()
        })
        .await;
        let client = http_client();
        let ask = |pre| VoteRequest {
            term: 9,
            candidate_id: "sim2".into(),
            last_index: 99,
            last_term: 99,
            pre,
        };
        for (api, pre) in [(&sim3.api, true), (&sim3.api, false), (&sim.api, false)] {
            let reply: VoteResponse = client
                .post(format!("{api}/cluster/vote"))
                .json(&ask(pre))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert!(!reply.granted);
            assert_eq!(reply.term, 6);
        }
        assert!(sim.leads());
        assert_eq!(sim.term(), 6);
        assert_eq!(sim3.term(), 6);
        sim.kill();
        sim2.kill();
        sim3.kill();
    }

    #[test]
    fn a_voter_in_a_three_voter_cluster_starts_as_a_follower() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let members = vec![
            Member::voter("sim", "http://sim"),
            Member::voter("sim2", "http://sim2"),
            Member::voter("sim3", "http://sim3"),
        ];
        store.put_meta("members", &members).unwrap();
        store
            .put_meta("cluster_leader_id", &"sim".to_string())
            .unwrap();
        store.put_meta("cluster_term", &6u64).unwrap();
        let LocalRole::Ready(old) = establish_local(&store, "sim", "http://sim", true).unwrap()
        else {
            panic!("member had to join");
        };
        assert!(!old.leader);
        assert_eq!(old.leader_api, "");
        assert_eq!(old.term, 6);
        let LocalRole::Ready(peer) = establish_local(&store, "sim2", "http://sim2", true).unwrap()
        else {
            panic!("member had to join");
        };
        assert!(!peer.leader);
        assert_eq!(peer.leader_api, "http://sim");
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
