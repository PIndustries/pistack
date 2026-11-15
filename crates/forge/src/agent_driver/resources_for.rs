// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::rpc_driver::RpcDriver;
use crate::types::Machine;
use crate::types::Size;
use pistack_types::Resources;
impl RpcDriver {
    pub(super) fn resources_for(&self, m: &Machine) -> Resources {
        let Some(store) = &self.store else {
            return Resources::default();
        };
        store
            .collection::<Size>()
            .resolve(None, &m.spec.size)
            .map(|s| s.spec.resources)
            .unwrap_or_default()
    }
}
