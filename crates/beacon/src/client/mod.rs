// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Client`: the one place zones and record sets are written.
//!
//! HTTP handlers, the auto-record task and other modules (`loom`, `relay`)
//! go through here. Every method validates and writes `spec`; the serial
//! reconciler (`reconcile_zone`) is the only writer of `status`.

mod check_same_project;
mod client_type;
mod content_digest;
mod require_primary;
mod secondary_pending_msg;
mod set_name;
mod set_shape;
mod store_err;
#[cfg(test)]
mod tests;
pub use client_type::Client;
pub use secondary_pending_msg::SECONDARY_PENDING_MSG;
