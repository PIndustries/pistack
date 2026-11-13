// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Server-side command queue: one FIFO per node, plus tracking of every
//! command from enqueue until the agent reports a result or it times out.
//!
//! Modules (`forge`, `fabric`, `slab`) call [`CommandQueue::enqueue`] and
//! await the returned [`Ticket`]. The HTTP layer in [`crate::server`] calls
//! [`CommandQueue::next`] for the agent's long-poll and
//! [`CommandQueue::ack`] when results arrive.
//!
//! The queue itself is memory on the leader. What survives a restart or
//! a new leader is the id of a keyed command ([`CommandQueue::enqueue_keyed`]):
//! one small record per node and key in the replicated store, written on
//! enqueue and removed on result, timeout, or when the node's commands are
//! dropped. The command is not stored. The module that issued it issues it
//! again from the state of its resource, the record gives it the id it had,
//! and the agent answers an id it has already completed from memory.
//!
//! Timeouts are measured from enqueue, so a command to an offline node
//! fails after `command_timeout` too, rather than waiting for ever. A
//! record lives no longer than its command, so a host that stays down
//! leaves at most one record per key, and each of those for one timeout.
//! Expiry is checked lazily on every queue operation and whenever a ticket
//! is awaited; there is no background task, which keeps the queue usable
//! from tests with paused time.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use pistack_store::Store;
use pistack_types::Resources;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::Instant;
use uuid::Uuid;

use crate::error::RpcError;
use crate::proto::{
    AgentEvent, AgentHello, Command, CommandId, CommandKind, CommandResult, Heartbeat,
    HeartbeatAck, HelloAck, MachineReport, OvnHostConfig, ReportAck, MAX_LONG_POLL_SECS,
};

/// Capacity of the [`AgentEvent`] broadcast; a subscriber this far behind
/// sees `Lagged` and should resync from [`CommandQueue::nodes`].
pub const AGENT_EVENT_CAPACITY: usize = 1024;

/// Meta keys under this prefix hold the ids of keyed commands, as
/// `<prefix><node>/<key>`.
pub const COMMAND_RECORD_PREFIX: &str = "rpc/commands/";

/// How often a queue operation also looks for records nothing in this
/// process owns: what an earlier leader left for commands nobody issued
/// again.
const SWEEP_EVERY: Duration = Duration::from_secs(30);

/// What the store keeps for one keyed command. The node and the key are in
/// the meta key; the payload is not kept.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    id: CommandId,
    kind: CommandKind,
    issued_at: DateTime<Utc>,
}

fn record_key(node: &str, key: &str) -> String {
    format!("{COMMAND_RECORD_PREFIX}{node}/{key}")
}

/// Tunables for [`CommandQueue`].
#[derive(Clone, Debug)]
pub struct QueueConfig {
    /// From enqueue to failure if no result arrives.
    pub command_timeout: Duration,
    /// Undelivered commands a node may have queued before `enqueue` fails.
    pub capacity: usize,
    /// Interval agents are told to heartbeat at.
    pub heartbeat_interval: Duration,
    /// OVN settings handed to every agent in the hello answer. `None`
    /// when the cluster's networks are not OVN.
    pub ovn: Option<OvnHostConfig>,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            command_timeout: Duration::from_secs(120),
            capacity: 1024,
            heartbeat_interval: Duration::from_secs(15),
            ovn: None,
        }
    }
}

/// What the control plane knows about one host. A snapshot; cheap to clone.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeInfo {
    pub name: String,
    pub capabilities: Vec<String>,
    pub labels: BTreeMap<String, String>,
    pub version: String,
    /// Underlay IPv4 from the last hello. None when the agent left it empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlay_ip: Option<String>,
    pub resources_total: Resources,
    pub resources_free: Resources,
    pub machines: Vec<MachineReport>,
    pub last_hello: Option<DateTime<Utc>>,
    pub last_heartbeat: Option<DateTime<Utc>>,
    /// Commands delivered to the agent and not yet answered.
    pub in_flight: usize,
    /// Commands queued and not yet fetched.
    pub queued: usize,
}

impl NodeInfo {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            capabilities: Vec::new(),
            labels: BTreeMap::new(),
            version: String::new(),
            underlay_ip: None,
            resources_total: Resources::default(),
            resources_free: Resources::default(),
            machines: Vec::new(),
            last_hello: None,
            last_heartbeat: None,
            in_flight: 0,
            queued: 0,
        }
    }
}

/// Lifecycle of one command, for `pending()` introspection.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingInfo {
    pub id: CommandId,
    pub node: String,
    pub kind: CommandKind,
    pub issued_at: DateTime<Utc>,
    pub delivered: bool,
}

type Outcome = Result<CommandResult, RpcError>;

struct Pending {
    node: String,
    kind: CommandKind,
    issued_at: DateTime<Utc>,
    enqueued: Instant,
    delivered: bool,
    waiter: Option<oneshot::Sender<Outcome>>,
    /// Meta key of the record, for a keyed command.
    record: Option<String>,
}

struct NodeEntry {
    tx: mpsc::Sender<Command>,
    rx: Arc<tokio::sync::Mutex<mpsc::Receiver<Command>>>,
    info: NodeInfo,
}

#[derive(Default)]
struct State {
    nodes: HashMap<String, NodeEntry>,
    pending: HashMap<CommandId, Pending>,
    /// Record key to the pending command that holds it.
    keyed: HashMap<String, CommandId>,
}

impl State {
    /// Take a command out of `pending`, and its key with it.
    fn forget(&mut self, id: CommandId) -> Option<Pending> {
        let p = self.pending.remove(&id)?;
        if let Some(record) = &p.record {
            if self.keyed.get(record) == Some(&id) {
                self.keyed.remove(record);
            }
        }
        Some(p)
    }
}

struct Inner {
    cfg: QueueConfig,
    state: Mutex<State>,
    agent_events: broadcast::Sender<AgentEvent>,
    /// Where keyed commands record their ids. Unset in tests and tools that
    /// run a bare queue; ids then last as long as the process.
    store: OnceLock<Store>,
    /// One keyed enqueue at a time, so the store is read and written
    /// outside the state lock without two callers minting an id each.
    keyed_lock: Mutex<()>,
    swept: Mutex<Option<Instant>>,
}

/// Per-node command queues with pending/ack tracking. Clone freely; all
/// clones share one queue.
#[derive(Clone)]
pub struct CommandQueue(Arc<Inner>);

impl Default for CommandQueue {
    fn default() -> Self {
        Self::new(QueueConfig::default())
    }
}

/// Handle to one enqueued command; await it for the agent's result.
#[must_use = "a Ticket that is never awaited still times out, but nobody learns the result"]
pub struct Ticket {
    id: CommandId,
    node: String,
    kind: CommandKind,
    queue: CommandQueue,
    rx: oneshot::Receiver<Outcome>,
}

impl std::fmt::Debug for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ticket")
            .field("id", &self.id)
            .field("node", &self.node)
            .field("kind", &self.kind)
            .finish()
    }
}

impl Ticket {
    pub fn id(&self) -> CommandId {
        self.id
    }

    /// Wait for the result, or [`RpcError::Timeout`] after the queue's
    /// `command_timeout` measured from enqueue.
    pub async fn wait(self) -> Outcome {
        let remaining = self.queue.remaining(self.id);
        let Ticket {
            id,
            node,
            kind,
            queue,
            rx,
        } = self;
        match tokio::time::timeout(remaining, rx).await {
            Ok(Ok(outcome)) => outcome,
            // Sender dropped without a result: the queue itself went away.
            Ok(Err(_)) => Err(RpcError::Closed),
            Err(_) => {
                queue.expire(id);
                Err(RpcError::Timeout { id, kind, node })
            }
        }
    }
}

impl CommandQueue {
    pub fn new(cfg: QueueConfig) -> Self {
        let (agent_events, _) = broadcast::channel(AGENT_EVENT_CAPACITY);
        Self(Arc::new(Inner {
            cfg,
            state: Mutex::new(State::default()),
            agent_events,
            store: OnceLock::new(),
            keyed_lock: Mutex::new(()),
            swept: Mutex::new(None),
        }))
    }

    /// Record the ids of keyed commands in `store`, so they outlive this
    /// process and follow the leader. The first store attached stays.
    pub fn attach_store(&self, store: Store) {
        let _ = self.0.store.set(store);
    }

    pub fn config(&self) -> &QueueConfig {
        &self.0.cfg
    }

    /// Every hello and heartbeat the queue records, as it records it.
    /// `forge` uses this to register hosts and refresh `last_seen`.
    pub fn subscribe_agent_events(&self) -> broadcast::Receiver<AgentEvent> {
        self.0.agent_events.subscribe()
    }

    fn emit(&self, ev: AgentEvent) {
        // No subscribers is normal (tests, a bare queue); not an error.
        let _ = self.0.agent_events.send(ev);
    }

    fn entry<'a>(&self, st: &'a mut State, node: &str) -> &'a mut NodeEntry {
        let capacity = self.0.cfg.capacity;
        st.nodes.entry(node.to_owned()).or_insert_with(|| {
            let (tx, rx) = mpsc::channel(capacity);
            NodeEntry {
                tx,
                rx: Arc::new(tokio::sync::Mutex::new(rx)),
                info: NodeInfo::new(node),
            }
        })
    }

    /// Record an agent's hello and return what to tell it.
    pub fn hello(&self, hello: &AgentHello) -> HelloAck {
        let now = Utc::now();
        {
            let mut st = self.0.state.lock();
            let e = self.entry(&mut st, &hello.node_name);
            e.info.capabilities = hello.capabilities.clone();
            e.info.labels = hello.labels.clone();
            e.info.version = hello.version.clone();
            e.info.underlay_ip = hello.underlay_ip.clone();
            e.info.resources_total = hello.resources;
            e.info.last_hello = Some(now);
        }
        tracing::info!(node = %hello.node_name, version = %hello.version, "agent hello");
        self.emit(AgentEvent::Hello(hello.clone()));
        HelloAck {
            node_name: hello.node_name.clone(),
            server_time: now,
            heartbeat_interval_secs: self.0.cfg.heartbeat_interval.as_secs().max(1),
            max_long_poll_secs: MAX_LONG_POLL_SECS,
            ovn: self.0.cfg.ovn.clone(),
        }
    }

    /// Record a heartbeat.
    pub fn heartbeat(&self, hb: &Heartbeat) -> HeartbeatAck {
        self.reap();
        let now = Utc::now();
        let mut st = self.0.state.lock();
        let e = self.entry(&mut st, &hb.node_name);
        e.info.resources_free = hb.resources_free;
        e.info.machines = hb.machines.clone();
        e.info.last_heartbeat = Some(now);
        let known = e.info.last_hello.is_some();
        let commands_waiting = self.0.cfg.capacity - e.tx.capacity();
        drop(st);
        self.emit(AgentEvent::Heartbeat(hb.clone()));
        HeartbeatAck {
            server_time: now,
            commands_waiting,
            known,
            servers: Vec::new(),
        }
    }

    /// Queue a command for `node`. Fails with [`RpcError::QueueFull`] when
    /// the node has `capacity` undelivered commands already.
    pub fn enqueue(
        &self,
        node: &str,
        kind: CommandKind,
        payload: serde_json::Value,
    ) -> Result<Ticket, RpcError> {
        self.reap();
        let id = Uuid::now_v7();
        let issued_at = Utc::now();
        let cmd = Command {
            id,
            kind,
            payload,
            issued_at,
            remember: false,
        };
        let (tx, rx) = oneshot::channel();
        self.push(node, cmd, Instant::now(), None, tx)?;
        tracing::debug!(node, %id, %kind, "command enqueued");
        Ok(self.ticket(id, node, kind, rx))
    }

    /// Queue a command that must not be lost or repeated when the leader
    /// changes. `key` names what the command acts on (`machine/<id>`); a
    /// node has at most one outstanding command per key.
    ///
    /// The id is recorded in the store. Issuing the same kind for the same
    /// key again, here or on the next leader, gives the command the id it
    /// had, for as long as the first would have been waited for. The agent
    /// answers a repeat of an id it has completed without running it, so
    /// the caller gets the first run's result. A different kind for the
    /// key replaces what was there and gets a new id. An earlier ticket for
    /// the key fails with [`RpcError::Closed`].
    pub fn enqueue_keyed(
        &self,
        node: &str,
        key: &str,
        kind: CommandKind,
        payload: serde_json::Value,
    ) -> Result<Ticket, RpcError> {
        self.reap();
        let record = record_key(node, key);
        let _one = self.0.keyed_lock.lock();
        let (tx, rx) = oneshot::channel();
        {
            let mut st = self.0.state.lock();
            if let Some(id) = st.keyed.get(&record).copied() {
                match st.pending.get_mut(&id) {
                    // Already queued or with the agent: wait for that one.
                    Some(p) if p.kind == kind => {
                        p.waiter = Some(tx);
                        return Ok(self.ticket(id, node, kind, rx));
                    }
                    _ => {
                        st.forget(id);
                    }
                }
            }
        }
        let now = Utc::now();
        let timeout = self.0.cfg.command_timeout;
        let store = self.0.store.get();
        let earlier = store
            .and_then(|s| s.get_meta::<Record>(&record).ok().flatten())
            .filter(|r| r.kind == kind)
            .and_then(|r| {
                let age = (now - r.issued_at).to_std().unwrap_or_default();
                (age < timeout).then_some((r, age))
            });
        let adopted = earlier.is_some();
        let (id, issued_at, age) = match earlier {
            Some((r, age)) => (r.id, r.issued_at, age),
            None => {
                let id = Uuid::now_v7();
                if let Some(s) = store {
                    let rec = Record {
                        id,
                        kind,
                        issued_at: now,
                    };
                    s.put_meta(&record, &rec)
                        .map_err(|e| RpcError::Store(e.to_string()))?;
                }
                (id, now, Duration::ZERO)
            }
        };
        let cmd = Command {
            id,
            kind,
            payload,
            issued_at,
            remember: true,
        };
        let enqueued = Instant::now();
        let enqueued = enqueued.checked_sub(age).unwrap_or(enqueued);
        if let Err(e) = self.push(node, cmd, enqueued, Some(record.clone()), tx) {
            if !adopted {
                self.drop_records(vec![record]);
            }
            return Err(e);
        }
        tracing::debug!(node, %id, %kind, key, adopted, "keyed command enqueued");
        Ok(self.ticket(id, node, kind, rx))
    }

    fn ticket(
        &self,
        id: CommandId,
        node: &str,
        kind: CommandKind,
        rx: oneshot::Receiver<Outcome>,
    ) -> Ticket {
        Ticket {
            id,
            node: node.to_owned(),
            kind,
            queue: self.clone(),
            rx,
        }
    }

    /// Put `cmd` on the node's FIFO and track it as pending.
    fn push(
        &self,
        node: &str,
        cmd: Command,
        enqueued: Instant,
        record: Option<String>,
        waiter: oneshot::Sender<Outcome>,
    ) -> Result<(), RpcError> {
        let (id, kind, issued_at) = (cmd.id, cmd.kind, cmd.issued_at);
        let mut st = self.0.state.lock();
        let e = self.entry(&mut st, node);
        e.tx.try_send(cmd).map_err(|_| RpcError::QueueFull {
            node: node.to_owned(),
        })?;
        if let Some(record) = &record {
            st.keyed.insert(record.clone(), id);
        }
        st.pending.insert(
            id,
            Pending {
                node: node.to_owned(),
                kind,
                issued_at,
                enqueued,
                delivered: false,
                waiter: Some(waiter),
                record,
            },
        );
        Ok(())
    }

    /// Remove records from the store in one write. A failure is logged and
    /// left: a record nobody owns goes at the next sweep, and a server that
    /// is no longer the leader cannot write at all.
    fn drop_records(&self, keys: Vec<String>) {
        let Some(store) = self.0.store.get() else {
            return;
        };
        if let Err(e) = store.delete_meta_many(&keys) {
            tracing::debug!(error = %e, count = keys.len(), "command records were not removed");
        }
    }

    /// Remove records that are older than `command_timeout` and belong to
    /// no pending command here: what an earlier leader left for commands
    /// nobody issued again. Returns how many went.
    pub fn sweep_records(&self) -> usize {
        let Some(store) = self.0.store.get() else {
            return 0;
        };
        let Ok(rows) = store.raw_scan("meta", COMMAND_RECORD_PREFIX) else {
            return 0;
        };
        let held: HashSet<String> = self.0.state.lock().keyed.keys().cloned().collect();
        let now = Utc::now();
        let timeout = self.0.cfg.command_timeout;
        let stale: Vec<String> = rows
            .into_iter()
            .filter(|(key, _)| !held.contains(key))
            .filter(|(_, bytes)| match serde_json::from_slice::<Record>(bytes) {
                Ok(r) => (now - r.issued_at).to_std().is_ok_and(|age| age >= timeout),
                Err(_) => true,
            })
            .map(|(key, _)| key)
            .collect();
        let n = stale.len();
        self.drop_records(stale);
        n
    }

    /// Agent long-poll: up to `max` commands for `node`, waiting at most
    /// `wait` for the first one. Returns an empty vector on timeout.
    /// Commands that expired while queued are skipped.
    pub async fn next(&self, node: &str, wait: Duration, max: usize) -> Vec<Command> {
        self.reap();
        let max = max.max(1);
        let rx = {
            let mut st = self.0.state.lock();
            self.entry(&mut st, node).rx.clone()
        };
        // One poller per node at a time; a second concurrent poll waits here
        // and gets whatever the first leaves behind.
        let mut rx = rx.lock().await;
        let deadline = Instant::now() + wait;
        let mut out = Vec::new();
        loop {
            while out.len() < max {
                match rx.try_recv() {
                    Ok(cmd) => {
                        if self.mark_delivered(cmd.id) {
                            out.push(cmd);
                        }
                    }
                    Err(_) => break,
                }
            }
            if !out.is_empty() {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match tokio::time::timeout(deadline - now, rx.recv()).await {
                Ok(Some(cmd)) => {
                    if self.mark_delivered(cmd.id) {
                        out.push(cmd);
                    }
                }
                // Channel closed (node removed) or timed out.
                Ok(None) | Err(_) => break,
            }
        }
        if !out.is_empty() {
            tracing::debug!(node, count = out.len(), "commands delivered");
        }
        out
    }

    /// Returns `false` if the command is no longer pending (expired).
    fn mark_delivered(&self, id: CommandId) -> bool {
        let mut st = self.0.state.lock();
        match st.pending.get_mut(&id) {
            Some(p) => {
                p.delivered = true;
                true
            }
            None => false,
        }
    }

    /// Deliver a result from `node`. The command must be pending and must
    /// have been issued to that node.
    pub fn ack(&self, node: &str, result: CommandResult) -> Result<(), RpcError> {
        let id = result.id;
        if self.ack_all(node, vec![result]).accepted == 1 {
            Ok(())
        } else {
            Err(RpcError::UnknownCommand(id))
        }
    }

    /// Deliver a batch of results from `node`. The records of the keyed
    /// commands among them are removed in one store write, before any
    /// waiter hears. In that order a leader that dies in between leaves a
    /// command that is issued again under a new id and run again by the
    /// agent, which every keyed command allows. The other order could leave
    /// the id of a finished command to be handed to a later one.
    pub fn ack_all(&self, node: &str, results: Vec<CommandResult>) -> ReportAck {
        self.reap();
        let mut ack = ReportAck::default();
        let mut done = Vec::new();
        let mut records = Vec::new();
        {
            let mut st = self.0.state.lock();
            for r in results {
                let ours = st.pending.get(&r.id).is_some_and(|p| p.node == node);
                match ours.then(|| st.forget(r.id)).flatten() {
                    Some(p) => {
                        records.extend(p.record.clone());
                        done.push((p, r));
                    }
                    None => ack.unknown.push(r.id),
                }
            }
        }
        if !records.is_empty() {
            self.drop_records(records);
        }
        for (p, result) in done {
            tracing::debug!(node, id = %result.id, kind = %p.kind, ok = result.ok, "command acked");
            ack.accepted += 1;
            if let Some(w) = p.waiter {
                // Receiver gone means nobody cares about the result; fine.
                let _ = w.send(Ok(result));
            }
        }
        ack
    }

    /// Forget a node: drop its queue and fail every pending command to it.
    pub fn remove_node(&self, node: &str) {
        self.drop_commands(node);
        let entry = self.0.state.lock().nodes.remove(node);
        drop(entry);
    }

    /// Fail every pending command to `node` and remove the node's records,
    /// its own and any an earlier leader left. For a host that was removed,
    /// or evacuated: what was asked of it no longer applies. The node stays
    /// known, and commands still on its FIFO are skipped when it polls.
    /// Returns how many pending commands were failed.
    pub fn drop_commands(&self, node: &str) -> usize {
        let failed: Vec<(CommandId, Pending)> = {
            let mut st = self.0.state.lock();
            let ids: Vec<CommandId> = st
                .pending
                .iter()
                .filter(|(_, p)| p.node == node)
                .map(|(id, _)| *id)
                .collect();
            ids.into_iter()
                .filter_map(|id| st.forget(id).map(|p| (id, p)))
                .collect()
        };
        if let Some(store) = self.0.store.get() {
            let records: Vec<String> = store
                .raw_scan("meta", &record_key(node, ""))
                .unwrap_or_default()
                .into_iter()
                .map(|(key, _)| key)
                .collect();
            if !records.is_empty() {
                self.drop_records(records);
            }
        }
        let n = failed.len();
        for (id, p) in failed {
            if let Some(w) = p.waiter {
                let _ = w.send(Err(RpcError::NodeRemoved {
                    id,
                    node: node.to_owned(),
                }));
            }
        }
        n
    }

    /// Fail every pending command older than `command_timeout`. Called
    /// automatically by the other operations; call it from a timer too if
    /// the queue can go quiet with waiters outstanding and nobody awaits
    /// tickets. Returns how many expired.
    pub fn reap(&self) -> usize {
        let timeout = self.0.cfg.command_timeout;
        let now = Instant::now();
        let expired: Vec<(CommandId, Pending)> = {
            let mut st = self.0.state.lock();
            let ids: Vec<CommandId> = st
                .pending
                .iter()
                .filter(|(_, p)| now.duration_since(p.enqueued) >= timeout)
                .map(|(id, _)| *id)
                .collect();
            ids.into_iter()
                .filter_map(|id| st.forget(id).map(|p| (id, p)))
                .collect()
        };
        let records: Vec<String> = expired
            .iter()
            .filter_map(|(_, p)| p.record.clone())
            .collect();
        if !records.is_empty() {
            self.drop_records(records);
        }
        let due = {
            let mut swept = self.0.swept.lock();
            let due = swept.is_none_or(|at| now.duration_since(at) >= SWEEP_EVERY);
            if due {
                *swept = Some(now);
            }
            due
        };
        if due {
            self.sweep_records();
        }
        let n = expired.len();
        for (id, p) in expired {
            tracing::warn!(node = %p.node, %id, kind = %p.kind, delivered = p.delivered, "command timed out");
            if let Some(w) = p.waiter {
                let _ = w.send(Err(RpcError::Timeout {
                    id,
                    kind: p.kind,
                    node: p.node,
                }));
            }
        }
        n
    }

    /// Expire one command now, regardless of age. Used by [`Ticket::wait`].
    fn expire(&self, id: CommandId) {
        let p = self.0.state.lock().forget(id);
        if let Some(p) = p {
            tracing::warn!(node = %p.node, %id, kind = %p.kind, "command timed out");
            if let Some(record) = p.record {
                self.drop_records(vec![record]);
            }
        }
    }

    /// Time left before `id` times out; zero if it is not pending.
    fn remaining(&self, id: CommandId) -> Duration {
        let st = self.0.state.lock();
        match st.pending.get(&id) {
            Some(p) => self
                .0
                .cfg
                .command_timeout
                .saturating_sub(p.enqueued.elapsed()),
            None => Duration::ZERO,
        }
    }

    /// Snapshot of one node.
    pub fn node(&self, node: &str) -> Option<NodeInfo> {
        let st = self.0.state.lock();
        st.nodes.get(node).map(|e| self.snapshot(&st, e))
    }

    /// Snapshot of every node, sorted by name.
    pub fn nodes(&self) -> Vec<NodeInfo> {
        let st = self.0.state.lock();
        let mut v: Vec<NodeInfo> = st.nodes.values().map(|e| self.snapshot(&st, e)).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    fn snapshot(&self, st: &State, e: &NodeEntry) -> NodeInfo {
        let mut info = e.info.clone();
        info.queued = self.0.cfg.capacity - e.tx.capacity();
        info.in_flight = st
            .pending
            .values()
            .filter(|p| p.node == info.name && p.delivered)
            .count();
        info
    }

    /// Every pending command, oldest first.
    pub fn pending(&self) -> Vec<PendingInfo> {
        let st = self.0.state.lock();
        let mut v: Vec<PendingInfo> = st
            .pending
            .iter()
            .map(|(id, p)| PendingInfo {
                id: *id,
                node: p.node.clone(),
                kind: p.kind,
                issued_at: p.issued_at,
                delivered: p.delivered,
            })
            .collect();
        v.sort_by_key(|p| p.id);
        v
    }

    pub fn pending_count(&self) -> usize {
        self.0.state.lock().pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg(timeout_secs: u64) -> QueueConfig {
        QueueConfig {
            command_timeout: Duration::from_secs(timeout_secs),
            capacity: 4,
            heartbeat_interval: Duration::from_secs(5),
            ovn: None,
        }
    }

    #[tokio::test]
    async fn enqueue_then_poll_then_ack() {
        let q = CommandQueue::new(cfg(60));
        let t = q
            .enqueue("n1", CommandKind::StartMachine, json!({"machine": "web"}))
            .unwrap();
        let id = t.id();
        assert_eq!(q.pending_count(), 1);
        assert_eq!(q.node("n1").unwrap().queued, 1);

        let cmds = q.next("n1", Duration::from_millis(10), 16).await;
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].id, id);
        assert_eq!(cmds[0].kind, CommandKind::StartMachine);
        assert_eq!(cmds[0].payload["machine"], "web");
        let info = q.node("n1").unwrap();
        assert_eq!(info.queued, 0);
        assert_eq!(info.in_flight, 1);
        assert!(q.pending()[0].delivered);

        q.ack("n1", CommandResult::ok(id).with_payload(json!({"pid": 42})))
            .unwrap();
        let r = t.wait().await.unwrap();
        assert!(r.ok);
        assert_eq!(r.payload["pid"], 42);
        assert_eq!(q.pending_count(), 0);
        assert_eq!(q.node("n1").unwrap().in_flight, 0);
    }

    #[tokio::test]
    async fn long_poll_receives_command_enqueued_later() {
        let q = CommandQueue::new(cfg(60));
        let q2 = q.clone();
        let poller = tokio::spawn(async move { q2.next("n1", Duration::from_secs(5), 16).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let t = q
            .enqueue("n1", CommandKind::StopMachine, json!(null))
            .unwrap();
        let cmds = poller.await.unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].id, t.id());
    }

    #[tokio::test]
    async fn long_poll_times_out_empty() {
        let q = CommandQueue::new(cfg(60));
        let start = Instant::now();
        let cmds = q.next("idle", Duration::from_millis(100), 16).await;
        assert!(cmds.is_empty());
        assert!(start.elapsed() >= Duration::from_millis(100));
    }

    #[tokio::test]
    async fn batches_respect_max_and_order() {
        let q = CommandQueue::new(cfg(60));
        let ids: Vec<CommandId> = (0..3)
            .map(|_| {
                q.enqueue("n1", CommandKind::PlugPort, json!(null))
                    .unwrap()
                    .id()
            })
            .collect();
        let first = q.next("n1", Duration::ZERO, 2).await;
        assert_eq!(first.iter().map(|c| c.id).collect::<Vec<_>>(), ids[..2]);
        let rest = q.next("n1", Duration::ZERO, 2).await;
        assert_eq!(rest.iter().map(|c| c.id).collect::<Vec<_>>(), ids[2..]);
        assert!(q.next("n1", Duration::ZERO, 2).await.is_empty());
    }

    #[tokio::test]
    async fn queues_are_per_node() {
        let q = CommandQueue::new(cfg(60));
        let a = q
            .enqueue("a", CommandKind::StartMachine, json!(null))
            .unwrap();
        let _b = q
            .enqueue("b", CommandKind::StartMachine, json!(null))
            .unwrap();
        let got = q.next("a", Duration::ZERO, 16).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, a.id());
        assert_eq!(q.node("b").unwrap().queued, 1);
        assert_eq!(
            q.nodes()
                .iter()
                .map(|n| n.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[tokio::test]
    async fn ack_unknown_or_wrong_node_is_rejected() {
        let q = CommandQueue::new(cfg(60));
        let t = q
            .enqueue("n1", CommandKind::AttachVolume, json!(null))
            .unwrap();
        let _ = q.next("n1", Duration::ZERO, 16).await;
        let e = q.ack("n2", CommandResult::ok(t.id())).unwrap_err();
        assert!(matches!(e, RpcError::UnknownCommand(id) if id == t.id()));
        let e = q.ack("n1", CommandResult::ok(Uuid::now_v7())).unwrap_err();
        assert!(matches!(e, RpcError::UnknownCommand(_)));
        // The right node still works, once.
        q.ack("n1", CommandResult::failed(t.id(), "no such volume"))
            .unwrap();
        let e = q.ack("n1", CommandResult::ok(t.id())).unwrap_err();
        assert!(matches!(e, RpcError::UnknownCommand(_)));
        let r = t.wait().await.unwrap();
        assert!(!r.ok);
        assert_eq!(r.message, "no such volume");
    }

    #[tokio::test]
    async fn queue_full() {
        let q = CommandQueue::new(cfg(60));
        let mut tickets = Vec::new();
        for _ in 0..4 {
            tickets.push(
                q.enqueue("n1", CommandKind::StartMachine, json!(null))
                    .unwrap(),
            );
        }
        let e = q
            .enqueue("n1", CommandKind::StartMachine, json!(null))
            .unwrap_err();
        assert!(matches!(e, RpcError::QueueFull { ref node } if node == "n1"));
        assert_eq!(e.to_string(), "command queue for node n1 is full");
        // Draining makes room; pending stays until acked.
        assert_eq!(q.next("n1", Duration::ZERO, 16).await.len(), 4);
        assert!(q
            .enqueue("n1", CommandKind::StartMachine, json!(null))
            .is_ok());
        assert_eq!(q.pending_count(), 5);
    }

    #[tokio::test(start_paused = true)]
    async fn ticket_times_out_when_undelivered() {
        let q = CommandQueue::new(cfg(10));
        let t = q
            .enqueue("offline", CommandKind::DeleteMachine, json!(null))
            .unwrap();
        let id = t.id();
        let e = t.wait().await.unwrap_err();
        assert!(
            matches!(e, RpcError::Timeout { id: tid, kind: CommandKind::DeleteMachine, ref node } if tid == id && node == "offline"),
            "{e}"
        );
        assert_eq!(q.pending_count(), 0);
        // The stale command is still in the channel but is skipped on poll.
        assert!(q.next("offline", Duration::ZERO, 16).await.is_empty());
        assert_eq!(q.node("offline").unwrap().queued, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn delivered_but_unanswered_times_out_via_reap() {
        let q = CommandQueue::new(cfg(10));
        let t = q
            .enqueue("n1", CommandKind::StartMachine, json!(null))
            .unwrap();
        assert_eq!(q.next("n1", Duration::ZERO, 16).await.len(), 1);
        tokio::time::advance(Duration::from_secs(11)).await;
        // Any queue operation reaps; here an explicit call.
        assert_eq!(q.reap(), 1);
        assert_eq!(q.pending_count(), 0);
        let e = t.wait().await.unwrap_err();
        assert!(matches!(e, RpcError::Timeout { .. }));
        // A late result is rejected.
        assert!(matches!(
            q.ack("n1", CommandResult::ok(e_id(&e))),
            Err(RpcError::UnknownCommand(_))
        ));
    }

    fn e_id(e: &RpcError) -> CommandId {
        match e {
            RpcError::Timeout { id, .. } => *id,
            _ => panic!("not a timeout"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn reap_is_triggered_by_other_operations() {
        let q = CommandQueue::new(cfg(10));
        let t = q
            .enqueue("n1", CommandKind::StartMachine, json!(null))
            .unwrap();
        tokio::time::advance(Duration::from_secs(10)).await;
        // A heartbeat from any node reaps expired commands.
        let ack = q.heartbeat(&Heartbeat {
            node_name: "n2".into(),
            resources_free: Resources::default(),
            machines: vec![],
            at: Utc::now(),
            ovn: None,
        });
        assert!(!ack.known);
        assert_eq!(q.pending_count(), 0);
        assert!(matches!(t.wait().await, Err(RpcError::Timeout { .. })));
    }

    #[tokio::test]
    async fn remove_node_fails_pending() {
        let q = CommandQueue::new(cfg(60));
        let t = q
            .enqueue("n1", CommandKind::StartMachine, json!(null))
            .unwrap();
        q.remove_node("n1");
        assert!(q.node("n1").is_none());
        let e = t.wait().await.unwrap_err();
        assert!(
            matches!(e, RpcError::NodeRemoved { ref node, .. } if node == "n1"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn hello_and_heartbeat_update_node_info() {
        let q = CommandQueue::new(cfg(60));
        let ack = q.hello(&AgentHello {
            node_name: "n1".into(),
            capabilities: vec!["kvm".into()],
            resources: Resources {
                vcpus: 8,
                memory_mib: 16384,
                disk_gib: 100,
                gpus: 0,
            },
            version: "26.11.12.1".into(),
            labels: Default::default(),
            underlay_ip: Some("10.66.66.5".into()),
        });
        assert_eq!(ack.node_name, "n1");
        assert_eq!(ack.heartbeat_interval_secs, 5);
        assert_eq!(ack.max_long_poll_secs, MAX_LONG_POLL_SECS);

        let _t = q
            .enqueue("n1", CommandKind::StartMachine, json!(null))
            .unwrap();
        let hb = q.heartbeat(&Heartbeat {
            node_name: "n1".into(),
            resources_free: Resources {
                vcpus: 6,
                memory_mib: 8192,
                disk_gib: 50,
                gpus: 0,
            },
            machines: vec![],
            at: Utc::now(),
            ovn: None,
        });
        assert!(hb.known);
        assert_eq!(hb.commands_waiting, 1);

        let info = q.node("n1").unwrap();
        assert_eq!(info.capabilities, vec!["kvm"]);
        assert_eq!(info.resources_total.vcpus, 8);
        assert_eq!(info.resources_free.vcpus, 6);
        assert_eq!(info.underlay_ip.as_deref(), Some("10.66.66.5"));
        assert!(info.last_hello.is_some() && info.last_heartbeat.is_some());
        assert_eq!(info.queued, 1);
    }

    #[tokio::test]
    async fn hello_and_heartbeat_are_broadcast() {
        let q = CommandQueue::new(cfg(60));
        let mut rx = q.subscribe_agent_events();
        let hello = AgentHello {
            node_name: "n1".into(),
            capabilities: vec!["mock".into()],
            resources: Resources::default(),
            version: String::new(),
            labels: Default::default(),
            underlay_ip: None,
        };
        q.hello(&hello);
        let hb = Heartbeat {
            node_name: "n1".into(),
            resources_free: Resources::default(),
            machines: vec![],
            at: Utc::now(),
            ovn: None,
        };
        q.heartbeat(&hb);
        assert_eq!(rx.recv().await.unwrap(), AgentEvent::Hello(hello));
        let got = rx.recv().await.unwrap();
        assert_eq!(got.node_name(), "n1");
        assert_eq!(got, AgentEvent::Heartbeat(hb));
    }

    // ---- keyed commands ------------------------------------------------------

    fn records(store: &Store) -> Vec<String> {
        store
            .raw_scan("meta", COMMAND_RECORD_PREFIX)
            .unwrap()
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    }

    /// A queue as a server has it: a fresh one over the same store is that
    /// server restarted, or the next leader.
    fn queue_on(store: &Store, timeout_secs: u64) -> CommandQueue {
        let q = CommandQueue::new(cfg(timeout_secs));
        q.attach_store(store.clone());
        q
    }

    #[tokio::test]
    async fn a_keyed_command_keeps_its_id_when_the_queue_restarts() {
        let store = Store::in_memory().unwrap();
        let first = queue_on(&store, 60);
        let lost = first
            .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(1))
            .unwrap();
        let id = lost.id();
        assert_eq!(records(&store), vec!["rpc/commands/n1/machine/m1"]);
        drop((first, lost));

        // The new queue holds no commands; the module issues its own again.
        let second = queue_on(&store, 60);
        assert!(second.next("n1", Duration::ZERO, 16).await.is_empty());
        let t = second
            .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(2))
            .unwrap();
        assert_eq!(t.id(), id);
        let cmds = second.next("n1", Duration::ZERO, 16).await;
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].id, id);
        assert!(cmds[0].remember);
        assert_eq!(cmds[0].payload, json!(2));
        second.ack("n1", CommandResult::ok(id)).unwrap();
        assert!(t.wait().await.unwrap().ok);
        assert!(records(&store).is_empty());
        assert!(second.next("n1", Duration::ZERO, 16).await.is_empty());
    }

    #[tokio::test]
    async fn a_result_for_a_command_in_flight_at_failover_reaches_the_new_leader() {
        let store = Store::in_memory().unwrap();
        let old = queue_on(&store, 60);
        let lost = old
            .enqueue_keyed("n1", "volume/v1", CommandKind::AttachVolume, json!(null))
            .unwrap();
        let id = lost.id();
        // The agent has it when the leader goes.
        assert_eq!(old.next("n1", Duration::ZERO, 16).await.len(), 1);
        drop((old, lost));

        let new = queue_on(&store, 60);
        // Before the module has issued it again the new leader cannot place
        // the result. The agent keeps it and answers the redelivery.
        let early = new.ack_all("n1", vec![CommandResult::ok(id)]);
        assert_eq!(early.accepted, 0);
        assert_eq!(early.unknown, vec![id]);
        assert_eq!(records(&store).len(), 1);

        let t = new
            .enqueue_keyed("n1", "volume/v1", CommandKind::AttachVolume, json!(null))
            .unwrap();
        assert_eq!(t.id(), id);
        let again = new.next("n1", Duration::ZERO, 16).await;
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].id, id);
        let ack = new.ack_all(
            "n1",
            vec![CommandResult::ok(id).with_payload(json!({"device": "vdb"}))],
        );
        assert_eq!(ack.accepted, 1);
        assert_eq!(t.wait().await.unwrap().payload["device"], "vdb");
        assert!(records(&store).is_empty());
    }

    #[tokio::test]
    async fn a_result_that_arrives_before_the_redelivery_is_taken_once() {
        let store = Store::in_memory().unwrap();
        let old = queue_on(&store, 60);
        let id = old
            .enqueue_keyed("n1", "machine/m1", CommandKind::StopMachine, json!(null))
            .unwrap()
            .id();
        assert_eq!(old.next("n1", Duration::ZERO, 16).await.len(), 1);
        drop(old);

        let new = queue_on(&store, 60);
        let t = new
            .enqueue_keyed("n1", "machine/m1", CommandKind::StopMachine, json!(null))
            .unwrap();
        // The agent reports what it ran for the old leader.
        new.ack("n1", CommandResult::ok(id)).unwrap();
        assert!(t.wait().await.unwrap().ok);
        // The copy on the FIFO is no longer pending and is not handed out.
        assert!(new.next("n1", Duration::ZERO, 16).await.is_empty());
        assert!(matches!(
            new.ack("n1", CommandResult::ok(id)),
            Err(RpcError::UnknownCommand(_))
        ));
    }

    #[tokio::test]
    async fn the_same_key_twice_is_one_command() {
        let store = Store::in_memory().unwrap();
        let q = queue_on(&store, 60);
        let a = q
            .enqueue_keyed("n1", "machine/m1", CommandKind::DeleteMachine, json!(null))
            .unwrap();
        let b = q
            .enqueue_keyed("n1", "machine/m1", CommandKind::DeleteMachine, json!(null))
            .unwrap();
        assert_eq!(a.id(), b.id());
        assert!(matches!(a.wait().await, Err(RpcError::Closed)));
        assert_eq!(q.pending_count(), 1);
        let cmds = q.next("n1", Duration::ZERO, 16).await;
        assert_eq!(cmds.len(), 1);
        q.ack("n1", CommandResult::ok(cmds[0].id)).unwrap();
        assert!(b.wait().await.unwrap().ok);

        // Answered: the next one for the key is a new command.
        let c = q
            .enqueue_keyed("n1", "machine/m1", CommandKind::DeleteMachine, json!(null))
            .unwrap();
        assert_ne!(c.id(), cmds[0].id);
    }

    #[tokio::test]
    async fn another_kind_for_the_key_is_a_new_command() {
        let store = Store::in_memory().unwrap();
        let q = queue_on(&store, 60);
        let stop = q
            .enqueue_keyed("n1", "machine/m1", CommandKind::StopMachine, json!(null))
            .unwrap();
        let stop_id = stop.id();
        let start = q
            .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(null))
            .unwrap();
        assert_ne!(start.id(), stop_id);
        assert!(matches!(stop.wait().await, Err(RpcError::Closed)));
        // Only the start is handed out, and one record is kept for the key.
        let cmds = q.next("n1", Duration::ZERO, 16).await;
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].id, start.id());
        assert_eq!(records(&store).len(), 1);

        // The same across a restart: a stop left by the old leader is not
        // the id of the start the new one issues.
        let store = Store::in_memory().unwrap();
        let old = queue_on(&store, 60);
        let left = old
            .enqueue_keyed("n1", "machine/m1", CommandKind::StopMachine, json!(null))
            .unwrap()
            .id();
        let new = queue_on(&store, 60);
        let start = new
            .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(null))
            .unwrap();
        assert_ne!(start.id(), left);
        let stop = new
            .enqueue_keyed("n1", "machine/m1", CommandKind::StopMachine, json!(null))
            .unwrap();
        assert_ne!(stop.id(), left);
    }

    #[tokio::test]
    async fn a_record_older_than_the_timeout_is_not_used_and_is_swept() {
        let store = Store::in_memory().unwrap();
        let old = Record {
            id: Uuid::now_v7(),
            kind: CommandKind::StartMachine,
            issued_at: Utc::now() - chrono::Duration::seconds(61),
        };
        store
            .put_meta(&record_key("n1", "machine/m1"), &old)
            .unwrap();
        store
            .put_meta(&record_key("n2", "machine/m2"), &old)
            .unwrap();
        let q = queue_on(&store, 60);
        let t = q
            .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(null))
            .unwrap();
        assert_ne!(t.id(), old.id);
        // The first operation swept: what nobody issued again is gone, what
        // is pending here stays.
        assert_eq!(records(&store), vec!["rpc/commands/n1/machine/m1"]);
        store
            .put_meta(&record_key("n2", "machine/m2"), &old)
            .unwrap();
        assert_eq!(q.sweep_records(), 1);
        assert_eq!(records(&store), vec!["rpc/commands/n1/machine/m1"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_that_stays_down_does_not_grow_the_store() {
        let store = Store::in_memory().unwrap();
        // Capacity 4, timeout 10 s, and an agent that never polls.
        let q = queue_on(&store, 10);
        let issue = |n: usize| {
            q.enqueue_keyed(
                "down",
                &format!("machine/m{n}"),
                CommandKind::DeleteMachine,
                json!(null),
            )
        };
        for _ in 0..5 {
            let tickets: Vec<_> = (0..6).map(issue).collect();
            // Four fit. The rest are refused and leave nothing behind.
            assert_eq!(tickets.iter().filter(|t| t.is_ok()).count(), 4);
            assert!(matches!(tickets[5], Err(RpcError::QueueFull { .. })));
            assert_eq!(records(&store).len(), 4);
            for t in tickets.into_iter().flatten() {
                assert!(matches!(t.wait().await, Err(RpcError::Timeout { .. })));
            }
            // Timed out: no record outlives its command.
            assert!(records(&store).is_empty());
            assert_eq!(q.pending_count(), 0);
            // The dead commands hold their places on the FIFO until the
            // agent polls, so a host that never does is refused from here on.
            assert!(matches!(issue(0), Err(RpcError::QueueFull { .. })));
            assert!(records(&store).is_empty());
            assert!(q.next("down", Duration::ZERO, 16).await.is_empty());
        }
    }

    #[tokio::test]
    async fn dropping_a_nodes_commands_fails_them_and_removes_its_records() {
        let store = Store::in_memory().unwrap();
        let old = queue_on(&store, 60);
        // One left by an earlier leader, one pending here, one for another node.
        let _left = old
            .enqueue_keyed("gone", "machine/m1", CommandKind::StartMachine, json!(null))
            .unwrap();
        let q = queue_on(&store, 60);
        let t = q
            .enqueue_keyed("gone", "volume/v1", CommandKind::AttachVolume, json!(null))
            .unwrap();
        let _other = q
            .enqueue_keyed(
                "stays",
                "machine/m2",
                CommandKind::StartMachine,
                json!(null),
            )
            .unwrap();
        assert_eq!(records(&store).len(), 3);

        assert_eq!(q.drop_commands("gone"), 1);
        assert!(matches!(
            t.wait().await,
            Err(RpcError::NodeRemoved { ref node, .. }) if node == "gone"
        ));
        assert_eq!(records(&store), vec!["rpc/commands/stays/machine/m2"]);
        // The host is still known, and gets nothing of what was dropped.
        assert!(q.node("gone").is_some());
        assert!(q.next("gone", Duration::ZERO, 16).await.is_empty());
        // A command issued afterwards is a new one.
        let again = q
            .enqueue_keyed("gone", "machine/m1", CommandKind::StartMachine, json!(null))
            .unwrap();
        assert_ne!(again.id(), _left.id());
    }

    #[tokio::test]
    async fn a_keyed_command_that_cannot_be_recorded_is_not_queued() {
        let store = Store::in_memory().unwrap();
        let q = queue_on(&store, 60);
        // A server that is not the leader cannot write.
        store.set_write_fence(true);
        let e = q
            .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(null))
            .unwrap_err();
        assert!(matches!(e, RpcError::Store(_)), "{e}");
        assert_eq!(q.pending_count(), 0);
        assert!(q.next("n1", Duration::ZERO, 16).await.is_empty());
    }

    #[tokio::test]
    async fn without_a_store_a_keyed_command_still_runs() {
        let q = CommandQueue::new(cfg(60));
        let t = q
            .enqueue_keyed("n1", "machine/m1", CommandKind::StartMachine, json!(null))
            .unwrap();
        let cmds = q.next("n1", Duration::ZERO, 16).await;
        assert_eq!(cmds[0].id, t.id());
        q.ack("n1", CommandResult::ok(t.id())).unwrap();
        assert!(t.wait().await.unwrap().ok);
    }
}
