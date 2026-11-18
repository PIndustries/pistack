// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Machine, MachineStatus};
use pistack_types::Phase;

use super::note_network;
use super::reconciler::Reconciler;
use super::volume_wait;
use super::waits_for;

impl Reconciler {
    pub(super) async fn start(&self, m: Machine) -> anyhow::Result<()> {
        // Loom holds the machine here until the slab volume has a host path.
        // Starting earlier creates the container before the bind source exists.
        if let Some(msg) = volume_wait(&m) {
            let msg = msg.to_string();
            self.patch(&m.meta.id, |s| {
                s.message = msg.clone();
                s.set_condition("Running", false, "VolumeNotAttached", &msg);
            })
            .await?;
            return Ok(());
        }
        let (hid, driver) = match self.driver_for(&m) {
            Ok(x) => x,
            Err(e) => {
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| s.message = msg.clone()).await?;
                return Ok(());
            }
        };
        if let Err(e) = crate::nic::ensure_ports(&self.store, &m).await {
            tracing::warn!(machine = %m.meta.id, error = %e, "forge: port was not allocated");
            let msg = e.message.clone();
            self.patch(&m.meta.id, |s| {
                s.phase = Phase::Error;
                s.message = msg.clone();
                s.set_condition("Running", false, "StartFailed", &msg);
            })
            .await?;
            return Ok(());
        }
        let started = driver.start(&m).await;
        // The call can take as long as a command timeout. If the machine was
        // evacuated off this host meanwhile, the answer is about a copy that
        // no longer counts and must not be written over the new placement.
        let here = |s: &MachineStatus| s.host.as_ref() == Some(&hid);
        match started {
            Ok(rt) => {
                tracing::info!(machine = %m.meta.id, name = %m.meta.name, host = %hid, addrs = ?rt.addresses, "forge: machine ready");
                let network_message = rt.message.clone();
                self.patch(&m.meta.id, |s| {
                    if !here(s) {
                        return;
                    }
                    s.phase = Phase::Ready;
                    s.addresses = rt.addresses.clone();
                    s.power = rt.power;
                    s.message.clear();
                    s.set_condition("Running", true, "Started", "");
                    note_network(s, &network_message);
                })
                .await?;
            }
            Err(e) if waits_for(&e) == Some("Segment") => {
                tracing::info!(machine = %m.meta.id, error = %e, "forge: waiting for the network segment");
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| {
                    if !here(s) {
                        return;
                    }
                    s.message = msg.clone();
                    s.set_condition("Scheduled", true, "Segment", &msg);
                })
                .await?;
            }
            Err(e) if waits_for(&e) == Some("PortBinding") => {
                // Fabric writes the port to OVN on its own pass. The machine
                // stays in creating and the next pass starts it.
                tracing::info!(machine = %m.meta.id, error = %e, "forge: waiting for the port binding");
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| {
                    if !here(s) {
                        return;
                    }
                    s.message = msg.clone();
                    s.set_condition("Scheduled", true, "PortBinding", &msg);
                })
                .await?;
            }
            Err(e) => {
                tracing::warn!(machine = %m.meta.id, host = %hid, error = %e, "forge: driver start failed");
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| {
                    if !here(s) {
                        return;
                    }
                    s.phase = Phase::Error;
                    s.message = msg.clone();
                    s.set_condition("Running", false, "StartFailed", &msg);
                })
                .await?;
            }
        }
        Ok(())
    }
}
