// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The bridge to real hosts over `pistack-rpc`.
//!
//! * [`RpcDriver`] is a [`HostDriver`] that turns every call into a command
//!   on the [`CommandQueue`] for one node and awaits the agent's result.
//! * [`Registrar`] listens to the queue's agent events: a hello creates or
//!   refreshes the `Host` resource for that node and registers an
//!   `RpcDriver` for it; a heartbeat bumps `last_seen` and folds the agent's
//!   machine reports (addresses, power, probe streaks) into machine status.
//!   The agent keeps no desired state of its own, so the heartbeat is also
//!   where a host that reports a machine evacuated off it is told to delete
//!   that copy.
//!
//! Wire shapes are `pistack_rpc::proto::{MachineRequest, MachineRef,
//! RunningMachine}`, shared with `anvil`.

mod apply_report;
#[cfg(test)]
mod apply_report_tests;
mod begin_migration;
#[cfg(test)]
mod begin_migration_tests;
mod boot_image_problem;
mod boot_image_size_problem;
mod call;
mod catalog_image;
mod catalog_image_use;
mod catalog_named;
mod consts;
mod delete;
mod delete_stale_copy;
#[cfg(test)]
mod delete_stale_copy_tests;
mod evacuated_off;
mod for_node;
mod handle;
mod host_for_node;
mod kind_to_wire;
mod list;
mod machine_key;
mod migrate_dest;
mod new;
mod node;
mod on_heartbeat;
#[cfg(test)]
mod on_heartbeat_tests;
mod on_hello;
#[cfg(test)]
mod on_hello_tests;
mod power_from_wire;
mod rebuild;
mod rebuild_key;
mod register;
mod registrar;
mod request_for;
#[cfg(test)]
mod request_for_tests;
mod resolve_boot_image;
mod resources_for;
mod resync;
mod rpc_driver;
mod run;
#[cfg(test)]
mod run_tests;
mod runtime_from_wire;
mod start;
#[cfg(test)]
mod start_tests;
mod stop;
#[cfg(test)]
mod stop_tests;
#[cfg(test)]
#[cfg(test)]
pub(crate) mod test_support;
mod with_gauge;
mod with_store;
pub(crate) use begin_migration::begin_migration;
pub(crate) use boot_image_problem::boot_image_problem;
pub(crate) use boot_image_size_problem::boot_image_size_problem;
pub use consts::AGENT_LABEL;
pub use machine_key::machine_key;
pub use registrar::Registrar;
pub use rpc_driver::RpcDriver;
