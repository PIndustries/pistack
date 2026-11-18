// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use pistack_store::StoreError;
use pistack_types::Id;

use super::reconciler::Reconciler;

impl Reconciler {
    /// Apply `f` to the whole machine and write it back only if anything
    /// changed. The rebuild request is an annotation, so ending it touches
    /// `meta` as well as `status`.
    pub(super) async fn modify(
        &self,
        id: &Id,
        f: impl Fn(&mut Machine),
    ) -> Result<Machine, StoreError> {
        let col = self.machines();
        for _ in 0..8 {
            let cur = col.get(id)?;
            let mut next = cur.clone();
            f(&mut next);
            if next.meta == cur.meta && next.status == cur.status {
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
