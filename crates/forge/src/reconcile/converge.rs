// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::scheduler::{self, Allocations};
use crate::types::{rebuild_requested, Host, Machine};
use pistack_types::{Phase, Resources};

use super::pinned_hosts;
use super::reconciler::Reconciler;
use super::volume_wait;

impl Reconciler {
    pub(super) async fn converge(
        &self,
        mut m: Machine,
        hosts: &[Host],
        alloc: &mut Allocations,
        size_of: &(dyn Fn(&Machine) -> Option<Resources> + Sync),
    ) -> anyhow::Result<()> {
        // Host reachability condition for anything already placed.
        if let Some(hid) = &m.status.host {
            let reachable = hosts
                .iter()
                .any(|h| &h.meta.id == hid && h.status.phase == Phase::Ready);
            let msg = if reachable {
                String::new()
            } else {
                format!("host {hid} is not ready")
            };
            m = self
                .patch(&m.meta.id, |s| {
                    s.set_condition(
                        "HostReachable",
                        reachable,
                        if reachable { "Ready" } else { "HostError" },
                        &msg,
                    );
                })
                .await?;
            if !reachable && m.status.phase != Phase::Pending {
                return Ok(());
            }
        }

        match m.status.phase {
            Phase::Pending => {
                if let Some(msg) = volume_wait(&m) {
                    let msg = msg.to_string();
                    self.patch(&m.meta.id, |s| {
                        s.message = msg.clone();
                        s.set_condition("Running", false, "VolumeNotAttached", &msg);
                    })
                    .await?;
                    return Ok(());
                }
                let Some(size) = size_of(&m) else {
                    self.patch(&m.meta.id, |s| {
                        s.phase = Phase::Error;
                        s.message = format!("size '{}' not found", m.spec.size);
                    })
                    .await?;
                    return Ok(());
                };
                let pinned = pinned_hosts(&m, hosts);
                let candidates = pinned.as_deref().unwrap_or(hosts);
                // A local or LVM volume attaches only on the node that
                // holds it, so the machine goes there or nowhere.
                let held = crate::volumes::held(
                    &self.store,
                    m.meta.project.as_ref(),
                    &m.spec.near_volumes,
                );
                let decided = crate::volumes::hosts_near(&held, candidates).and_then(|near| {
                    let candidates = near.as_deref().unwrap_or(candidates);
                    scheduler::place_networked(&m.spec, &size, candidates, alloc, self.strategy)
                });
                let host = match decided {
                    Ok(h) => h,
                    Err(e) => {
                        let msg = e.message.clone();
                        self.patch(&m.meta.id, |s| {
                            s.set_condition("Scheduled", false, "NoHost", &msg);
                            s.message = msg.clone();
                        })
                        .await?;
                        return Ok(());
                    }
                };
                alloc.add(&host, &m.meta.name, size);
                let m = self
                    .patch(&m.meta.id, |s| {
                        s.host = Some(host.clone());
                        s.phase = Phase::Creating;
                        s.message.clear();
                        s.set_condition("Scheduled", true, "Placed", &format!("placed on {host}"));
                    })
                    .await?;
                if rebuild_requested(&m) {
                    // A rebuild asked for while the machine was unplaced:
                    // it has a host now, so rebuild it in place.
                    let m = self
                        .patch(&m.meta.id, |s| {
                            s.phase = Phase::Updating;
                            s.message = format!("rebuilding from image {}", m.spec.image);
                        })
                        .await?;
                    return self.rebuild(m).await;
                }
                self.start(m).await
            }
            Phase::Creating => self.start(m).await,
            Phase::Ready => {
                if rebuild_requested(&m) {
                    let m = self
                        .patch(&m.meta.id, |s| {
                            s.phase = Phase::Updating;
                            s.message = format!("rebuilding from image {}", m.spec.image);
                        })
                        .await?;
                    return self.rebuild(m).await;
                }
                self.apply_power(m).await
            }
            Phase::Updating => {
                if rebuild_requested(&m) && m.status.host.is_some() {
                    // The flip to `Updating` happened but the pass ended
                    // before the host was asked.
                    return self.rebuild(m).await;
                }
                Ok(())
            }
            Phase::Error => {
                // A reboot request is the user's "try again".
                if m.spec.restarts > m.status.restarts && m.status.host.is_some() {
                    let n = m.spec.restarts;
                    let m = self
                        .patch(&m.meta.id, |s| {
                            s.restarts = n;
                            s.phase = Phase::Creating;
                            s.message.clear();
                        })
                        .await?;
                    return self.start(m).await;
                }
                Ok(())
            }
            Phase::Deleting => Ok(()),
        }
    }
}
