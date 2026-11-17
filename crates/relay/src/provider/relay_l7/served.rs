// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What one listener's connections serve from: the handles taken off its
//! slot when the accept task starts.

use super::slot::{Serve, Slot};
use super::table::Table;
use crate::picker::Picker;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

/// What one HTTP listener hands to every accepted connection.
pub(super) struct Served {
    pub(super) members: Arc<parking_lot::Mutex<Table>>,
    pub(super) picker: Picker,
    pub(super) conns: Arc<parking_lot::Mutex<HashMap<SocketAddr, u64>>>,
    pub(super) limit: Arc<AtomicUsize>,
    pub(super) inflight: Arc<AtomicUsize>,
    pub(super) mode: Arc<parking_lot::Mutex<Serve>>,
}

impl Served {
    /// The handles a listener's connections serve from, taken off the slot
    /// the moment its accept task starts.
    pub(super) fn for_view(slot: &Slot) -> Arc<Self> {
        Arc::new(Served {
            members: slot.members.clone(),
            picker: slot.picker.clone(),
            conns: slot.conns.clone(),
            limit: slot.limit.clone(),
            inflight: slot.inflight.clone(),
            mode: slot.mode.clone(),
        })
    }
}
