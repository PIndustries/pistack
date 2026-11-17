// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The seam between the balancer model and whatever realises it.
//!
//! `relay` owns the resources and decides *what* should exist; an
//! [`LbProvider`] makes it so. Every call is idempotent ("ensure"), takes
//! the fully resolved picture (a listener with its pool and that pool's
//! members already looked up) and returns only what the model cannot know
//! by itself (the VIP that was allocated, a provider-side handle). The
//! reconciler is the only caller.
//!
//! Implementations: [`MockProvider`] for tests and single-node development,
//! [`ForwardProvider`] (in-process L4 TCP and UDP), [`RelayL7Provider`]
//! (in-process HTTP, and TLS when `keep` already holds the certificate),
//! and [`OvnProvider`] (unimplemented).

pub mod forward;
pub mod lb_provider;
pub mod listener_view;
pub mod mock;
pub mod ovn;
pub mod providers;
pub mod realized;
pub mod relay_l7;
pub mod route_view;

pub use forward::ForwardProvider;
pub use lb_provider::LbProvider;
pub use listener_view::ListenerView;
pub use mock::{Call, MockProvider};
pub use ovn::OvnProvider;
pub use providers::Providers;
pub use realized::Realized;
pub use relay_l7::RelayL7Provider;
pub use route_view::RouteView;
