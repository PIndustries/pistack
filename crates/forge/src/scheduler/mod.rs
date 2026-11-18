// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Placement. One pure function: filters, then weighs, then picks.
//!
//! Filters (hard): host is `Ready` and not disabled; it supports the machine kind; its labels
//! satisfy `spec.host_selector` and, when the machine names one, its `zone`
//! label is `spec.zone`; it has room for `size` after subtracting
//! `allocations` made earlier in the same pass.
//!
//! Weights (soft): affinity/anti-affinity hints first, then free memory
//! (most free wins under `Strategy::Spread`, least free that still fits
//! wins under `Strategy::Pack`), then free vcpus, then host id for a
//! deterministic tie-break.

mod allocations;
mod place;
mod place_networked;
mod place_on_ovn;
mod place_on_segment;
mod rejections;
mod score;
mod strategy;
#[cfg(test)]
mod tests;
pub use allocations::Allocations;
pub use place::place;
pub(crate) use place_networked::place_networked;
pub use place_on_ovn::place_on_ovn;
pub use place_on_segment::place_on_segment;
pub use strategy::Strategy;
