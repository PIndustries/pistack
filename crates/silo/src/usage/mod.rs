// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Bucket quota: `buckets` per project. No project, no limit, or a project
//! gate has no row for, means unlimited. A disabled project refuses.
//!
//! The count is the project's bucket rows, taken under gate's admission
//! lock. There is no limit on object bytes: silo keeps no running total
//! per bucket or project, so the only way to know the usage is to read
//! every object record, and a put is too frequent for that.

mod admitted;
mod count;
mod released;
#[cfg(test)]
mod tests;
pub(crate) use admitted::admitted;
pub use count::count;
pub(crate) use released::released;
