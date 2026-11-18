// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The seam between the Service controller and whatever makes a cluster IP
//! actually route: L4/L7 balancers via `relay`, or nothing at all in dev mode.

mod delete_labeled;
mod delete_unnamed;
mod endpoint_addrs;
mod ensure_forward;
mod held;
mod holding;
mod ingress_cert;
mod ingress_label;
mod ingress_plan;
mod listener_answers;
mod member_slug;
mod node_port_address;
mod node_port_address_env;
mod noop_realizer;
mod one_backend;
mod port_matches;
mod projects_match;
mod relay_realizer;
mod route_plan;
mod service_label;
mod service_realizer;
mod sync_members;
#[cfg(test)]
mod tests;
mod vip_network;
pub use node_port_address_env::NODE_PORT_ADDRESS_ENV;
pub use noop_realizer::NoopRealizer;
pub use relay_realizer::RelayRealizer;
pub use service_realizer::ServiceRealizer;
