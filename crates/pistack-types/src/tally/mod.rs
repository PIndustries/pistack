// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Counts a module keeps for the operator. They live here because every
//! module depends on this crate and none of them on the metrics module.
//!
//! The counts are per process and start at zero with it.

mod reconcile_failed;
mod reconcile_failures;
#[cfg(test)]
mod tests;

pub use reconcile_failed::reconcile_failed;
pub use reconcile_failures::reconcile_failures;
