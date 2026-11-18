// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::in_zone;
use hickory_proto::rr::Name;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use super::zone_data::ZoneData;

/// In-memory secondary zones filled by AXFR pull. Shared by the transfer task
/// and the authoritative handler.
#[derive(Clone, Default)]
pub struct SecondaryCache {
    pub(crate) inner: Arc<RwLock<HashMap<String, ZoneData>>>,
}

impl SecondaryCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert(&self, data: ZoneData) {
        let key = data.apex.to_ascii();
        self.inner.write().insert(key, data);
    }

    pub fn remove(&self, apex: &str) {
        self.inner.write().remove(apex);
    }

    pub fn retain_apices(&self, keep: &std::collections::HashSet<String>) {
        self.inner.write().retain(|k, _| keep.contains(k));
    }

    pub(crate) fn find(&self, name: &Name) -> Option<ZoneData> {
        let n = name.to_ascii();
        self.inner
            .read()
            .values()
            .filter(|z| in_zone(&z.apex.to_ascii(), &n))
            .max_by_key(|z| z.apex.num_labels())
            .cloned()
    }

    pub(crate) fn find_apex(&self, name: &Name) -> Option<ZoneData> {
        self.inner.read().get(&name.to_ascii()).cloned()
    }
}
