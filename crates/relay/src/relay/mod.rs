// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The relay module and the interval its reconciler retries on.
//!
//! [`Relay`] is what a server mounts; [`RETRY_INTERVAL`] is how often its
//! reconciler retries what is not ready. Callers see [`Relay`] here, as
//! before.

mod relay_module;
mod retry_interval;

pub use relay_module::Relay;
