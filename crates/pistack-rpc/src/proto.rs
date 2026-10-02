// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Wire types shared by the control plane and `anvil`.
//!
//! The protocol is JSON over HTTP with a long-poll for commands:
//!
//! | Method | Path                       | Body            | Reply            |
//! | ------ | -------------------------- | --------------- | ---------------- |
//! | POST   | `/agents/hello`            | [`AgentHello`]  | [`HelloAck`]     |
//! | POST   | `/agents/heartbeat`        | [`Heartbeat`]   | [`HeartbeatAck`] |
//! | GET    | `/agents/{node}/commands`  | `?wait=&max=`   | `Vec<Command>`   |
//! | POST   | `/agents/{node}/results`   | `Vec<CommandResult>` | [`ReportAck`] |
//!
//! Every type here is `serde` and nothing else; the transport can be swapped
//! for gRPC later without touching the queue or the modules.

use chrono::{DateTime, Utc};
use pistack_types::{Id, Phase, Resources};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Longest a command long-poll may hang before an empty reply.
pub const MAX_LONG_POLL_SECS: u64 = 30;
/// Default `max` for a command poll.
pub const DEFAULT_POLL_BATCH: usize = 16;

/// First message an agent sends after it starts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentHello {
    pub node_name: String,
    /// What this host can run: "kvm", "container", "baremetal", "ovn",
    /// "lvm", "rbd", ...
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Total resources of the host.
    pub resources: Resources,
    /// Agent build version, for the operator's eyes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// Labels the operator set on the host (rack, zone, ...).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HelloAck {
    pub node_name: String,
    /// Server's idea of now; agents may use it to detect clock skew.
    pub server_time: DateTime<Utc>,
    /// How often the agent should send [`Heartbeat`].
    pub heartbeat_interval_secs: u64,
    /// Upper bound the server will honour for `?wait=` on the command poll.
    pub max_long_poll_secs: u64,
}

/// Periodic state report.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Heartbeat {
    pub node_name: String,
    pub resources_free: Resources,
    #[serde(default)]
    pub machines: Vec<MachineReport>,
    #[serde(default = "Utc::now")]
    pub at: DateTime<Utc>,
}

/// One machine as the host sees it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineReport {
    pub id: Id,
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Resources>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeartbeatAck {
    pub server_time: DateTime<Utc>,
    /// Commands waiting for this node; a hint to poll right away.
    pub commands_waiting: usize,
    /// `false` when the server has never seen a hello from this node (for
    /// example after a control-plane restart) and wants one.
    pub known: bool,
}

/// What the control plane asks a host to do.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    StartMachine,
    StopMachine,
    DeleteMachine,
    AttachVolume,
    DetachVolume,
    PlugPort,
    UnplugPort,
}

impl std::fmt::Display for CommandKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CommandKind::StartMachine => "start_machine",
            CommandKind::StopMachine => "stop_machine",
            CommandKind::DeleteMachine => "delete_machine",
            CommandKind::AttachVolume => "attach_volume",
            CommandKind::DetachVolume => "detach_volume",
            CommandKind::PlugPort => "plug_port",
            CommandKind::UnplugPort => "unplug_port",
        };
        f.write_str(s)
    }
}

/// Identifier of one command; a UUIDv7 so ids sort by issue time.
pub type CommandId = Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Command {
    pub id: CommandId,
    pub kind: CommandKind,
    /// Kind-specific body (machine spec, volume attachment, port), owned by
    /// the module that issued it.
    #[serde(default)]
    pub payload: serde_json::Value,
    pub issued_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandResult {
    pub id: CommandId,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl CommandResult {
    pub fn ok(id: CommandId) -> Self {
        Self {
            id,
            ok: true,
            message: String::new(),
            payload: serde_json::Value::Null,
        }
    }
    pub fn failed(id: CommandId, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            message: message.into(),
            payload: serde_json::Value::Null,
        }
    }
    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }
}

/// Reply to a batch of results.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReportAck {
    /// Results matched to a pending command.
    pub accepted: usize,
    /// Ids the server no longer knew (already timed out or duplicated).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<CommandId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_kind_wire_names() {
        let v = serde_json::to_value(CommandKind::AttachVolume).unwrap();
        assert_eq!(v, serde_json::json!("attach_volume"));
        assert_eq!(CommandKind::AttachVolume.to_string(), "attach_volume");
        let back: CommandKind = serde_json::from_value(v).unwrap();
        assert_eq!(back, CommandKind::AttachVolume);
    }

    #[test]
    fn hello_minimal_json() {
        let h: AgentHello =
            serde_json::from_str(r#"{"node_name":"n1","resources":{"vcpus":4,"memory_mib":8192}}"#)
                .unwrap();
        assert_eq!(h.node_name, "n1");
        assert!(h.capabilities.is_empty());
        assert_eq!(h.resources.vcpus, 4);
    }

    #[test]
    fn result_helpers() {
        let id = Uuid::now_v7();
        let r = CommandResult::failed(id, "boom").with_payload(serde_json::json!({"code": 7}));
        assert!(!r.ok);
        let text = serde_json::to_string(&r).unwrap();
        let back: CommandResult = serde_json::from_str(&text).unwrap();
        assert_eq!(back, r);
        let ok = CommandResult::ok(id);
        assert!(!serde_json::to_string(&ok).unwrap().contains("message"));
    }
}
