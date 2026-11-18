// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The handlers `crud!` generates, gathered for the router.
//!
//! Each `crud!` invocation lives in the file of the kind it serves and emits
//! five handlers named after it. They are `pub(crate)` so this module can
//! reach them; nothing outside `api` uses them. Like [`super::crud_deps`],
//! this is a view only — the handlers come from the macro.

pub(crate) use super::floating_ip::{create_fip, delete_fip, get_fip, list_fips, update_fip};
pub(crate) use super::network::{
    create_network, delete_network, get_network, list_networks, update_network,
};
pub(crate) use super::port::{create_port, delete_port, get_port, list_ports, update_port};
pub(crate) use super::routers::{
    create_router, delete_router, get_router, list_routers, update_router,
};
pub(crate) use super::security_group::{create_sg, delete_sg, get_sg, list_sgs, update_sg};
pub(crate) use super::subnet::{
    create_subnet, delete_subnet, get_subnet, list_subnets, update_subnet,
};
