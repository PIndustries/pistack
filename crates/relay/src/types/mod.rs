// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The load-balancer resource model: `Balancer`, `Listener`, `Pool`,
//! `Member`.
//!
//! A balancer owns a VIP. Listeners bind a protocol and port on it and
//! point at a default pool. A pool holds members and a health monitor.
//! `spec` is the request. `status` is what the provider reported.
//!
//! A module is a directory and each file inside it is one item, named for
//! the item alone. `balancer/` holds `spec`, `status`, `provider` and the
//! `resource!` call in `mod.rs`; `listener/`, `pool/` and `member/` the
//! same. `route/` and `health_monitor/` hold the rule and the check a
//! resource carries. `kinds` the id constants, `resource` the macro, and
//! the bare `default_*` / `root_path` files the serde defaults shared
//! across more than one resource.

// Declares the `resource!` macro every resource expands.
#[macro_use]
mod resource;

mod balancer;
mod default_true;
mod default_weight;
mod health_monitor;
pub mod kinds;
mod listener;
mod member;
mod pool;
mod root_path;
mod route;

pub use balancer::Balancer;
pub use balancer::BalancerSpec;
pub use balancer::BalancerStatus;
pub use balancer::Provider;
pub use health_monitor::HealthMonitor;
pub use health_monitor::MonitorType;
pub use kinds::KINDS;
pub use kinds::MANAGED_LABEL;
pub use listener::Listener;
pub use listener::ListenerProtocol;
pub use listener::ListenerSpec;
pub use listener::ListenerStatus;
pub use member::Member;
pub use member::MemberSpec;
pub use member::MemberStatus;
pub use member::OperatingStatus;
pub use pool::Algorithm;
pub use pool::Pool;
pub use pool::PoolProtocol;
pub use pool::PoolSpec;
pub use pool::PoolStatus;
pub use pool::SessionPersistence;
pub use route::PathMatch;
pub use route::Route;
