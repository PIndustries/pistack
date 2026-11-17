// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A connection counted against a member for as long as the request holding
//! it lives.

use super::closed::closed;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

/// One connection counted against a backend until the request holding it
/// is over, whichever way the request ends.
pub(super) struct Held(
    pub(super) Arc<parking_lot::Mutex<HashMap<SocketAddr, u64>>>,
    pub(super) SocketAddr,
);

impl Drop for Held {
    fn drop(&mut self) {
        closed(&self.0, self.1);
    }
}
