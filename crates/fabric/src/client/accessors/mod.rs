// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The constructor and the accessors.
//!
//! These are the files that read `Client`'s fields; every other method goes
//! through the accessors. The two fields they need are visible across `client`
//! so this folder can hold them, which is still short of the crate.
mod backend;
mod floating_ips;
mod networks;
mod new;
mod ports;
mod routers;
mod security_groups;
mod store;
mod subnets;
mod vpcs;
