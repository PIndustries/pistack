// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Control-plane membership.
//!
//! A machine that already has a store stays that cluster and announces it.
//! A machine with no store, on a private IPv4 listen address, sweeps its /24.
//! One cluster means adopt it. None means found one. Two means wait for
//! `--join`. Every server that joins replicates as a learner: it never
//! campaigns and never grants a vote. The leader promotes learners once
//! they have caught up, until three servers are voters. A store copied from
//! another server, or downloaded as a snapshot, is never trusted as this
//! server's vote: it rejoins as a learner.
//!
//! A write is confirmed before it is answered. The leader applies and logs
//! it, then holds the response until a majority of voters has the entry
//! ([`confirm_write`]). A follower's pull names the index it has applied,
//! which is its acknowledgement, and the pull waits briefly on the leader
//! when there is nothing new, so an acknowledgement follows a write in
//! milliseconds. A candidate needs a log at least as current as a majority's
//! to win, so the next leader holds every confirmed write. A write that is
//! not confirmed in time is answered 503: it is in the leader's log and may
//! still take effect, but nothing was promised.

use std::collections::HashMap;
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
    consider_vote, election_seed, election_timeout, follow_candidate, in_leader_lease, is_voter,
    join_membership, lost_quorum, pick_leader, plan_promotions, scan_targets, seed_membership,
    should_campaign, sweep, Announce, Ballot, Formation, JoinRequest, Member, PeerView, Role,
    VoteRequest, VoteResponse, DISCOVER_PORT,
};
use pistack_rpc::{JOIN_HEADER, REQUIRE_JOIN_ENV};
use pistack_store::{LogEntry, Store};
use pistack_types::{ApiError, Leadership};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

const FOLLOW_TICK: Duration = Duration::from_millis(200);
const RPC_TIMEOUT: Duration = Duration::from_secs(2);
const RPC_CONNECT_TIMEOUT: Duration = Duration::from_millis(400);
/// A peer status probe is best effort. A dark peer must not stall a tick.
const PROBE_TIMEOUT: Duration = Duration::from_millis(600);
/// How long a write waits for a voter majority to hold it.
const CONFIRM_WAIT: Duration = Duration::from_secs(5);
/// How long an empty `/cluster/log` pull waits on the leader for a new
/// entry. Shorter than [`FOLLOW_TICK`], so a follower's next tick starts a
/// fresh pull, and far shorter than [`RPC_TIMEOUT`].
const LOG_WAIT: Duration = Duration::from_millis(150);
/// Entries one `/cluster/log` page carries.
const LOG_PAGE: usize = 64;
/// A learner is caught up when its last pull asked for entries no more than
/// this far behind the leader's commit index.
const PROMOTE_WINDOW: u64 = LOG_PAGE as u64;
/// A learner's pull counts toward promotion only this soon after it.
const PROGRESS_FRESH: Duration = Duration::from_secs(3);
/// A follower that pulled this recently keeps the log rows it still needs.
const PROGRESS_PIN: Duration = Duration::from_secs(600);
/// Log rows every server keeps behind its commit index after compaction.
const LOG_KEEP: u64 = 10_000;
/// A slow follower never makes the leader keep more than this many rows.
const LOG_KEEP_MAX: u64 = 200_000;
/// How often a server trims its log, and how many rows one pass removes.
const COMPACT_EVERY: Duration = Duration::from_secs(30);
const COMPACT_MAX_ROWS: u64 = 50_000;
/// Bytes per piece of a snapshot response.
const SNAPSHOT_CHUNK: usize = 256 * 1024;
/// A snapshot download fails when no bytes arrive for this long. There is
/// no limit on the whole transfer: a slow link that keeps moving finishes.
const SNAPSHOT_STALL: Duration = Duration::from_secs(30);
/// Downloads that broke after bytes arrived are restarted this many times.
const SNAPSHOT_RESTARTS: u32 = 3;
/// How long to keep retrying a leader that is not answering yet.
const SNAPSHOT_STARTUP: Duration = Duration::from_secs(30);

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
    /// True while a background pull from the leader runs. The follow loop
    /// keeps checking the leader every tick meanwhile, so a slow pull never
    /// looks like a silent leader, and it does not campaign until the pull
    /// is done.
    pub pulling: AtomicBool,
    /// On the leader: each follower's last pull, as (index it asked after, when).
    pub progress: Mutex<HashMap<String, (u64, Instant)>>,
    /// Last log compaction pass.
    pub last_compact: Mutex<Instant>,
    /// Rows kept behind the commit index; [`LOG_KEEP`] outside tests.
    pub log_keep: AtomicU64,
    /// Woken when a follower's pull acknowledges entries.
    pub acks: tokio::sync::Notify,
    /// Woken when an API write lands, so a waiting pull answers at once.
    pub log_grew: tokio::sync::Notify,
    /// How long a write waits for a majority; [`CONFIRM_WAIT`] outside tests.
    pub confirm_wait: Mutex<Duration>,
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
    /// The puller's node id, so the leader can tell when a learner caught up
    /// and which voters hold a write.
    #[serde(default)]
    node: String,
    /// The puller's current term, 0 when it does not say. A puller in a
    /// newer term has voted this leader out: its pull is not an
    /// acknowledgement.
    #[serde(default)]
    fterm: u64,
    /// Milliseconds to wait for a new entry when there is none, capped at
    /// [`LOG_WAIT`].
    #[serde(default)]
    wait: u64,
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
    let owner: Option<String> = store.get_meta("node_id")?;
    if let Some(owner) = owner.filter(|owner| owner != node_id) {
        // This store was written by another server: a snapshot download or
        // an offline copy. Its term is the cluster's and may stay, but its
        // vote and leadership were that server's. Forget the vote and rejoin
        // as a learner, so this server cannot campaign or vote on a log it
        // never checked with the leader.
        tracing::warn!(
            store_owner = %owner,
            node_id,
            "state.redb was written by another server; rejoining as a learner"
        );
        forget_copied_vote(store)?;
        return Ok(LocalRole::NeedJoin);
    }
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

/// Clear the node-local parts of a store that came from another server: its
/// vote. The term stays (it is the cluster's, and keeping it means the
/// joiner never raises it), and the leader id stays as a hint for the join.
fn forget_copied_vote(store: &Store) -> anyhow::Result<()> {
    store.put_meta("cluster_voted_for", &None::<String>)?;
    Ok(())
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
    forget_copied_vote(store)?;
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
        pulling: AtomicBool::new(false),
        progress: Mutex::new(HashMap::new()),
        last_compact: Mutex::new(Instant::now()),
        log_keep: AtomicU64::new(LOG_KEEP),
        acks: tokio::sync::Notify::new(),
        log_grew: tokio::sync::Notify::new(),
        confirm_wait: Mutex::new(CONFIRM_WAIT),
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
    // Agents must reach the leader for every call, reads included: a
    // follower's command queue never receives anything, so a long-poll
    // there would wait forever. They follow the leader address below.
    let rpc = path == "/rpc" || path.starts_with("/rpc/");
    if (read && !rpc) || path == "/healthz" || path == "/" || path.starts_with("/cluster") {
        return next.run(req).await;
    }
    if handle.leader.load(Ordering::Acquire) {
        if read {
            return next.run(req).await;
        }
        let term = handle.term.load(Ordering::Relaxed);
        let before = handle.store.commit_index().ok();
        let response = next.run(req).await;
        let after = match handle.store.commit_index() {
            Ok(after) => after,
            Err(err) => return store_err(err).into_response(),
        };
        if before == Some(after) {
            // Nothing was logged while this request ran.
            return response;
        }
        return match confirm_write(&handle, after, term).await {
            Ok(()) => response,
            Err(err) => err.into_response(),
        };
    }
    if rpc {
        // Agents act on the address, so only name a leader this follower
        // heard from within an election window. During an election there
        // is none yet; the agent retries shortly.
        return match fresh_leader(&handle) {
            Some(leader) => ApiError::conflict("not the leader")
                .with_detail("leader", leader)
                .into_response(),
            None => ApiError::unavailable("no leader yet, retry soon").into_response(),
        };
    }
    let leader = locked(&handle.leader_api);
    ApiError::conflict("not the leader")
        .with_detail("leader", leader)
        .into_response()
}

/// Wait until a majority of voters holds the log through `index`, written in
/// `term`. This server counts as one. A follower holds it once a pull of its
/// asked for entries after `index` or later.
///
/// Fails when this server stops leading, the term moves, or no majority
/// confirms within `confirm_wait`. The entry stays in this server's log
/// either way: the caller is told nothing was promised, not that nothing
/// happened.
async fn confirm_write(handle: &ClusterHandle, index: u64, term: u64) -> Result<(), ApiError> {
    handle.log_grew.notify_waiters();
    let deadline = Instant::now() + *guard(&handle.confirm_wait);
    loop {
        let acked = handle.acks.notified();
        tokio::pin!(acked);
        // Register before counting, so an acknowledgement between the count
        // and the wait is not missed.
        acked.as_mut().enable();
        if !handle.leader.load(Ordering::Acquire) || handle.term.load(Ordering::Relaxed) != term {
            return Err(ApiError::unavailable(
                "this server stopped leading before a majority of voters confirmed the write; it may or may not take effect",
            )
            .with_detail("index", index));
        }
        let members = members_of(&handle.store)?;
        let voters: Vec<&Member> = members
            .iter()
            .filter(|member| member.role == Role::Voter)
            .collect();
        let have = {
            let progress = guard(&handle.progress);
            voters
                .iter()
                .filter(|voter| {
                    voter.id == handle.node_id
                        || progress
                            .get(&voter.id)
                            .is_some_and(|(after, _)| *after >= index)
                })
                .count()
        };
        // A store with no voter listed is a single server.
        if voters.len() <= 1 || have >= majority(voters.len()) {
            return Ok(());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ApiError::unavailable(
                "a majority of voters did not confirm the write in time; it may or may not take effect",
            )
            .with_detail("index", index)
            .with_detail("confirmed_by", have)
            .with_detail("voters", voters.len()));
        }
        // Wake on an acknowledgement, and look again every so often for a
        // stand-down or a membership change.
        let _ = tokio::time::timeout(left.min(Duration::from_millis(100)), acked).await;
    }
}

/// The leader this follower tracks, if it heard from it within the current
/// election timeout. Reads the follow loop's state; changes nothing.
fn fresh_leader(handle: &ClusterHandle) -> Option<String> {
    let leader = locked(&handle.leader_api);
    if leader.is_empty() || leader == handle.self_api {
        return None;
    }
    let window = *guard(&handle.election_timeout);
    match since_contact(handle) {
        Some(quiet) if quiet < window => Some(leader),
        _ => None,
    }
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
    if !handle.leader.load(Ordering::Acquire) {
        return Err(not_leader(&handle));
    }
    let node_id = body.node_id.trim();
    let api = body.api.trim();
    if node_id.is_empty() || api.is_empty() {
        return Err(ApiError::bad_request("node_id and api are required"));
    }
    if node_id == handle.node_id {
        return Err(ApiError::conflict("a server cannot join itself"));
    }
    let current = members_of(&handle.store)?;
    let next = join_membership(&current, node_id, api);
    handle.store.put_meta("members", &next).map_err(store_err)?;
    guard(&handle.progress).remove(node_id);
    tracing::info!(node = %node_id, api = %api, "server joined as a learner");
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
    let remove = RemoveFile(dest.clone());
    let store = handle.store.clone();
    let src = handle.db_path.clone();
    let target = dest.clone();
    tokio::task::spawn_blocking(move || store.snapshot_file(&src, &target))
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?
        .map_err(store_err)?;
    let file = tokio::fs::File::open(&dest)
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?;
    let len = file
        .metadata()
        .await
        .map_err(|err| ApiError::internal(err.to_string()))?
        .len();
    // The open file keeps its bytes; the name goes now so nothing is left
    // behind however the transfer ends.
    drop(remove);
    tracing::info!(bytes = len, "sending a snapshot");
    let stream = tokio_util::io::ReaderStream::with_capacity(file, SNAPSHOT_CHUNK);
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, len)
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()))
}

async fn log(
    State(handle): State<std::sync::Arc<ClusterHandle>>,
    headers: HeaderMap,
    Query(query): Query<LogQuery>,
) -> Result<Json<Vec<LogEntry>>, ApiError> {
    enforce_join(&handle.store, &headers)?;
    let term = handle.term.load(Ordering::Relaxed);
    if query.fterm > term {
        // The puller has voted in a newer term. Its pull must not count as
        // an acknowledgement for a leader that term replaced.
        if let Err(err) = adopt_term(&handle, query.fterm) {
            tracing::warn!(error = %err, "could not store the newer term");
        }
        stand_down(&handle, "a follower reports a newer term");
        return Err(ApiError::conflict(format!(
            "stale term: the puller is at term {}, this server at term {term}",
            query.fterm
        )));
    }
    if query.after > 0 {
        let commit = handle.store.commit_index().map_err(store_err)?;
        let ours = handle.store.log_term_at(query.after).map_err(store_err)?;
        check_follower_tip(query.after, query.term, commit, ours)
            .map_err(|err| ApiError::conflict(err.to_string()))?;
    }
    let start = handle.store.log_start().map_err(store_err)?;
    if query.after.saturating_add(1) < start {
        return Err(ApiError::conflict(format!(
            "compacted: this server needs entries after {}, the leader keeps entries from {start}",
            query.after
        )));
    }
    let node = query.node.trim();
    if !node.is_empty() {
        guard(&handle.progress).insert(node.to_owned(), (query.after, Instant::now()));
        handle.acks.notify_waiters();
    }
    let grew = handle.log_grew.notified();
    tokio::pin!(grew);
    // Register before reading, so a write between the read and the wait is
    // not missed.
    grew.as_mut().enable();
    let mut entries = handle
        .store
        .read_log(query.after, LOG_PAGE)
        .map_err(store_err)?;
    if entries.is_empty() && query.wait > 0 {
        let wait = Duration::from_millis(query.wait).min(LOG_WAIT);
        if tokio::time::timeout(wait, grew).await.is_ok() {
            entries = handle
                .store
                .read_log(query.after, LOG_PAGE)
                .map_err(store_err)?;
        }
    }
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
    // A server that is not a voter in its own membership (a copied or
    // half-joined store) never votes, and nobody votes for a server that is
    // not a voter here.
    let members = members_of(&handle.store)?;
    if !is_voter(&members, &handle.node_id) || !is_voter(&members, &req.candidate_id) {
        tracing::debug!(
            candidate = %req.candidate_id,
            term = req.term,
            "vote refused: not between voters"
        );
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
#[cfg(test)]
pub async fn pull_log(leader: &str, store: &Store) -> anyhow::Result<()> {
    pull_page(leader, store, "", 0, Duration::ZERO)
        .await
        .map(|_| ())
}

/// One page of entries after our commit index. Returns how many were applied.
/// Naming that index tells the leader we hold everything through it.
/// `current_term` is this server's term (0 leaves it unsaid), and `wait` is
/// how long the leader may hold an empty answer for a new entry.
async fn pull_page(
    leader: &str,
    store: &Store,
    node: &str,
    current_term: u64,
    wait: Duration,
) -> anyhow::Result<usize> {
    let after = store.commit_index()?;
    let term = store.log_term_at(after)?.unwrap_or(0);
    let url = format!(
        "{}/cluster/log?after={after}&term={term}&node={node}&fterm={current_term}&wait={}",
        leader.trim_end_matches('/'),
        wait.as_millis()
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
        return Ok(0);
    }
    store.apply_entries(&entries)?;
    Ok(entries.len())
}

/// Pull from `leader` in the background until a page comes back empty. Only
/// one runs at a time. A page that brought entries is followed by another
/// pull straight away: that pull is what tells the leader we hold them. The follow loop keeps checking the leader each tick
/// while it runs, so the election timer and the lease track the leader, not
/// how long replication takes.
fn start_pull(handle: &std::sync::Arc<ClusterHandle>, leader: &str) {
    if handle.pulling.swap(true, Ordering::AcqRel) {
        return;
    }
    let handle = handle.clone();
    let leader = leader.to_owned();
    tokio::spawn(async move {
        let pulled = loop {
            if handle.leader.load(Ordering::Acquire) {
                break Ok(());
            }
            let term = handle.term.load(Ordering::Relaxed);
            match pull_page(&leader, &handle.store, &handle.node_id, term, LOG_WAIT).await {
                Ok(n) if n > 0 => continue,
                Ok(_) => break Ok(()),
                Err(err) => break Err(err),
            }
        };
        note_pull(&handle, &leader, pulled);
        handle.pulling.store(false, Ordering::Release);
    });
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
        maybe_compact(&handle);
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
        // A pull still running means the leader answered moments ago; let it
        // finish (it is bounded by the RPC timeout) before deciding.
        if should_campaign(silent, timeout) && !handle.pulling.load(Ordering::Acquire) {
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
    promote_caught_up(handle);
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

/// Promote learners whose last pull was within [`PROMOTE_WINDOW`] of our
/// commit index, as [`plan_promotions`] allows. The membership change is an
/// ordinary logged write, so every server copies it.
fn promote_caught_up(handle: &ClusterHandle) {
    let Ok(members) = members_of(&handle.store) else {
        return;
    };
    if !members.iter().any(|member| member.role == Role::Learner) {
        return;
    }
    let Ok(commit) = handle.store.commit_index() else {
        return;
    };
    let progress = guard(&handle.progress).clone();
    let caught_up = |id: &str| {
        progress.get(id).is_some_and(|(after, at)| {
            at.elapsed() < PROGRESS_FRESH && commit.saturating_sub(*after) <= PROMOTE_WINDOW
        })
    };
    let Some(next) = plan_promotions(&members, caught_up) else {
        return;
    };
    if !handle.leader.load(Ordering::Acquire) {
        return;
    }
    match handle.store.put_meta("members", &next) {
        Ok(()) => {
            for member in next
                .iter()
                .filter(|member| member.role == Role::Voter && !is_voter(&members, &member.id))
            {
                tracing::info!(node = %member.id, api = %member.api, "learner caught up; now a voter");
            }
        }
        Err(err) => tracing::warn!(error = %err, "could not promote caught-up learners"),
    }
}

/// Trim log rows every [`COMPACT_EVERY`]. Every server keeps the last
/// `log_keep` rows. The leader also keeps what a follower that pulled in the
/// last [`PROGRESS_PIN`] still needs, up to [`LOG_KEEP_MAX`] rows. Runs on a
/// blocking thread; one pass removes at most [`COMPACT_MAX_ROWS`].
fn maybe_compact(handle: &std::sync::Arc<ClusterHandle>) {
    {
        let mut last = guard(&handle.last_compact);
        if last.elapsed() < COMPACT_EVERY {
            return;
        }
        *last = Instant::now();
    }
    let handle = handle.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(err) = compact_now(&handle) {
            tracing::warn!(error = %err, "log compaction failed");
        }
    });
}

/// One compaction pass. Returns the rows removed.
fn compact_now(handle: &ClusterHandle) -> anyhow::Result<u64> {
    let commit = handle.store.commit_index()?;
    let keep = handle.log_keep.load(Ordering::Relaxed).max(1);
    let mut upto = commit.saturating_sub(keep);
    if handle.leader.load(Ordering::Acquire) {
        let members = members_of(&handle.store).map_err(|err| anyhow::anyhow!("{err}"))?;
        let floor = commit.saturating_sub(LOG_KEEP_MAX.max(keep));
        let progress = guard(&handle.progress).clone();
        if let Some(slowest) = members
            .iter()
            .filter(|member| member.id != handle.node_id)
            .filter_map(|member| progress.get(&member.id))
            .filter(|(_, at)| at.elapsed() < PROGRESS_PIN)
            .map(|(after, _)| *after)
            .min()
        {
            upto = upto.min(slowest.max(floor));
        }
    }
    let removed = handle.store.compact_log(upto, COMPACT_MAX_ROWS)?;
    if removed > 0 {
        tracing::info!(
            removed,
            log_start = handle.store.log_start()?,
            commit,
            "trimmed replication log"
        );
    }
    Ok(removed)
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
async fn refresh(handle: &std::sync::Arc<ClusterHandle>, leader: &str) -> anyhow::Result<()> {
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
    start_pull(handle, leader);
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
            } else if text.contains("compacted") {
                tracing::warn!(
                    leader = %leader,
                    error = %text,
                    "replication stopped: the leader no longer keeps the entries this store needs. Stop this node, move state.redb aside (keep node.id), and start it with --join <leader api>"
                );
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
///
/// The reverse of [`become_leader`]: the flag and the signal drop first,
/// then the store write fence rises, then logging stops. The fence refuses
/// ordinary writes that were already in flight, so they cannot land
/// unlogged after logging stops. Logging cannot stay on longer than this,
/// because the follow loop's next pass stores the new leader id and
/// membership, and a follower that logged those would split from the leader
/// it follows.
fn stand_down(handle: &ClusterHandle, reason: &str) {
    if !handle.leader.swap(false, Ordering::Relaxed) {
        return;
    }
    handle.leadership.set(false);
    // Fence ordinary writes before logging stops so an in-flight controller
    // or API write cannot land unlogged after this point. Follow-loop
    // membership keys and apply_entries bypass the fence.
    handle.store.set_write_fence(true);
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
    // Only a voter in this store's own membership campaigns. A store with no
    // membership, or one that lists this server as a learner or not at all,
    // has nothing to campaign for.
    if !is_voter(&members, &handle.node_id) {
        tracing::info!(
            node = %handle.node_id,
            "not a voter in this store's membership; not campaigning"
        );
        return Ok(());
    }
    let voters: Vec<&Member> = members
        .iter()
        .filter(|member| member.role == Role::Voter)
        .collect();
    if voters.len() == 1 {
        // The only voter is this server: a one-voter cluster leads itself.
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

/// Take leadership for `term`.
///
/// The store starts logging writes before anything can see this node as
/// leader. The leader flag opens the API to writes, and the leadership
/// signal starts controllers that write at once (seeds, first reconcile
/// passes). A write that lands before logging is on never reaches the
/// followers, so this node's store would split from theirs. Turning logging
/// on first is safe while the node still follows: API writes and
/// controllers are held back by the flag and the signal, and the cluster
/// bookkeeping a follower writes (term and vote) is never logged.
fn become_leader(handle: &ClusterHandle, term: u64) -> anyhow::Result<()> {
    handle.term.store(term, Ordering::Relaxed);
    handle.store.set_log_term(term);
    handle.store.set_record_log(true);
    // Clear the stand-down fence only after logging is on, so a concurrent
    // writer cannot slip an unlogged row through the gap.
    handle.store.set_write_fence(false);
    // Release pairs with the Acquire loads that admit API writes, so a
    // request that sees the flag also sees logging on.
    handle.leader.store(true, Ordering::Release);
    handle.leadership.set(true);
    *guard(&handle.leader_api) = handle.self_api.clone();
    *guard(&handle.last_heard) = Instant::now();
    *guard(&handle.leader_contact) = Some(Instant::now());
    *guard(&handle.pull_error) = None;
    // Acknowledgements from an earlier time leading say nothing about what
    // followers hold now.
    guard(&handle.progress).clear();
    *guard(&handle.role) = Role::Voter;
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
    let text = err.to_string();
    if text == "not the leader" {
        return ApiError::conflict("not the leader");
    }
    ApiError::internal(text)
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

/// Limits for one snapshot download. [`Default`] is what `--join` uses.
#[derive(Debug, Clone)]
pub struct SnapshotFetch {
    /// Fail when no bytes arrive for this long.
    pub stall: Duration,
    /// Restarts after a transfer that broke part way.
    pub restarts: u32,
    /// How long to retry a leader that refuses connections or answers 5xx
    /// before sending anything.
    pub startup: Duration,
    /// Pause between attempts.
    pub retry_wait: Duration,
}

impl Default for SnapshotFetch {
    fn default() -> Self {
        Self {
            stall: SNAPSHOT_STALL,
            restarts: SNAPSHOT_RESTARTS,
            startup: SNAPSHOT_STARTUP,
            retry_wait: Duration::from_millis(500),
        }
    }
}

/// What one download moved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Downloaded {
    pub bytes: u64,
    /// The largest piece held in memory at once.
    pub largest_chunk: usize,
}

enum Attempt {
    /// Nothing usable arrived (refused, 5xx, stalled before the body).
    NotStarted(anyhow::Error),
    /// The body started and then broke.
    Broken(anyhow::Error),
    /// The leader said no (4xx). Retrying will not help.
    Refused(anyhow::Error),
}

async fn fetch_snapshot(api: &str, dest: &Path) -> anyhow::Result<()> {
    fetch_snapshot_with(api, dest, &SnapshotFetch::default())
        .await
        .map(|_| ())
}

/// Download the leader's snapshot to `dest`. The body is written to
/// `dest.partial` piece by piece as it arrives, so memory stays at one piece
/// whatever the store size. There is no limit on the whole transfer, only on
/// a stall. A broken transfer starts again from the beginning on a truncated
/// partial file. The file is synced, opened once to check it, and only then
/// renamed to `dest`, so `dest` is either absent or complete.
pub async fn fetch_snapshot_with(
    api: &str,
    dest: &Path,
    limits: &SnapshotFetch,
) -> anyhow::Result<Downloaded> {
    if dest.exists() {
        anyhow::bail!("{} already exists", dest.display());
    }
    let url = format!("{}/cluster/snapshot", api.trim_end_matches('/'));
    let partial = dest.with_extension("partial");
    let started = tokio::time::Instant::now();
    let mut restarts = 0u32;
    let result = loop {
        let _ = tokio::fs::remove_file(&partial).await;
        match download_once(&url, &partial, limits.stall).await {
            Ok(done) => match check_and_install(&partial, dest).await {
                Ok(()) => break Ok(done),
                Err(err) => {
                    if restarts >= limits.restarts {
                        break Err(err);
                    }
                    restarts += 1;
                    tracing::warn!(error = %err, restarts, "snapshot did not open; downloading again");
                }
            },
            Err(Attempt::Refused(err)) => break Err(err),
            Err(Attempt::NotStarted(err)) => {
                if started.elapsed() >= limits.startup {
                    break Err(err);
                }
                tracing::debug!(error = %err, "snapshot not available yet");
            }
            Err(Attempt::Broken(err)) => {
                if restarts >= limits.restarts {
                    break Err(err);
                }
                restarts += 1;
                tracing::warn!(error = %err, restarts, "snapshot transfer broke; starting it again");
            }
        }
        tokio::time::sleep(limits.retry_wait).await;
    };
    if result.is_err() {
        let _ = tokio::fs::remove_file(&partial).await;
    }
    result
}

async fn download_once(url: &str, partial: &Path, stall: Duration) -> Result<Downloaded, Attempt> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    // The leader writes its copy before the first byte, so the wait for
    // headers is a stall too.
    let response = match tokio::time::timeout(stall, with_join(client.get(url)).send()).await {
        Err(_) => {
            return Err(Attempt::NotStarted(anyhow::anyhow!(
                "snapshot {url}: no answer for {stall:?}"
            )))
        }
        Ok(Err(err)) => return Err(Attempt::NotStarted(err.into())),
        Ok(Ok(response)) => response,
    };
    let status = response.status();
    if status.is_client_error() {
        return Err(Attempt::Refused(anyhow::anyhow!(
            "snapshot {url} returned {status}"
        )));
    }
    if !status.is_success() {
        return Err(Attempt::NotStarted(anyhow::anyhow!(
            "snapshot {url} returned {status}"
        )));
    }
    let expected = response.content_length();
    let mut response = response;
    let broken = |err: anyhow::Error| Attempt::Broken(err);
    let mut file = tokio::fs::File::create(partial)
        .await
        .map_err(|err| broken(err.into()))?;
    let mut done = Downloaded::default();
    let mut next_note = 256u64 << 20;
    loop {
        let chunk = match tokio::time::timeout(stall, response.chunk()).await {
            Err(_) => {
                return Err(broken(anyhow::anyhow!(
                    "snapshot transfer stalled: no data for {stall:?} after {} bytes",
                    done.bytes
                )))
            }
            Ok(Err(err)) => return Err(broken(err.into())),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(chunk))) => chunk,
        };
        done.largest_chunk = done.largest_chunk.max(chunk.len());
        file.write_all(&chunk)
            .await
            .map_err(|err| broken(err.into()))?;
        done.bytes += chunk.len() as u64;
        if done.bytes >= next_note {
            tracing::info!(bytes = done.bytes, total = ?expected, "snapshot download progress");
            next_note += 256 << 20;
        }
    }
    file.flush().await.map_err(|err| broken(err.into()))?;
    file.sync_all().await.map_err(|err| broken(err.into()))?;
    if let Some(expected) = expected {
        if expected != done.bytes {
            return Err(broken(anyhow::anyhow!(
                "snapshot ended after {} of {expected} bytes",
                done.bytes
            )));
        }
    }
    Ok(done)
}

/// Open the downloaded copy once, then rename it into place and sync the
/// directory so the rename survives a crash.
async fn check_and_install(partial: &Path, dest: &Path) -> anyhow::Result<()> {
    let partial = partial.to_owned();
    let dest = dest.to_owned();
    tokio::task::spawn_blocking(move || {
        verify_db(&partial)?;
        std::fs::rename(&partial, &dest)?;
        if let Some(dir) = dest.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            if let Ok(dir) = std::fs::File::open(dir) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    })
    .await?
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
    use pistack_store::Backend;
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
        // Agents' command polls are reads, but a follower's queue never
        // fills, so they are sent to the leader too.
        let poll = client
            .get(format!("{base}/rpc/agents/n1/commands"))
            .send()
            .await
            .unwrap();
        // Not heard from the leader yet: no address, just "retry soon".
        assert_eq!(poll.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = poll.text().await.unwrap();
        assert!(body.contains("no leader yet"), "{body}");
        *guard(&handle.leader_contact) = Some(Instant::now());
        let poll = client
            .get(format!("{base}/rpc/agents/n1/commands"))
            .send()
            .await
            .unwrap();
        assert_eq!(poll.status(), StatusCode::CONFLICT);
        let body: serde_json::Value = poll.json().await.unwrap();
        assert_eq!(body["error"]["details"]["leader"], base.as_str());
        let read = client.get(format!("{base}/cluster")).send().await.unwrap();
        assert_eq!(read.status(), StatusCode::OK);
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
        assert!(third.members.iter().any(|member| member.id == "node-b"));
        let c = third.members.iter().find(|m| m.id == "node-c").unwrap();
        assert_eq!(c.role, Role::Learner, "a join never adds a voter");

        // Both learners pulled up to the leader's commit: promoted together.
        promote_caught_up(&handle);
        assert_eq!(
            members_of(&store).unwrap()[1].role,
            Role::Learner,
            "no promotion before the learners pulled"
        );
        let commit = store.commit_index().unwrap();
        for id in ["node-b", "node-c"] {
            guard(&handle.progress).insert(id.into(), (commit, Instant::now()));
        }
        promote_caught_up(&handle);
        assert!(members_of(&store)
            .unwrap()
            .iter()
            .all(|member| member.role == Role::Voter));
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
        start_voter_with(
            id,
            listener,
            store,
            dir,
            ready,
            Leadership::new(ready.leader),
        )
    }

    fn start_voter_with(
        id: &str,
        listener: tokio::net::TcpListener,
        store: Store,
        dir: tempfile::TempDir,
        ready: &Established,
        leadership: Leadership,
    ) -> LiveVoter {
        start_voter_opts(id, listener, store, dir, ready, leadership, None)
    }

    /// `log_delay` holds every `/cluster/log` answer that long: a leader
    /// whose pulls are slow (a big log, a busy disk) but that is alive.
    fn start_voter_opts(
        id: &str,
        listener: tokio::net::TcpListener,
        store: Store,
        dir: tempfile::TempDir,
        ready: &Established,
        leadership: Leadership,
        log_delay: Option<Duration>,
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
            leadership,
        ));
        let serve_stop = CancellationToken::new();
        let app = router(handle.clone())
            .route("/mutate", axum::routing::post(|| async { "ok" }))
            .route(
                "/write/{key}",
                axum::routing::post({
                    let store = handle.store.clone();
                    move |axum::extract::Path(key): axum::extract::Path<String>| async move {
                        match store.put_meta(&key, &"yes".to_string()) {
                            Ok(()) => StatusCode::OK,
                            Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
                        }
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(
                handle.clone(),
                reject_follower_writes,
            ))
            .layer(middleware::from_fn(
                move |req: axum::extract::Request, next: Next| async move {
                    if let Some(delay) = log_delay {
                        if req.uri().path() == "/cluster/log" {
                            tokio::time::sleep(delay).await;
                        }
                    }
                    next.run(req).await
                },
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

    /// Wraps the store backend and notes every moment the leadership signal
    /// is up while the store is not logging writes. A write in such a moment
    /// would land unlogged and never reach the followers.
    struct LogProbe {
        inner: pistack_store::RedbBackend,
        leadership: Leadership,
        unlogged: Mutex<Vec<&'static str>>,
    }

    impl LogProbe {
        fn new(leadership: &Leadership) -> Arc<Self> {
            Arc::new(Self {
                inner: pistack_store::RedbBackend::in_memory().unwrap(),
                leadership: leadership.clone(),
                unlogged: Mutex::new(Vec::new()),
            })
        }

        fn check(&self, at: &'static str) {
            if self.leadership.is_leader() && !self.inner.record_log() {
                guard(&self.unlogged).push(at);
            }
        }

        fn store(self: &Arc<Self>) -> Store {
            let (events, _) = tokio::sync::broadcast::channel(16);
            Store::new(self.clone(), events)
        }

        fn gaps(&self) -> Vec<&'static str> {
            locked(&self.unlogged)
        }
    }

    impl pistack_store::Backend for LogProbe {
        fn get(&self, table: &str, key: &str) -> pistack_store::Result<Option<Vec<u8>>> {
            self.check("get");
            self.inner.get(table, key)
        }
        fn put(&self, table: &str, key: &str, value: &[u8]) -> pistack_store::Result<()> {
            self.check("put");
            self.inner.put(table, key, value)
        }
        fn delete(&self, table: &str, key: &str) -> pistack_store::Result<bool> {
            self.check("delete");
            self.inner.delete(table, key)
        }
        fn scan(&self, table: &str, prefix: &str) -> pistack_store::Result<Vec<(String, Vec<u8>)>> {
            self.check("scan");
            self.inner.scan(table, prefix)
        }
        fn batch(&self, ops: Vec<pistack_store::Op>) -> pistack_store::Result<()> {
            self.check("batch");
            self.inner.batch(ops)
        }
        fn set_record_log(&self, on: bool) {
            self.inner.set_record_log(on);
            self.check("set_record_log");
        }
        fn set_log_term(&self, term: u64) {
            self.check("set_log_term");
            self.inner.set_log_term(term);
        }
        fn set_write_fence(&self, on: bool) {
            self.inner.set_write_fence(on);
            self.check("set_write_fence");
        }
        fn set_applying(&self, on: bool) {
            self.inner.set_applying(on);
            self.check("set_applying");
        }
        fn read_log(&self, after: u64, limit: usize) -> pistack_store::Result<Vec<LogEntry>> {
            self.check("read_log");
            self.inner.read_log(after, limit)
        }
        fn record_log(&self) -> bool {
            self.inner.record_log()
        }
    }

    fn seed_voter(store: &Store, id: &str, members: &[Member], term: u64) {
        store.put_meta("cluster_id", &"c1".to_string()).unwrap();
        store.put_meta("node_id", &id.to_string()).unwrap();
        store.put_meta("members", &members.to_vec()).unwrap();
        store.put_meta("cluster_term", &term).unwrap();
    }

    /// The log entry that wrote meta `key`, if any.
    fn entry_for(store: &Store, key: &str) -> Option<LogEntry> {
        store
            .read_log(0, 1024)
            .unwrap()
            .into_iter()
            .find(|entry| entry.ops.iter().any(|op| op.key == key))
    }

    /// A node promoted to leader logs the first write made the moment the
    /// leadership signal rises, the way a controller seeds on promotion. A
    /// follower then copies that write and keeps replicating: no split.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_new_leaders_first_write_is_logged_and_replicated() {
        let term = 6u64;
        let a_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a_api = format!("http://{}", a_listener.local_addr().unwrap());
        let members = vec![
            Member::voter("sim", &a_api),
            Member::voter("sim2", "http://127.0.0.1:9"),
            Member::voter("sim3", "http://127.0.0.1:9"),
        ];

        // sim2 led term 6 and logged one write; sim copied it.
        let old_dir = tempfile::tempdir().unwrap();
        let old = open_store(old_dir.path());
        seed_voter(&old, "sim2", &members, term);
        old.set_log_term(term);
        old.set_record_log(true);
        old.put_meta("before", &"one".to_string()).unwrap();
        old.set_record_log(false);

        let leadership = Leadership::new(false);
        let probe = LogProbe::new(&leadership);
        let store = probe.store();
        seed_voter(&store, "sim", &members, term);
        store.apply_entries(&old.read_log(0, 64).unwrap()).unwrap();
        assert_eq!(store.commit_index().unwrap(), 1);
        let ready = Established {
            role: Role::Voter,
            leader: false,
            leader_api: String::new(),
            term,
            members: members.clone(),
        };
        let sim = start_voter_with(
            "sim",
            a_listener,
            store.clone(),
            tempfile::tempdir().unwrap(),
            &ready,
            leadership.clone(),
        );

        // Writes the instant the signal rises, like a controller's seed.
        let writer = {
            let store = store.clone();
            let leadership = leadership.clone();
            std::thread::spawn(move || {
                while !leadership.is_leader() {
                    std::hint::spin_loop();
                }
                store.put_meta("first", &"yes".to_string()).unwrap();
            })
        };
        become_leader(&sim.handle, term + 1).unwrap();
        writer.join().unwrap();

        assert!(
            probe.gaps().is_empty(),
            "the leadership signal was up while writes were not logged: {:?}",
            probe.gaps()
        );
        let first = entry_for(&store, "first").expect("the first write was not logged");
        assert_eq!(first.term, term + 1);

        // The follower copies it, and a second pull finds nothing forked.
        pull_log(&sim.api, &old).await.unwrap();
        assert_eq!(marker(&old, "first").as_deref(), Some("yes"));
        assert_eq!(old.commit_index().unwrap(), store.commit_index().unwrap());
        let tip = old.commit_index().unwrap();
        assert_eq!(
            old.log_term_at(tip).unwrap(),
            store.log_term_at(tip).unwrap()
        );
        store.put_meta("second", &"yes".to_string()).unwrap();
        pull_log(&sim.api, &old).await.unwrap();
        assert_eq!(marker(&old, "second").as_deref(), Some("yes"));
        assert!(probe.gaps().is_empty(), "{:?}", probe.gaps());
        sim.kill();
    }

    /// The reverse: a leader that steps down drops the signal before it
    /// stops logging, then logs nothing of its own. The follow loop's
    /// bookkeeping after stepping down stays out of the log, so the node
    /// copies the new leader's writes without a split.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_leader_that_steps_down_logs_nothing_and_follows_without_a_split() {
        let term = 6u64;
        let a_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let b_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a_api = format!("http://{}", a_listener.local_addr().unwrap());
        let b_api = format!("http://{}", b_listener.local_addr().unwrap());
        let members = vec![
            Member::voter("sim", &a_api),
            Member::voter("sim2", &b_api),
            Member::voter("sim3", "http://127.0.0.1:9"),
        ];

        // Set up the way establish_local does: logging on, then leading.
        let leadership = Leadership::new(false);
        let probe = LogProbe::new(&leadership);
        let store = probe.store();
        seed_voter(&store, "sim", &members, term);
        store.set_log_term(term);
        store.set_record_log(true);
        let leading = Established {
            role: Role::Voter,
            leader: true,
            leader_api: a_api.clone(),
            term,
            members: members.clone(),
        };
        let sim = start_voter_with(
            "sim",
            a_listener,
            store.clone(),
            tempfile::tempdir().unwrap(),
            &leading,
            leadership.clone(),
        );
        store.put_meta("before", &"one".to_string()).unwrap();
        assert!(entry_for(&store, "before").is_some());

        let b_dir = tempfile::tempdir().unwrap();
        let b_store = open_store(b_dir.path());
        seed_voter(&b_store, "sim2", &members, term);
        pull_log(&sim.api, &b_store).await.unwrap();
        assert_eq!(marker(&b_store, "before").as_deref(), Some("one"));
        let following = Established {
            role: Role::Voter,
            leader: false,
            leader_api: a_api.clone(),
            term,
            members: members.clone(),
        };
        let sim2 = start_voter("sim2", b_listener, b_store.clone(), b_dir, &following);

        stand_down(&sim.handle, "a voter reports a newer term");
        assert!(!sim.leads());
        assert!(!leadership.is_leader());
        assert!(!probe.inner.record_log(), "a follower must not log");
        assert!(
            probe.gaps().is_empty(),
            "logging stopped while the leadership signal was still up: {:?}",
            probe.gaps()
        );

        // What the follow loop stores once it hears the new leader.
        let commit = store.commit_index().unwrap();
        store
            .put_meta("cluster_leader_id", &"sim2".to_string())
            .unwrap();
        store.put_meta("members", &members).unwrap();
        assert_eq!(store.commit_index().unwrap(), commit);

        // sim2 leads term 7 and writes; sim copies it without a split.
        become_leader(&sim2.handle, term + 1).unwrap();
        b_store.put_meta("after", &"two".to_string()).unwrap();
        pull_log(&sim2.api, &store).await.unwrap();
        assert_eq!(marker(&store, "after").as_deref(), Some("two"));
        assert_eq!(
            store.commit_index().unwrap(),
            b_store.commit_index().unwrap()
        );
        let tip = store.commit_index().unwrap();
        assert_eq!(store.log_term_at(tip).unwrap(), Some(term + 1));
        assert!(probe.gaps().is_empty(), "{:?}", probe.gaps());
        sim.kill();
        sim2.kill();
    }

    /// After stand-down the write fence refuses an ordinary put (the in-flight
    /// race is covered in the store crate). Follower apply still succeeds, so
    /// the node can copy the new leader without a split.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_write_after_stand_down_is_refused_and_apply_still_works() {
        let term = 6u64;
        let a_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a_api = format!("http://{}", a_listener.local_addr().unwrap());
        let members = vec![
            Member::voter("sim", &a_api),
            Member::voter("sim2", "http://127.0.0.1:9"),
            Member::voter("sim3", "http://127.0.0.1:9"),
        ];

        let leadership = Leadership::new(false);
        let probe = LogProbe::new(&leadership);
        let store = probe.store();
        seed_voter(&store, "sim", &members, term);
        store.set_log_term(term);
        store.set_record_log(true);
        let leading = Established {
            role: Role::Voter,
            leader: true,
            leader_api: a_api.clone(),
            term,
            members: members.clone(),
        };
        let sim = start_voter_with(
            "sim",
            a_listener,
            store.clone(),
            tempfile::tempdir().unwrap(),
            &leading,
            leadership.clone(),
        );
        store.put_meta("before", &"one".to_string()).unwrap();
        let commit = store.commit_index().unwrap();

        stand_down(&sim.handle, "a voter reports a newer term");
        let err = store
            .put_meta("inflight", &"split".to_string())
            .unwrap_err();
        assert!(matches!(err, pistack_store::StoreError::NotLeader), "{err}");
        assert!(store.get_meta::<String>("inflight").unwrap().is_none());
        assert_eq!(store.commit_index().unwrap(), commit);
        assert!(!probe.inner.record_log());

        let entry = LogEntry {
            index: commit + 1,
            term: term + 1,
            ops: vec![pistack_store::LogOp {
                table: "meta".into(),
                key: "after".into(),
                value: Some(serde_json::to_vec("two").unwrap()),
            }],
        };
        store.apply_entries(std::slice::from_ref(&entry)).unwrap();
        assert_eq!(marker(&store, "after").as_deref(), Some("two"));
        assert_eq!(store.commit_index().unwrap(), commit + 1);
        sim.kill();
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
        let members = vec![
            Member::voter("sim", "http://sim"),
            Member::voter("sim2", "http://sim2"),
            Member::voter("sim3", "http://sim3"),
        ];
        let seeded = || {
            let dir = tempfile::tempdir().unwrap();
            let store = open_store(dir.path());
            store.put_meta("members", &members).unwrap();
            store
                .put_meta("cluster_leader_id", &"sim".to_string())
                .unwrap();
            store.put_meta("cluster_term", &6u64).unwrap();
            (dir, store)
        };
        let (_dir, store) = seeded();
        let LocalRole::Ready(old) = establish_local(&store, "sim", "http://sim", true).unwrap()
        else {
            panic!("member had to join");
        };
        assert!(!old.leader);
        assert_eq!(old.leader_api, "");
        assert_eq!(old.term, 6);
        let (_dir2, store) = seeded();
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

    async fn status_of_api(api: &str) -> ClusterStatus {
        http_client()
            .get(format!("{api}/cluster"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn ask_vote(api: &str, candidate: &str, term: u64, pre: bool) -> VoteResponse {
        http_client()
            .post(format!("{api}/cluster/vote"))
            .json(&VoteRequest {
                term,
                candidate_id: candidate.into(),
                last_index: 1_000_000,
                last_term: 1_000,
                pre,
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    /// A learner whose leader went silent waits: election timeouts pass, it
    /// never campaigns, never moves its term, and refuses every vote. The
    /// same holds when its in-memory role says voter but its own store does
    /// not, and for a server whose store has no membership at all.
    #[tokio::test]
    async fn a_learner_never_campaigns_or_votes_even_when_the_leader_is_silent() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let members = vec![
            Member::voter("sim", "http://127.0.0.1:9"),
            Member::voter("sim2", "http://127.0.0.1:9"),
            Member::learner("joiner", &api),
        ];
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        seed_voter(&store, "joiner", &members, 6);
        let ready = Established {
            role: Role::Learner,
            leader: false,
            leader_api: "http://127.0.0.1:9".into(),
            term: 6,
            members: members.clone(),
        };
        let joiner = start_voter("joiner", listener, store, dir, &ready);
        *guard(&joiner.handle.election_timeout) = Duration::from_millis(50);
        run(&joiner);
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            assert!(!joiner.leads(), "a learner took leadership");
        }
        assert_eq!(joiner.term(), 6, "a learner moved the term");
        for pre in [true, false] {
            let reply = ask_vote(&joiner.api, "sim2", 9, pre).await;
            assert!(!reply.granted, "a learner granted a vote");
            assert_eq!(reply.term, 6);
        }

        // In-memory role out of step with the store: the store wins.
        *guard(&joiner.handle.role) = Role::Voter;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            assert!(!joiner.leads());
        }
        assert_eq!(joiner.term(), 6);
        assert!(!ask_vote(&joiner.api, "sim2", 9, true).await.granted);
        joiner.kill();

        // No membership in the store: never a lone voter.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        store.put_meta("cluster_term", &6u64).unwrap();
        let ready = Established {
            role: Role::Voter,
            leader: false,
            leader_api: String::new(),
            term: 6,
            members: Vec::new(),
        };
        let bare = start_voter("bare", listener, store, dir, &ready);
        *guard(&bare.handle.election_timeout) = Duration::from_millis(50);
        run(&bare);
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            assert!(!bare.leads(), "a server with no membership elected itself");
        }
        assert_eq!(bare.term(), 6);
        bare.kill();
    }

    /// The lab failure. The leader's own store is copied, offline, onto a
    /// voter that keeps its node id; the leader comes back and writes on.
    /// The copy carries the leader's id, term, vote and a membership naming
    /// the copy's server a voter. It must rejoin as a learner, never lead or
    /// move the term, catch up, and only then become a voter: one leader the
    /// whole time and no fork.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_copied_leader_store_rejoins_as_a_learner_and_is_promoted_without_a_fork() {
        let term = 6u64;
        let (sim, sim2, sim3) = three_voters(term).await;
        for n in 0..20 {
            sim.handle.store.put_meta(&format!("k{n}"), &n).unwrap();
        }
        wait_for("followers copy", Duration::from_secs(5), || {
            sim3.handle.store.get_meta::<i32>("k19").unwrap() == Some(19)
                && sim2.handle.store.get_meta::<i32>("k19").unwrap() == Some(19)
        })
        .await;

        // Stop sim3 and copy the leader's store over it.
        sim3.kill();
        let LiveVoter {
            addr: sim3_addr,
            dir: sim3_dir,
            api: sim3_api,
            ..
        } = sim3;
        let copy_path = sim3_dir.path().join("state.redb.reseed");
        sim.handle
            .store
            .snapshot_file(&sim.handle.db_path, &copy_path)
            .unwrap();
        // The leader keeps writing past the copy, as it did in the lab.
        for n in 20..200 {
            sim.handle.store.put_meta(&format!("k{n}"), &n).unwrap();
        }

        let copy = {
            let (events, _) = tokio::sync::broadcast::channel(16);
            Store::open(&copy_path, events).unwrap()
        };
        let owner: Option<String> = copy.get_meta("node_id").unwrap();
        assert_eq!(
            owner.as_deref(),
            Some("sim"),
            "the copy carries the leader's id"
        );
        let voted: Option<String> = copy.get_meta("cluster_voted_for").unwrap_or(None);
        assert_eq!(voted.as_deref(), Some("sim"));
        assert!(is_voter(&members_of(&copy).unwrap(), "sim3"));

        // Boot as the server does with an existing store (Keep).
        let established = match establish_local(&copy, "sim3", &sim3_api, true).unwrap() {
            LocalRole::NeedJoin => adopt(&copy, &sim.api, "sim3", &sim3_api).await.unwrap(),
            LocalRole::Ready(_) => panic!("a copied store was trusted as this server's own"),
        };
        assert_eq!(established.role, Role::Learner);
        assert!(!established.leader);
        let voted: Option<String> = copy.get_meta("cluster_voted_for").unwrap_or(None);
        assert!(voted.is_none(), "the copied vote was kept");
        let owner: Option<String> = copy.get_meta("node_id").unwrap();
        assert_eq!(owner.as_deref(), Some("sim3"));
        let leader_view = members_of(&sim.handle.store).unwrap();
        assert!(
            !is_voter(&leader_view, "sim3"),
            "the rejoiner stayed a voter"
        );

        let listener = loop {
            match tokio::net::TcpListener::bind(sim3_addr).await {
                Ok(listener) => break listener,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        let back = start_voter("sim3", listener, copy.clone(), sim3_dir, &established);
        *guard(&back.handle.election_timeout) = Duration::from_millis(50);
        run(&back);

        let mut promoted = false;
        for _ in 0..200 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            let leaders = [&sim, &sim2, &back].iter().filter(|v| v.leads()).count();
            assert_eq!(leaders, 1, "exactly one node may claim leadership");
            assert!(sim.leads(), "the healthy leader lost leadership");
            assert!(!back.leads(), "the reseeded server led");
            assert_eq!(sim.term(), term, "the term moved");
            assert_eq!(back.term(), term, "the reseeded server moved its term");
            if is_voter(&members_of(&sim.handle.store).unwrap(), "sim3")
                && *guard(&back.handle.role) == Role::Voter
            {
                promoted = true;
                break;
            }
        }
        assert!(promoted, "the caught-up learner was not promoted");
        // Promoted within one page of the leader; the rest follows.
        wait_for(
            "the learner copies the writes made after the copy",
            Duration::from_secs(3),
            || copy.get_meta::<i32>("k199").unwrap() == Some(199),
        )
        .await;
        sim.handle.store.put_meta("after", &"yes").unwrap();
        wait_for(
            "the promoted voter copies on",
            Duration::from_secs(3),
            || marker(&copy, "after").as_deref() == Some("yes"),
        )
        .await;
        let tip = copy.commit_index().unwrap();
        assert_eq!(tip, sim.handle.store.commit_index().unwrap());
        assert_eq!(
            copy.log_term_at(tip).unwrap(),
            sim.handle.store.log_term_at(tip).unwrap()
        );
        assert!(guard(&back.handle.pull_error).is_none());
        assert_eq!(status_of_api(&sim.api).await.term, term);
        sim.kill();
        sim2.kill();
        back.kill();
    }

    /// The other half of the lab failure. Pulls from a live leader are slow
    /// (a large log): a follower that just heard the leader must not count
    /// the time its pull takes as silence, start an election, or let its
    /// lease lapse and hand a vote to a candidate.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_slow_pull_from_a_live_leader_neither_starts_an_election_nor_opens_the_lease() {
        let term = 6u64;
        let first = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let third = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sim_api = format!("http://{}", first.local_addr().unwrap());
        let members = vec![
            Member::voter("sim", &sim_api),
            Member::voter("sim2", &format!("http://{}", second.local_addr().unwrap())),
            Member::voter("sim3", &format!("http://{}", third.local_addr().unwrap())),
        ];
        let mut voters = Vec::new();
        for (id, listener) in [("sim", first), ("sim2", second), ("sim3", third)] {
            let leader = id == "sim";
            let dir = tempfile::tempdir().unwrap();
            let store = open_store(dir.path());
            seed_voter(&store, id, &members, term);
            store.set_log_term(term);
            if leader {
                store.set_record_log(true);
            }
            let ready = Established {
                role: Role::Voter,
                leader,
                leader_api: sim_api.clone(),
                term,
                members: members.clone(),
            };
            let delay = leader.then_some(Duration::from_millis(1_500));
            voters.push(start_voter_opts(
                id,
                listener,
                store,
                dir,
                &ready,
                Leadership::new(leader),
                delay,
            ));
        }
        let (sim, sim2, sim3) = (&voters[0], &voters[1], &voters[2]);
        *guard(&sim2.handle.election_timeout) = Duration::from_millis(300);
        *guard(&sim3.handle.election_timeout) = Duration::from_millis(300);
        for voter in &voters {
            run(voter);
        }
        sim.handle.store.put_meta("slow", &"yes").unwrap();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(4) {
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(sim.leads(), "a slow pull deposed a live leader");
            assert!(!sim2.leads() && !sim3.leads());
            assert_eq!(sim2.term(), term, "a follower started an election");
            assert_eq!(sim3.term(), term, "a follower started an election");
            if started.elapsed() > Duration::from_millis(500) {
                let reply = ask_vote(&sim2.api, "sim3", term + 1, true).await;
                assert!(
                    !reply.granted,
                    "a follower with a live leader granted a pre-vote"
                );
            }
        }
        assert_eq!(marker(&sim2.handle.store, "slow").as_deref(), Some("yes"));
        assert_eq!(marker(&sim3.handle.store, "slow").as_deref(), Some("yes"));
        for voter in &voters {
            voter.kill();
        }
    }

    /// A store of several megabytes, written as a real redb file.
    fn big_store(dir: &Path, rows: usize, size: usize) -> (PathBuf, Store) {
        let path = dir.join("state.redb");
        let store = open_store(dir);
        store.put_meta("cluster_id", &"c1".to_string()).unwrap();
        store.set_log_term(3);
        store.set_record_log(true);
        let blob = "x".repeat(size);
        for n in 0..rows {
            store.put_meta(&format!("blob/{n:06}"), &blob).unwrap();
        }
        store.set_record_log(false);
        (path, store)
    }

    /// Serve `bytes` as `/cluster/snapshot` in `piece`-byte pieces with a
    /// pause between them. `break_first` cuts the first answer off half way;
    /// `hang` stops sending half way and never finishes.
    async fn slow_snapshot_server(
        bytes: Arc<Vec<u8>>,
        piece: usize,
        pause: Duration,
        break_first: bool,
        hang: bool,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let app = Router::new().route(
            "/cluster/snapshot",
            axum::routing::get(move || {
                let bytes = bytes.clone();
                let call = seen.fetch_add(1, Ordering::SeqCst);
                async move {
                    let total = bytes.len();
                    let cut = (break_first && call == 0) || hang;
                    let stream = futures::stream::unfold(0usize, move |at| {
                        let bytes = bytes.clone();
                        async move {
                            if at >= total {
                                return None;
                            }
                            if cut && at >= total / 2 {
                                if hang {
                                    std::future::pending::<()>().await;
                                }
                                return Some((Err(std::io::Error::other("link dropped")), total));
                            }
                            tokio::time::sleep(pause).await;
                            let end = (at + piece).min(total);
                            Some((
                                Ok::<_, std::io::Error>(axum::body::Bytes::copy_from_slice(
                                    &bytes[at..end],
                                )),
                                end,
                            ))
                        }
                    });
                    Response::builder()
                        .header(header::CONTENT_LENGTH, total)
                        .body(Body::from_stream(stream))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        (format!("http://{addr}"), calls)
    }

    /// Many megabytes in small pieces over a slow link: the download holds
    /// one piece at a time, takes far longer than the stall limit, and still
    /// finishes because only a stall fails it. The result opens and is the
    /// leader's data.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_large_snapshot_streams_in_small_pieces_and_a_slow_link_finishes() {
        let src_dir = tempfile::tempdir().unwrap();
        let (path, store) = big_store(src_dir.path(), 600, 8 * 1024);
        drop(store);
        let bytes = Arc::new(std::fs::read(&path).unwrap());
        assert!(bytes.len() > 4 << 20, "{}", bytes.len());
        let piece = 32 * 1024;
        let (api, _) =
            slow_snapshot_server(bytes.clone(), piece, Duration::from_millis(8), false, false)
                .await;
        let dest_dir = tempfile::tempdir().unwrap();
        let dest = dest_dir.path().join("state.redb");
        let limits = SnapshotFetch {
            stall: Duration::from_millis(400),
            restarts: 0,
            startup: Duration::from_secs(2),
            retry_wait: Duration::from_millis(50),
        };
        let started = Instant::now();
        let done = fetch_snapshot_with(&api, &dest, &limits).await.unwrap();
        let took = started.elapsed();
        assert_eq!(done.bytes, bytes.len() as u64);
        assert!(done.largest_chunk <= 4 * piece, "{}", done.largest_chunk);
        assert!(took > limits.stall * 3, "the link was not slow: {took:?}");
        assert!(!dest.with_extension("partial").exists());
        let copy = open_store(dest_dir.path());
        assert_eq!(
            copy.get_meta::<String>("blob/000599")
                .unwrap()
                .unwrap()
                .len(),
            8 * 1024
        );
    }

    /// A transfer cut off half way starts again cleanly and installs once.
    /// A transfer that stops moving fails on the stall limit and leaves
    /// neither the store nor a partial file behind.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_broken_snapshot_restarts_cleanly_and_a_stalled_one_fails() {
        let src_dir = tempfile::tempdir().unwrap();
        let (path, store) = big_store(src_dir.path(), 100, 8 * 1024);
        drop(store);
        let bytes = Arc::new(std::fs::read(&path).unwrap());
        let limits = SnapshotFetch {
            stall: Duration::from_millis(300),
            restarts: 2,
            startup: Duration::from_secs(2),
            retry_wait: Duration::from_millis(20),
        };

        let (api, calls) =
            slow_snapshot_server(bytes.clone(), 16 * 1024, Duration::ZERO, true, false).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("state.redb");
        let done = fetch_snapshot_with(&api, &dest, &limits).await.unwrap();
        assert_eq!(done.bytes, bytes.len() as u64);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one restart");
        assert!(dest.exists());
        assert!(!dest.with_extension("partial").exists());

        let (api, _) =
            slow_snapshot_server(bytes.clone(), 16 * 1024, Duration::ZERO, false, true).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("state.redb");
        let limits = SnapshotFetch {
            restarts: 0,
            ..limits
        };
        let err = fetch_snapshot_with(&api, &dest, &limits).await.unwrap_err();
        assert!(err.to_string().contains("stalled"), "{err:#}");
        assert!(!dest.exists());
        assert!(!dest.with_extension("partial").exists());
    }

    /// The real `/cluster/snapshot` route streams a consistent copy of a
    /// leader's multi-megabyte store; the joiner gets every row, the same
    /// commit index, and the leader leaves no temporary file behind.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_snapshot_route_streams_a_consistent_copy() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (_path, store) = big_store(dir.path(), 700, 8 * 1024);
        let members = vec![Member::voter("sim", "http://sim")];
        seed_voter(&store, "sim", &members, 3);
        let ready = Established {
            role: Role::Voter,
            leader: true,
            leader_api: String::new(),
            term: 3,
            members,
        };
        let leader = start_voter("sim", listener, store.clone(), dir, &ready);
        let dest_dir = tempfile::tempdir().unwrap();
        let dest = dest_dir.path().join("state.redb");
        let done = fetch_snapshot_with(&leader.api, &dest, &SnapshotFetch::default())
            .await
            .unwrap();
        assert!(done.bytes > 4 << 20, "{}", done.bytes);
        assert!(
            done.largest_chunk <= 4 * SNAPSHOT_CHUNK,
            "{}",
            done.largest_chunk
        );
        let copy = open_store(dest_dir.path());
        assert_eq!(copy.commit_index().unwrap(), store.commit_index().unwrap());
        assert!(copy.get_meta::<String>("blob/000699").unwrap().is_some());
        let leftovers: Vec<_> = std::fs::read_dir(leader.dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("snapshot-"))
            .collect();
        assert!(leftovers.is_empty(), "temporary snapshot files left behind");
        leader.kill();
    }

    /// Compaction trims what every follower already has, keeps what a
    /// slightly-behind follower still needs, and replication goes on. A
    /// server that needs trimmed rows is told to rejoin, not fed a gap.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn compaction_trims_the_log_and_followers_keep_working() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let members = vec![
            Member::voter("sim", "http://sim"),
            Member::voter("a", "http://a"),
            Member::voter("b", "http://b"),
        ];
        let leader = serve_voter("sim", listener, &members, true, 6, "").await;
        let store = leader.handle.store.clone();
        for n in 0..300 {
            store.put_meta(&format!("k{n}"), &n).unwrap();
        }
        let commit = store.commit_index().unwrap();
        let (_a_dir, a) = {
            let dir = tempfile::tempdir().unwrap();
            let s = open_store(dir.path());
            (dir, s)
        };
        let (_b_dir, b) = {
            let dir = tempfile::tempdir().unwrap();
            let s = open_store(dir.path());
            (dir, s)
        };
        while pull_page(&leader.api, &a, "a", 0, Duration::ZERO)
            .await
            .unwrap()
            > 0
        {}
        assert_eq!(a.commit_index().unwrap(), commit);
        // Two pages: the leader saw b ask after the first one.
        pull_page(&leader.api, &b, "b", 0, Duration::ZERO)
            .await
            .unwrap();
        pull_page(&leader.api, &b, "b", 0, Duration::ZERO)
            .await
            .unwrap();
        let b_at = b.commit_index().unwrap();
        assert!(b_at < commit);

        leader.handle.log_keep.store(50, Ordering::Relaxed);
        let removed = compact_now(&leader.handle).unwrap();
        assert!(removed > 0);
        let start = store.log_start().unwrap();
        assert!(start <= b_at, "trimmed rows the slow follower still needs");
        assert!(store.log_term_at(commit).unwrap().is_some());
        // The slightly-behind follower carries on.
        while pull_page(&leader.api, &b, "b", 0, Duration::ZERO)
            .await
            .unwrap()
            > 0
        {}
        assert_eq!(b.commit_index().unwrap(), commit);

        // Everyone is current: trim down to the kept tail.
        compact_now(&leader.handle).unwrap();
        assert_eq!(store.log_start().unwrap(), commit - 50);
        assert_eq!(store.read_log(0, 1).unwrap()[0].index, commit - 50);
        for n in 300..320 {
            store.put_meta(&format!("k{n}"), &n).unwrap();
        }
        while pull_page(&leader.api, &a, "a", 0, Duration::ZERO)
            .await
            .unwrap()
            > 0
        {}
        while pull_page(&leader.api, &b, "b", 0, Duration::ZERO)
            .await
            .unwrap()
            > 0
        {}
        assert_eq!(a.get_meta::<i32>("k319").unwrap(), Some(319));
        assert_eq!(b.get_meta::<i32>("k319").unwrap(), Some(319));

        // A server that needs trimmed rows is sent back to --join.
        let fresh = Store::in_memory().unwrap();
        let err = pull_page(&leader.api, &fresh, "c", 0, Duration::ZERO)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("compacted"), "{err:#}");
        assert_eq!(fresh.commit_index().unwrap(), 0);
        leader.kill();
    }

    async fn post_write(api: &str, key: &str) -> u16 {
        reqwest::Client::new()
            .post(format!("{api}/write/{key}"))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    #[tokio::test]
    async fn a_write_is_answered_only_once_a_voter_majority_holds_it() {
        let (sim, sim2, sim3) = three_voters(4).await;
        for n in 0..5 {
            let key = format!("confirmed-{n}");
            assert_eq!(post_write(&sim.api, &key).await, 200);
            // No waiting here: the answer itself means a follower has it.
            let holders = [&sim2, &sim3]
                .iter()
                .filter(|voter| marker(&voter.handle.store, &key).is_some())
                .count();
            assert!(
                holders >= 1,
                "{key} was answered before any follower held it"
            );
        }
        sim.kill();
        sim2.kill();
        sim3.kill();
    }

    #[tokio::test]
    async fn a_write_no_majority_holds_is_not_confirmed() {
        let (sim, sim2, sim3) = three_voters(4).await;
        assert_eq!(post_write(&sim.api, "while-healthy").await, 200);
        // Both followers go dark. The leader still leads until it notices.
        sim2.kill();
        sim3.kill();
        *guard(&sim.handle.confirm_wait) = Duration::from_millis(400);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(post_write(&sim.api, "alone").await, 503);
        assert!(marker(&sim2.handle.store, "alone").is_none());
        assert!(marker(&sim3.handle.store, "alone").is_none());
        sim.kill();
    }

    #[tokio::test]
    async fn one_follower_is_enough_for_a_majority_of_three() {
        let (sim, sim2, sim3) = three_voters(4).await;
        assert_eq!(post_write(&sim.api, "warm-up").await, 200);
        sim3.kill();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(post_write(&sim.api, "two-of-three").await, 200);
        assert!(marker(&sim2.handle.store, "two-of-three").is_some());
        sim.kill();
        sim2.kill();
    }

    #[tokio::test]
    async fn a_confirmed_write_survives_losing_the_leader() {
        let (sim, sim2, sim3) = three_voters(4).await;
        for n in 0..3 {
            assert_eq!(post_write(&sim.api, &format!("kept-{n}")).await, 200);
        }
        sim.kill();
        wait_for("a follower takes over", Duration::from_secs(15), || {
            sim2.leads() || sim3.leads()
        })
        .await;
        let winner = if sim2.leads() { &sim2 } else { &sim3 };
        for n in 0..3 {
            assert!(
                marker(&winner.handle.store, &format!("kept-{n}")).is_some(),
                "the new leader is missing confirmed write kept-{n}"
            );
        }
        sim2.kill();
        sim3.kill();
    }

    #[tokio::test]
    async fn a_pull_from_a_newer_term_is_not_an_acknowledgement() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let members = vec![
            Member::voter("sim", &api),
            Member::voter("sim2", "http://127.0.0.1:9"),
            Member::voter("sim3", "http://127.0.0.1:9"),
        ];
        // No follow loop: only the log route's own check is under test.
        let sim = serve_voter("sim", listener, &members, true, 4, &api).await;
        sim.handle
            .store
            .put_meta("pending", &"yes".to_string())
            .unwrap();
        let index = sim.handle.store.commit_index().unwrap();
        let response = rpc_client()
            .get(format!(
                "{api}/cluster/log?after={index}&term=4&node=sim2&fterm=5"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 409);
        assert!(!sim.leads(), "a leader told of a newer term keeps leading");
        assert_eq!(sim.term(), 5);
        assert!(
            guard(&sim.handle.progress).get("sim2").is_none(),
            "a pull from a newer term was counted as an acknowledgement"
        );
        sim.kill();
    }
}
