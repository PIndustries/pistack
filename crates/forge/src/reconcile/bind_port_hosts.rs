// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::reconciler::Reconciler;

impl Reconciler {
    /// Set `status.host` on each fabric port plugged into a placed machine.
    /// A port that already names that host is not written again.
    pub(super) fn bind_port_hosts(&self) -> anyhow::Result<()> {
        let machines = self.machines().list(None)?;
        let ports = self.store.collection::<fabric::Port>();
        let listed = ports.list(None)?;
        for machine in machines {
            let Some(host) = machine.status.host.as_ref() else {
                continue;
            };
            let recorded = host.to_string();
            for port in &listed {
                if port.spec.device.as_ref() != Some(&machine.meta.id) {
                    continue;
                }
                if port.status.host.as_deref() == Some(recorded.as_str()) {
                    continue;
                }
                let next = recorded.clone();
                match ports.modify(&port.meta.id, |p| {
                    p.status.host = Some(next.clone());
                    Ok(())
                }) {
                    Ok(_) => {}
                    Err(pistack_store::StoreError::NotFound(_)) => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }
}
