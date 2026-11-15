// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::backend::NetBackend;
use crate::client::PortRef;
use pistack_store::Store;
use pistack_types::Id;
use std::collections::HashMap;
use std::sync::Arc;
#[derive(Clone)]
pub struct Client {
    pub(in crate::client) store: Store,
    pub(in crate::client) backend: Arc<dyn NetBackend>,
    /// Last-seen shape of each port the reconciler realised, so a Deleted
    /// event (which carries only the id) can still refresh the subnet's
    /// allocation count and the port's security groups.
    pub(crate) port_index: Arc<parking_lot::Mutex<HashMap<Id, PortRef>>>,
}
