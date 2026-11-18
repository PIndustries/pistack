// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use pistack_store::StoreError;
use pistack_types::Phase;

use super::reconciler::Reconciler;

impl Reconciler {
    pub(super) async fn teardown(&self, m: Machine) -> anyhow::Result<()> {
        let m = if m.status.phase != Phase::Deleting {
            self.patch(&m.meta.id, |s| {
                s.phase = Phase::Deleting;
                s.message.clear();
            })
            .await?
        } else {
            m
        };
        if m.status.host.is_some() {
            let (hid, driver) = match self.driver_for(&m) {
                Ok(x) => x,
                Err(e) => {
                    let msg = e.message.clone();
                    self.patch(&m.meta.id, |s| s.message = msg.clone()).await?;
                    return Ok(());
                }
            };
            if let Err(e) = driver.stop(&m.meta.id).await {
                if e.code != pistack_types::error::ErrorCode::NotFound {
                    tracing::debug!(machine = %m.meta.id, error = %e, "forge: stop before delete failed");
                }
            }
            if let Err(e) = driver.delete(&m.meta.id).await {
                let msg = e.message.clone();
                tracing::warn!(machine = %m.meta.id, host = %hid, error = %e, "forge: driver delete failed");
                self.patch(&m.meta.id, |s| s.message = msg.clone()).await?;
                return Ok(());
            }
        }
        if let Err(e) = crate::nic::release_ports(&self.store, &m.meta.id) {
            tracing::warn!(machine = %m.meta.id, error = %e, "forge: port release failed");
        }
        match self.machines().delete(&m.meta.id) {
            Ok(_) => {
                tracing::info!(machine = %m.meta.id, name = %m.meta.name, "forge: machine deleted")
            }
            Err(StoreError::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
}
