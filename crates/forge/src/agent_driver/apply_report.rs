// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::evacuated_off::evacuated_off;
use super::power_from_wire::power_from_wire;
use super::registrar::Registrar;
use crate::types::Host;
use crate::types::Machine;
use crate::types::PowerState;
use crate::types::EVACUATED_FROM_ANNOTATION;
use pistack_rpc::proto::MachineReport;
use pistack_store::StoreError;
use pistack_types::Phase;
impl Registrar {
    /// reconciler, which owns those phases.
    pub(super) fn apply_report(&self, reporter: &Host, r: &MachineReport) -> anyhow::Result<()> {
        let host = &reporter.meta.id;
        let col = self.store.collection::<Machine>();
        let Some(m) = col.try_get(&r.id)? else {
            return Ok(());
        };
        if m.status.phase == Phase::Updating {
            let dest = m.status.message.strip_prefix("migrating ").unwrap_or("");
            if dest == host.to_string() {
                let power = r.power.map(power_from_wire);
                match col.modify(&m.meta.id, |x| {
                    x.status.host = Some(host.clone());
                    x.status.phase = Phase::Ready;
                    x.status.message.clear();
                    if !r.addresses.is_empty() {
                        x.status.addresses = r.addresses.clone();
                    }
                    if let Some(p) = power {
                        if p != PowerState::Unknown {
                            x.status.power = p;
                        }
                    }
                    x.status.probes = r.probes.clone();
                    Ok(())
                }) {
                    Ok(_) | Err(StoreError::NotFound(_)) => Ok(()),
                    Err(e) => Err(e.into()),
                }
            } else {
                Ok(())
            }
        } else if m.status.host.as_ref() != Some(host) {
            self.delete_stale_copy(reporter, &m);
            Ok(())
        } else if evacuated_off(&m, host) {
            // Placed back on the host it was evacuated from: the copy there
            // is the machine again, so the marker no longer applies.
            match col.modify(&m.meta.id, |x| {
                x.meta.annotations.remove(EVACUATED_FROM_ANNOTATION);
                Ok(())
            }) {
                Ok(_) | Err(StoreError::NotFound(_)) => Ok(()),
                Err(e) => Err(e.into()),
            }
        } else if m.status.phase != Phase::Ready {
            Ok(())
        } else {
            let power = r.power.map(power_from_wire);
            let addrs_differ = !r.addresses.is_empty() && r.addresses != m.status.addresses;
            let power_differs =
                power.is_some_and(|p| p != PowerState::Unknown && p != m.status.power);
            let probes_differ = r.probes != m.status.probes;
            if !addrs_differ && !power_differs && !probes_differ {
                return Ok(());
            }
            match col.modify(&m.meta.id, |x| {
                if addrs_differ {
                    x.status.addresses = r.addresses.clone();
                }
                if let (true, Some(p)) = (power_differs, power) {
                    x.status.power = p;
                }
                if probes_differ {
                    x.status.probes = r.probes.clone();
                }
                Ok(())
            }) {
                Ok(_) | Err(StoreError::NotFound(_)) => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
    }
}
