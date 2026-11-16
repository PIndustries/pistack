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

pub(crate) mod command_queue;
pub(crate) mod consts;
pub(crate) mod default;
pub(crate) mod fmt;
pub(crate) mod id;
pub(crate) mod inner;
pub mod lifecycle;
pub mod nodes;
pub(crate) mod outcome;
pub(crate) mod pending;
pub(crate) mod pending_info;
pub(crate) mod queue_config;
pub(crate) mod record;
pub(crate) mod record_key;
pub(crate) mod ticket;
pub(crate) mod wait;

pub use command_queue::CommandQueue;
pub use consts::{AGENT_EVENT_CAPACITY, COMMAND_RECORD_PREFIX};
pub use nodes::NodeInfo;
pub use pending_info::PendingInfo;
pub use queue_config::QueueConfig;
pub use ticket::Ticket;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod wait_tests;
