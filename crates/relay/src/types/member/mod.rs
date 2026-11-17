// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `Member` resource: its spec and status bound to the `mbr` kind.

mod operating_status;
mod spec;
mod status;
use crate::types::kinds::MEMBER;
use pistack_types::{Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

pub use operating_status::OperatingStatus;
pub use spec::MemberSpec;
pub use status::MemberStatus;

resource!(
    /// One backend address:port in a pool.
    Member,
    MemberSpec,
    MemberStatus,
    MEMBER
);

impl Member {
    pub fn addr(&self) -> std::net::SocketAddr {
        std::net::SocketAddr::new(self.spec.address, self.spec.port)
    }
}
