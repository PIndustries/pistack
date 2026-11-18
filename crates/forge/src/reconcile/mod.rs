// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The machine reconciler: makes `status` true to `spec`.
//!
//! Wakes on every `mach` store event and every [`TICK`], then runs one
//! idempotent pass over all hosts and machines:
//!
//! * hosts: locally-served hosts get a heartbeat; hosts unseen for
//!   [`HOST_STALE_AFTER`] go to `Error`; `status.free` is recomputed from
//!   the machines placed on each host.
//! * machines: `Pending` -> scheduler -> `Creating` -> driver start ->
//!   `Ready`; delete requests -> `Deleting` -> driver stop/delete -> gone;
//!   desired power and reboot counters are applied to `Ready` machines; a
//!   machine annotated [`REBUILD_ANNOTATION`] has its root disk replaced on
//!   its host, see [`Reconciler::rebuild`]; machines on an unreachable host
//!   get a `HostReachable=false` condition.
//!   A machine annotated [`WAIT_VOLUME_ANNOTATION`] is left unplaced and
//!   unstarted. [`NODE_PIN_ANNOTATION`] limits placement to that host.

mod apply_power;
mod bind_port_hosts;
mod converge;
mod copy_network;
mod driver_for;
mod host_stale_after;
mod hosts;
mod machines;
mod modify;
mod new;
mod nonempty_annotation;
mod note_network;
mod overlay_due;
mod overlay_failed;
mod overlay_retry;
mod overlay_sent;
mod overlay_sent_map;
mod patch;
mod pinned_hosts;
mod rebuild;
mod reconcile_hosts;
mod reconcile_once;
mod reconciler;
mod run;
mod run_loop;
mod sizes;
mod start;
mod sync_overlay;
mod teardown;
mod tick;
mod volume_wait;
mod waits_for;
mod with_queue;
pub(crate) use copy_network::copy_network;
pub use host_stale_after::HOST_STALE_AFTER;
pub(crate) use note_network::note_network;
pub(crate) use overlay_due::overlay_due;
pub(crate) use overlay_failed::overlay_failed;
pub use overlay_retry::OVERLAY_RETRY;
pub(crate) use overlay_sent::OverlaySent;
pub(crate) use pinned_hosts::pinned_hosts;
pub use reconciler::Reconciler;
pub use tick::TICK;
pub(crate) use volume_wait::volume_wait;
pub(crate) use waits_for::waits_for;
#[cfg(test)]
mod overlay_retry_tests;
#[cfg(test)]
mod pin_tests;
