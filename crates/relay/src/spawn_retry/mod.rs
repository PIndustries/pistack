// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Retrying the start of a helper program that may still be being written.
//!
//! [`spawn_retrying_busy`] and [`status_retrying_busy`] are the two retry
//! calls, over [`SPAWN_ATTEMPTS`] tries spaced [`SPAWN_RETRY_DELAY`] apart.
//! Callers see the two functions here, as before.

mod spawn_attempts;
mod spawn_retry_delay;
mod spawn_retrying_busy;
mod status_retrying_busy;

pub(crate) use spawn_retrying_busy::spawn_retrying_busy;
pub(crate) use status_retrying_busy::status_retrying_busy;

#[cfg(test)]
mod tests;
