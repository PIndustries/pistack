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
//! Timeouts are measured from enqueue, so a command to an offline node
//! fails after `command_timeout` too, rather than waiting for ever.
//! Expiry is checked lazily on every queue operation and whenever a ticket
//! is awaited; there is no background task, which keeps the queue usable
//! from tests with paused time.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use pistack_types::Resources;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::Instant;
use uuid::Uuid;

use crate::error::RpcError;
use crate::proto::{
    AgentEvent, AgentHello, Command, CommandId, CommandKind, CommandResult, Heartbeat,
    HeartbeatAck, HelloAck, MachineReport, MAX_LONG_POLL_SECS,
};

/// Capacity of the [`AgentEvent`] broadcast; a subscriber this far behind
/// sees `Lagged` and should resync from [`CommandQueue::nodes`].
pub const AGENT_EVENT_CAPACITY: usize = 1024;

/// Tunables for [`CommandQueue`].
#[derive(Clone, Debug)]
pub struct QueueConfig {
    /// From enqueue to failure if no result arrives.
    pub command_timeout: Duration,
    /// Undelivered commands a node may have queued before `enqueue` fails.
    pub capacity: usize,
    /// Interval agents are told to heartbeat at.
    pub heartbeat_interval: Duration,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            command_timeout: Duration::from_secs(120),
            capacity: 1024,
            heartbeat_interval: Duration::from_secs(15),
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
}

struct Inner {
    cfg: QueueConfig,
    state: Mutex<State>,
    agent_events: broadcast::Sender<AgentEvent>,
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
        }))
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
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut st = self.0.state.lock();
            let e = self.entry(&mut st, node);
            e.tx.try_send(cmd).map_err(|_| RpcError::QueueFull {
                node: node.to_owned(),
            })?;
            st.pending.insert(
                id,
                Pending {
                    node: node.to_owned(),
                    kind,
                    issued_at,
                    enqueued: Instant::now(),
                    delivered: false,
                    waiter: Some(tx),
                },
            );
        }
        tracing::debug!(node, %id, %kind, "command enqueued");
        Ok(Ticket {
            id,
            node: node.to_owned(),
            kind,
            queue: self.clone(),
            rx,
        })
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
        self.reap();
        let pending = {
            let mut st = self.0.state.lock();
            match st.pending.get(&result.id) {
                Some(p) if p.node == node => st.pending.remove(&result.id),
                _ => None,
            }
        };
        let Some(p) = pending else {
            return Err(RpcError::UnknownCommand(result.id));
        };
        tracing::debug!(node, id = %result.id, kind = %p.kind, ok = result.ok, "command acked");
        if let Some(w) = p.waiter {
            // Receiver gone means nobody cares about the result; fine.
            let _ = w.send(Ok(result));
        }
        Ok(())
    }

    /// Forget a node: drop its queue and fail every pending command to it.
    pub fn remove_node(&self, node: &str) {
        let (entry, failed) = {
            let mut st = self.0.state.lock();
            let entry = st.nodes.remove(node);
            let ids: Vec<CommandId> = st
                .pending
                .iter()
                .filter(|(_, p)| p.node == node)
                .map(|(id, _)| *id)
                .collect();
            let failed: Vec<(CommandId, Pending)> = ids
                .into_iter()
                .filter_map(|id| st.pending.remove(&id).map(|p| (id, p)))
                .collect();
            (entry, failed)
        };
        drop(entry);
        for (id, p) in failed {
            if let Some(w) = p.waiter {
                let _ = w.send(Err(RpcError::NodeRemoved {
                    id,
                    node: node.to_owned(),
                }));
            }
        }
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
                .filter_map(|id| st.pending.remove(&id).map(|p| (id, p)))
                .collect()
        };
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
        let p = self.0.state.lock().pending.remove(&id);
        if let Some(p) = p {
            tracing::warn!(node = %p.node, %id, kind = %p.kind, "command timed out");
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
            version: "0.1.0".into(),
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
        };
        q.heartbeat(&hb);
        assert_eq!(rx.recv().await.unwrap(), AgentEvent::Hello(hello));
        let got = rx.recv().await.unwrap();
        assert_eq!(got.node_name(), "n1");
        assert_eq!(got, AgentEvent::Heartbeat(hb));
    }
}
