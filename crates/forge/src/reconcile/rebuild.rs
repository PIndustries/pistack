// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{DesiredPower, Machine, PowerState, REBUILD_ANNOTATION};
use pistack_types::Phase;

use super::note_network;
use super::reconciler::Reconciler;

impl Reconciler {
    /// Replace the machine's root disk on its host and start it again with
    /// the power it wants. The old disk is gone by the time the host
    /// answers, whether the new one was written or not, so any failure ends
    /// the request in `Error` with the host's message and the machine left
    /// off. The request lives on [`REBUILD_ANNOTATION`] until it ends.
    pub(super) async fn rebuild(&self, m: Machine) -> anyhow::Result<()> {
        let Some(driver) = m.status.host.as_ref().and_then(|hid| self.drivers.get(hid)) else {
            // Nothing to rebuild until the machine has a host again.
            return Ok(());
        };
        if let Err(e) = crate::nic::ensure_ports(&self.store, &m).await {
            tracing::warn!(machine = %m.meta.id, error = %e, "forge: port was not allocated");
            let msg = e.message.clone();
            self.modify(&m.meta.id, |x| {
                x.status.phase = Phase::Error;
                x.status.message = msg.clone();
                x.status
                    .set_condition("Running", false, "RebuildFailed", &msg);
                x.meta.annotations.remove(REBUILD_ANNOTATION);
            })
            .await?;
            return Ok(());
        }
        match driver.rebuild(&m).await {
            Ok(rt) => {
                tracing::info!(machine = %m.meta.id, name = %m.meta.name, addrs = ?rt.addresses, "forge: machine rebuilt");
                let network_message = rt.message.clone();
                let want_off = m.spec.power == DesiredPower::Off;
                // The host answers with what it observes; until its next
                // heartbeat says otherwise the machine keeps the addresses
                // its ports gave it.
                let addresses = if rt.addresses.is_empty() {
                    m.status.addresses.clone()
                } else {
                    rt.addresses.clone()
                };
                self.modify(&m.meta.id, |x| {
                    x.status.phase = Phase::Ready;
                    x.status.addresses = addresses.clone();
                    x.status.power = rt.power;
                    x.status.message.clear();
                    x.status
                        .set_condition("Running", rt.power == PowerState::On, "Rebuilt", "");
                    note_network(&mut x.status, &network_message);
                    x.meta.annotations.remove(REBUILD_ANNOTATION);
                })
                .await?;
                // A stopped machine is rebuilt to its image and left off.
                if want_off && rt.power != PowerState::Off {
                    match driver.stop(&m.meta.id).await {
                        Ok(()) => {
                            self.patch(&m.meta.id, |s| s.power = PowerState::Off)
                                .await?;
                        }
                        Err(e) => {
                            tracing::warn!(machine = %m.meta.id, error = %e, "forge: stop after rebuild failed");
                            let msg = e.message.clone();
                            self.patch(&m.meta.id, |s| {
                                s.power = PowerState::Unknown;
                                s.message = msg.clone();
                            })
                            .await?;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(machine = %m.meta.id, error = %e, "forge: driver rebuild failed");
                // The host keeps the old disk when it cannot write the new
                // one, so the machine is still what it was. Say the request
                // failed; do not file the machine away. A rebuild may be
                // asked for again, and the machine must stay startable.
                let msg = e.message.clone();
                self.modify(&m.meta.id, |x| {
                    x.status.phase = Phase::Ready;
                    x.status.message = msg.clone();
                    x.status.set_condition(
                        "Running",
                        m.status.power == PowerState::On,
                        "RebuildFailed",
                        &msg,
                    );
                    x.meta.annotations.remove(REBUILD_ANNOTATION);
                })
                .await?;
            }
        }
        Ok(())
    }
}
