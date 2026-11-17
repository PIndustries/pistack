// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A call the in-memory provider recorded, for tests to assert on.

use pistack_types::Id;
use std::net::IpAddr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    EnsureBalancer {
        balancer: Id,
        vip: IpAddr,
    },
    DeleteBalancer(Id),
    EnsureListener {
        balancer: Id,
        listener: Id,
        port: u16,
        pool: Option<Id>,
        /// `address:port` of every member that should take traffic.
        members: Vec<String>,
    },
    DeleteListener {
        balancer: Id,
        listener: Id,
    },
}
