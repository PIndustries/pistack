// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The checker's memory between ticks: when each member was last probed.

use crate::client::Client;
use pistack_types::Id;
use std::collections::HashMap;
use std::time::Instant;

/// The checker's memory between ticks: when each member was last probed.
pub struct Checker {
    pub(super) client: Client,
    pub(super) last: HashMap<Id, Instant>,
}

impl Checker {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            last: HashMap::new(),
        }
    }
}
