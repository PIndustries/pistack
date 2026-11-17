// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The reconciler: turns `spec` into provider state and writes `status`.
//!
//! Driven by store events (`handle_event`), a full pass at start
//! (`reconcile_all`) and a periodic retry of anything not `Ready`
//! (`reconcile_pending`). Every status write records
//! `observed_revision = revision + 1`, the revision the write itself
//! produces, so the event that write emits is recognised and skipped.
//!
//! A listener is the unit the provider programs: it carries its pool and
//! the pool's active members. Member and pool changes therefore re-program
//! the listeners that use the pool, and a health-check flip does the same.
//!
//! One file per thing the loop does: `stale` and `store_err` the two small
//! reads it leans on, `event` the per-event reply, `all` and `pending` the
//! two sweeps, then one file per resource pass — `balancer`, `listener`,
//! `pool`, `member` — with the listener's provider program and view in
//! `program_listener` and `listener_view`.

mod all;
mod balancer;
mod event;
mod listener;
mod listener_view;
mod member;
mod pending;
mod pool;
mod program_listener;
mod stale;
mod store_err;

pub(crate) use stale::stale;
pub(crate) use store_err::store_err;
