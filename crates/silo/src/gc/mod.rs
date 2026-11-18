// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Sweep for abandoned multipart uploads.
//!
//! A client that creates a multipart upload and never completes or aborts
//! it leaves a staging dir under `{root}/{bucket}/.uploads/{uploadId}/`.
//! [`sweep_expired_uploads`] removes every staging dir whose initiation time
//! is older than the TTL (`[modules.silo] multipart_ttl_secs`, default 7
//! days). Age comes from `meta.json`'s `initiated` field, falling back to the
//! `meta.json` mtime, then the dir mtime, so uploads staged before that
//! field existed (or whose create crashed before `meta.json` was written)
//! are still collected.
//!
//! The sweep is a plain function over a [`Client`] and an explicit `now` so
//! tests drive it without a clock. [`run`] is the background loop the module
//! starts while this node is the cluster leader: it sweeps once immediately
//! (startup, or promotion) and then every [`sweep_interval`].
//!
//! Like S3 lifecycle expiry, the TTL counts from initiation, not from the
//! last part upload.

mod bucket_dirs;
mod max_sweep_interval;
mod run;
mod sweep_expired_uploads;
mod sweep_interval;
mod sweep_report;
#[cfg(test)]
mod tests;
pub use max_sweep_interval::MAX_SWEEP_INTERVAL;
pub use run::run;
pub use sweep_expired_uploads::sweep_expired_uploads;
pub use sweep_interval::sweep_interval;
pub use sweep_report::SweepReport;
