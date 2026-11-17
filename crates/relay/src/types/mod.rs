// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The load-balancer resource model: `Balancer`, `Listener`, `Pool`,
//! `Member`.
//!
//! A balancer owns a VIP. Listeners bind a protocol and port on it and
//! point at a default pool. A pool holds members and a health monitor.
//! `spec` is the request. `status` is what the provider reported.
//!
//! One file per resource and per shape, so the file names are the map:
//! `kinds` the id kinds and constants relay owns, `resource` the macro that
//! gives each of them its wire shape, then the four resources in
//! `balancer`, `listener`, `pool`, `member`. Each of those holds its spec,
//! its status, and the `resource!` call that binds them, because a spec
//! apart from its resource does not read. `route` is the HTTP rule a
//! listener carries and `health_monitor` the check a pool carries, each
//! named for the thing it describes.

// Declares the `resource!` macro every resource below expands.
#[macro_use]
mod resource;

mod balancer;
mod health_monitor;
pub mod kinds;
mod listener;
mod member;
mod pool;
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
