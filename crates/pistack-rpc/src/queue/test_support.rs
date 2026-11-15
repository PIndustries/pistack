// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use pistack_store::Store;

use crate::error::RpcError;
use crate::queue::consts::COMMAND_RECORD_PREFIX;
use crate::queue::queue_config::QueueConfig;

pub(crate) fn cfg(timeout_secs: u64) -> QueueConfig {
    QueueConfig {
        command_timeout: Duration::from_secs(timeout_secs),
        capacity: 4,
        heartbeat_interval: Duration::from_secs(5),
        ovn: None,
    }
}

pub(crate) fn e_id(e: &RpcError) -> CommandId {
    match e {
        RpcError::Timeout { id, .. } => *id,
        _ => panic!("not a timeout"),
    }
}

pub(crate) fn records(store: &Store) -> Vec<String> {
    store
        .raw_scan("meta", COMMAND_RECORD_PREFIX)
        .unwrap()
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

/// A queue as a server has it: a fresh one over the same store is that
/// server restarted, or the next leader.
pub(crate) fn queue_on(store: &Store, timeout_secs: u64) -> CommandQueue {
    let q = CommandQueue::new(cfg(timeout_secs));
    q.attach_store(store.clone());
    q
}

pub(crate) use crate::proto::{CommandId, CommandKind, CommandResult, Heartbeat};
pub(crate) use crate::queue::command_queue::CommandQueue;
