// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `Pool` resource: its spec and status bound to the `pool` kind.

mod algorithm;
mod protocol;
mod session_persistence;
mod spec;
mod status;
use crate::types::kinds::POOL;
use pistack_types::{Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

pub use algorithm::Algorithm;
pub use protocol::PoolProtocol;
pub use session_persistence::SessionPersistence;
pub use spec::PoolSpec;
pub use status::PoolStatus;

resource!(
    /// A set of members with a balancing algorithm and a health monitor.
    Pool,
    PoolSpec,
    PoolStatus,
    POOL
);
