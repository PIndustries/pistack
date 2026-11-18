// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Read just enough of a slab volume to know which host can attach it.
//! forge does not depend on slab; this view ignores every field it does
//! not need.
//!
//! slab records `status.home_node` on a local file or an LVM volume: the
//! node whose disk holds the bytes. Such a volume attaches there and
//! nowhere else, so a machine that names one in `spec.near_volumes` is
//! placed on that node or not at all. An RBD volume has no home node and does
//! not limit placement. A name that matches no volume is left to the
//! attach to report.

mod held;
mod hosts_near;
#[cfg(test)]
mod tests;
mod volume_spec_view;
mod volume_status_view;
pub use held::held;
#[cfg(test)]
pub(crate) use held::Held;
pub use hosts_near::hosts_near;
#[cfg(test)]
pub(crate) use volume_spec_view::VolumeSpecView;
#[cfg(test)]
pub(crate) use volume_status_view::VolumeStatusView;
