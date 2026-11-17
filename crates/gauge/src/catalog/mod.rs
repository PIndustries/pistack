// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The metrics PiStack records itself: their names, what kind each is and
//! what it means. The names and labels are part of the interface; dashboards
//! and alert rules are written against them, so they do not change.
//!
//! Host series carry `host`. Machine series carry `machine` (the id),
//! `name`, `project` (the id) and `host`; disk series add `device` and
//! network series add `interface`. Cluster series carry `server`, the id of
//! the server that reports them.

mod builders;
mod info;
mod metric_info;
mod metric_kind;
mod names;
mod reserved_prefix;
mod table;
#[cfg(test)]
mod tests;

pub use info::info;
pub use metric_info::MetricInfo;
pub use metric_kind::MetricKind;
pub use names::*;
pub use reserved_prefix::RESERVED_PREFIX;
pub use table::CATALOG;
