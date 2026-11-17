// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The contract between `pistack-core` and every module.
//!
//! A module is a library crate that exposes:
//! * `info()` – name, API prefix, version,
//! * `router()` – an axum `Router` mounted at `/{prefix}` by the gateway,
//! * `start()` – background work (reconcilers, pollers) run once at boot.
//!
//! Modules never bind ports or open stores themselves; `ModuleContext`
//! hands them everything. That is what makes "one binary, many modules" work.
//!
//! Controllers and reconcile loops must honour [`Leadership`]: they stay idle
//! on a follower, run while this node is the leader, and stop when leadership
//! is lost. API writes are already refused on followers; this keeps in-process
//! writers from mutating a local copy.

pub use tokio_util::sync::CancellationToken;

mod leadership;
mod module_context;
mod module_info;
mod module_trait;
#[cfg(test)]
mod tests;

pub use leadership::Leadership;
pub use module_context::ModuleContext;
pub use module_info::ModuleInfo;
pub use module_trait::Module;
