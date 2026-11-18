// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `compute` API: `/machines`, `/hosts`, `/sizes`, `/keypairs`.
//!
//! Handlers write `spec` and `meta` only. Anything that must happen on a
//! host (start, stop, reboot, delete) is recorded as desired state and the
//! reconciler carries it out, so every mutating call returns quickly with
//! the resource as stored and the client polls `status.phase`.
//! Live migration is the exception: it enqueues on the source node and only
//! then records the destination on `status`, so port binding can follow.
//! Draining a host starts live migrations the same way, and evacuating
//! one resets the status of its machines so they are placed again.
//!
//! Project scoping reads `pistack_types::Caller` from request extensions
//! when the gateway's auth layer attached one. With no `Caller` the request
//! is treated as cloud admin in dev mode and rejected otherwise.

mod agent_certs;
mod app_state;
mod body;
mod check_zone;
mod clean_reason;
mod clear_host_power;
mod console;
mod console_ticket;
mod console_vnc;
mod create_keypair;
mod create_machine;
mod create_size;
mod created;
mod delete_keypair;
mod delete_machine;
mod delete_size;
mod disable_host;
mod drain_host;
mod enable_host;
mod evacuate_host;
mod get_host;
mod get_host_power;
mod get_keypair;
mod get_machine;
mod get_size;
mod host_stats;
mod hosts;
mod if_match;
mod index;
mod keypair_created;
mod keypair_public_key;
mod keypair_same;
mod keypairs;
mod list;
mod list_hosts;
mod list_keypairs;
mod list_machines;
mod list_sizes;
mod load_keypair;
mod load_machine;
mod machine_stats;
mod machines;
mod migrate_body;
mod migrate_machine;
mod optional_body;
mod paginate;
mod percent_decode;
mod power_action;
mod power_view;
mod reboot_machine;
mod rebuild_body;
mod rebuild_machine;
mod refuse_volumes;
mod resize_body;
mod resize_machine;
mod resolve_size;
mod resources_of;
mod room;
mod router;
mod scope;
mod serde_urlencoded_lite;
mod set_disabled;
mod set_host_labels;
mod set_host_power;
mod shown;
mod sizes;
mod start_machine;
mod stop_machine;
#[cfg(test)]
mod tests;
mod update_machine;
mod update_size;
mod validate_label;
mod validate_name;
mod validate_resources;
pub use app_state::AppState;
pub use body::Body;
pub use create_keypair::CreateKeypair;
pub use create_machine::CreateMachine;
pub use create_size::CreateSize;
pub use disable_host::DisableHost;
pub use keypair_created::KeypairCreated;
pub use list::List;
#[cfg(test)]
pub(crate) use paginate::paginate;
pub use router::router;
pub use scope::Scope;
#[cfg(test)]
pub(crate) use serde_urlencoded_lite::serde_urlencoded_lite;
pub use set_host_labels::SetHostLabels;
pub use set_host_power::SetHostPower;
pub use update_machine::UpdateMachine;
#[cfg(test)]
pub(crate) use validate_name::validate_name;
