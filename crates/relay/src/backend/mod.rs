// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The relay module's backend choice: which providers it mounts.
//!
//! [`RelayBackend`] is the enum and the four environment reads that pick
//! one. Callers see [`RelayBackend`] here, as before.

mod relay_backend;

pub use relay_backend::RelayBackend;

#[cfg(test)]
mod tests;
