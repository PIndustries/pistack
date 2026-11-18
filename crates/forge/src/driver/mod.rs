// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The seam between the scheduler/reconciler and whatever actually runs a
//! machine on a host.
//!
//! * [`HostDriver`] is the trait the reconciler calls. One implementation per
//!   host, looked up in the [`DriverRegistry`].
//! * [`MockDriver`] is an in-memory implementation with deterministic
//!   addresses (`10.0.0.N`). Dev mode and tests use it.
//! * [`ChannelDriver`] turns every call into a [`DriverRequest`] on an
//!   in-process mpsc channel. Whoever owns the receiver (an `anvil` runtime
//!   in the same process, or the `pistack-rpc` transport later) answers via
//!   the oneshot in the request. [`serve_driver`] is the trivial pump that
//!   answers requests with any other `HostDriver`.

mod channel_driver;
mod driver_command;
mod driver_registry;
mod driver_reply;
mod driver_request;
mod entry;
mod host_driver;
mod machine_runtime;
mod mock_driver;
mod mock_state;
mod serve_driver;
#[cfg(test)]
mod tests;
mod unexpected;
pub use channel_driver::ChannelDriver;
pub use driver_command::DriverCommand;
pub use driver_registry::DriverRegistry;
pub use driver_reply::DriverReply;
pub use driver_request::DriverRequest;
pub use host_driver::HostDriver;
pub use machine_runtime::MachineRuntime;
pub use mock_driver::MockDriver;
pub use serve_driver::serve_driver;
