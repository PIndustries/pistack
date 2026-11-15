// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The checks, as free functions: no store, only the values handed them.
//!
//! Re-exported from the client module so callers keep writing
//! `client::validate_routes(..)`.
mod check_same_project;
pub mod default_rules;
mod is_router_device;
mod overlaps;
mod owned_by_vpc;
mod prefix_len;
mod store_err;
mod validate_network;
mod validate_pairs;
pub(crate) mod validate_routes;
pub(crate) use check_same_project::check_same_project;
pub use default_rules::default_rules;
pub(crate) use is_router_device::is_router_device;
pub(crate) use overlaps::overlaps;
pub(crate) use owned_by_vpc::owned_by_vpc;
pub(crate) use prefix_len::prefix_len;
pub(crate) use store_err::store_err;
pub(crate) use validate_network::validate_network;
pub(crate) use validate_pairs::validate_pairs;
pub(crate) use validate_routes::validate_routes;
