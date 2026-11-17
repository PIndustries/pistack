// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! How a listener is published off this host.
//!
//! [`plan`] returns argv. [`apply`] runs that command when the binary is
//! present and returns unavailable when it is not. WireGuard may create
//! only an interface named `pw` plus 12 hex digits, and is unavailable
//! when `wg` is missing. It does not name, configure, or delete `wg-fleet`.
//! Edge enqueues a publish for a host whose driver is `edge` and starts no
//! process. No edge host leaves the listener unavailable.
//!
//! One file per item, so the file names are the map: [`kind`] holds
//! [`DoorKind`] and [`DoorSide`], [`plan`] the [`DoorPlan`] and its argv,
//! [`listen`] and [`upstream`] the argument checks, [`iface`] the
//! WireGuard interface name, [`process`] the running publishers,
//! [`binary`] the lookup that decides whether a publisher is installed,
//! [`wireguard`] the `ip link add` for a `pw` interface, and [`edge`] the
//! publish queue path. Callers see [`DoorKind`], [`DoorPlan`], [`plan`],
//! [`apply`] and [`stop`] at this module, as before.

mod apply;
mod binary;
mod edge;
mod iface;
mod kind;
mod listen;
mod plan;
mod process;
mod upstream;
mod wireguard;

pub use apply::apply;
pub use kind::DoorKind;
pub use kind::DoorSide;
pub use plan::plan;
pub use plan::DoorPlan;
pub use process::stop;

#[cfg(test)]
mod tests;
