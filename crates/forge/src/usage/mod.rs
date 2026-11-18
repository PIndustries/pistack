// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Compute quotas: `machines`, `vcpus`, `memory_mib` and `disk_gib` per
//! project. No project, or no limit for a resource, means unlimited. A
//! disabled project rejects a create.
//!
//! Usage is counted from the machine rows at each admission: every machine
//! of the project whose delete has not been accepted, with the resources of
//! the size it names. A size that no longer exists counts as nothing. The
//! count, the check and the write of the row run under gate's admission
//! lock, so two creates cannot both take the last unit.
//!
//! A pod's machine is counted like any other, but the pod controller
//! stores it without asking here, so a pod is not refused.

mod count;
mod create;
mod mirror;
mod note_delete;
mod resize;
#[cfg(test)]
mod tests;
mod usage_type;
#[cfg(test)]
pub(crate) use count::count;
pub use create::create;
pub use note_delete::note_delete;
pub use resize::resize;
