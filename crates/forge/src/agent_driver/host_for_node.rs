// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use crate::types::Host;
impl Registrar {
    pub(super) fn host_for_node(&self, node: &str) -> anyhow::Result<Option<Host>> {
        let col = self.store.collection::<Host>();
        Ok(col
            .list(None)?
            .into_iter()
            .find(|h| h.spec.node_name == node))
    }
}
