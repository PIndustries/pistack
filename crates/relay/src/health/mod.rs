// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Active health checking.
//!
//! One task probes every member of every pool that has a `health_monitor`,
//! on that monitor's interval, and flips `status.operating_status` with
//! rise/fall hysteresis: `fall` consecutive failures take a member
//! `Online -> Offline`, `rise` consecutive successes bring it back. A flip
//! touches the pool so the reconciler re-programs the listeners using it.
//!
//! Probes: `Tcp` is a connect; `Http` is a `GET path` over a fresh
//! connection whose status line must match `expected_codes`; `Https` is a
//! connect only until a TLS client is wired in (noted in the member
//! message). Every probe is bounded by the monitor's `timeout`.
//!
//! Members administratively down are left `Draining` and not probed; pools
//! without a monitor leave their members `NoMonitor`.
//!
//! One file per thing the checker does: [`Probe`] is the result a probe
//! returns, [`probe`] the bounded call that produces one and
//! [`probe_inner`] the connection it bounds, [`apply`] the hysteresis that
//! folds a result into a member's status, [`Checker`] the thing holding
//! probe times across ticks with its [`tick`](Checker::tick) pass, and
//! [`start`] the task that ticks it.

mod apply;
mod checker;
mod probe;
mod probe_inner;
mod start;
mod tick;

pub use apply::apply;
pub use checker::Checker;
pub use probe::probe;
pub use probe::Probe;
pub use start::start;

#[cfg(test)]
mod tests;
