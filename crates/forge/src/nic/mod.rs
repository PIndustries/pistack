// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Allocate a fabric port before a guest boots, and describe that port to
//! the host. An empty `spec.networks` leaves the shared bridge alone.

mod attachments;
mod ensure_ports;
mod fabric_client;
mod gateway_and_prefix;
mod is_routed;
mod named;
mod nic_from;
mod release_ports;
mod resolve_groups;
mod resolve_network;
#[cfg(test)]
mod tests;
mod wants_port;
pub use attachments::attachments;
pub use ensure_ports::ensure_ports;
#[cfg(test)]
pub(crate) use fabric_client::fabric_client;
pub use release_ports::release_ports;
pub use wants_port::wants_port;
