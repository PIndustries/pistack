// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Machine, MachineStatus};
use pistack_store::StoreError;
use pistack_types::Id;

use super::reconciler::Reconciler;

impl Reconciler {
    /// Apply `f` to the machine's status and write it back only if it
    /// changed. Returns the stored machine.
    pub(super) async fn patch(
        &self,
        id: &Id,
        f: impl Fn(&mut MachineStatus),
    ) -> Result<Machine, StoreError> {
        let col = self.machines();
        for _ in 0..8 {
            let cur = col.get(id)?;
            let mut next = cur.clone();
            f(&mut next.status);
            if next.status == cur.status {
                return Ok(cur);
            }
            match col.update(next, Some(cur.meta.revision)) {
                Ok(m) => return Ok(m),
                Err(StoreError::Revision { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(StoreError::Conflict(format!(
            "machine {id}: too many concurrent writers"
        )))
    }
}
