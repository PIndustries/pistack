// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! VPCs: a routed private address space inside one project.
//!
//! A VPC holds one or more address blocks and owns one router, made with it
//! and deleted with it. A network that names a VPC belongs to it; a subnet on
//! such a network sits inside one of the VPC's blocks, overlaps no other
//! subnet of that VPC, and is joined to the VPC's router as part of its own
//! create. A network with no `vpc` is untouched by any of this.
//!
//! Two counts matter here. A VPC is counted against its own `vpcs` limit and
//! never against `routers`, which is why its router is written outside the
//! quota path; and the VPC's subnet count lives in its status, kept current
//! whenever one of them appears or goes.

mod align;
mod block_at;
mod carve;
mod cidr;
mod cidrs_required;
mod client;
mod first_free;
mod index;
mod inside;
mod joined;
mod meets;
mod next_block;
mod overlaps;
mod owner_name;
mod router_address;
mod same_project;
mod span;
mod store_err;
mod unusable;
mod validate_cidrs;
pub(crate) use carve::carve;
pub(crate) use cidrs_required::cidrs_required;
pub(crate) use owner_name::owner_name;
