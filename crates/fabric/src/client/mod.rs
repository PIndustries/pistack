// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Client`: the one place the network model is written.
//!
//! The HTTP handlers, the reconciler and other modules (`forge`, `loom`)
//! all go through here. Every method validates, allocates (IPAM) and writes
//! `spec`; nothing here touches the backend or `status`. That is the
//! reconciler's job (`reconcile.rs`), which runs on the events these writes
//! emit. The exceptions are documented on `create_port_for_device`.
//!
//! One file per function: each `Client` method in a file of its own, holding
//! its own `impl Client` block, and the free checks under `functions`. A file
//! is a module, so the method files sit here rather than in a subfolder;
//! an `impl` block is only read in the type's own module or an ancestor of it.
//! Everything is re-exported below, so the paths other modules use are the
//! ones they always used.
mod accessors;
mod add_rule;
mod associate_floating_ip;
mod associate_to_machine;
mod client_struct;
mod create_floating_ip;
mod create_gateway_port;
mod create_network;
mod create_port;
mod create_port_as_router;
mod create_port_for_device;
mod create_port_for_device_with;
mod create_router;
mod create_security_group;
mod create_subnet;
mod default_security_group;
mod default_sg;
mod delete_floating_ip;
mod delete_network;
mod delete_port;
mod delete_port_unchecked;
mod delete_router;
mod delete_security_group;
mod delete_subnet;
mod ensure_default_template;
mod ensure_machine_port;
mod external_network;
pub mod functions;
mod macs_on_network;
mod only_auto_fip_network;
mod port_ref;
mod port_row;
mod ports_of_device;
mod ports_on_network;
mod release_device_ports;
mod remove_rule;
mod router_row;
mod set_router_interfaces;
mod subnet_containing;
mod subnets_of_network;
mod update_floating_ip;
mod update_network;
mod update_port;
mod update_router;
mod update_security_group;
mod update_subnet;
mod used_addresses;
mod validate_fip_target;
mod validate_fixed_ips;
mod validate_rules;
mod validate_security_groups;
mod vpc_of_router;
pub use client_struct::Client;
pub use default_sg::DEFAULT_SG;
pub use functions::*;
pub(crate) use port_ref::PortRef;
