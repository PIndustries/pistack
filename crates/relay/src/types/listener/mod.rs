// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `Listener` resource: its spec and status bound to the `lsn` kind.

mod protocol;
mod spec;
mod status;
use crate::types::kinds::LISTENER;
use pistack_types::{Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

pub use protocol::ListenerProtocol;
pub use spec::ListenerSpec;
pub use status::ListenerStatus;

resource!(
    /// A protocol and port on a balancer's VIP.
    Listener,
    ListenerSpec,
    ListenerStatus,
    LISTENER
);
