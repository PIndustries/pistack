// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{FloatingIp, FloatingIpSpec, Network};
use crate::Client;
use pistack_store::StoreError;
use pistack_types::{kind, ApiError, ApiResult, Condition, Id, Meta};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;

use super::auto_fip_label::AUTO_FIP_LABEL;
use super::fip_condition::FIP_CONDITION;
use super::is_auto::is_auto;
use super::label_choice::label_choice;
use super::labeled_machine::labeled_machine;
use super::list_machines::list_machines;
use super::listed_machine::ListedMachine;
use super::machine_label::MACHINE_LABEL;
use super::machine_view::MachineView;
use super::port_covers::port_covers;
use super::reason_of::reason_of;

impl Client {
    /// One floating IP per machine that has an IPv4, drawn from the network
    /// marked `auto_floating_ip`, and none left behind for a machine that
    /// is gone or has opted out.
    ///
    /// Idempotent. With no marked network nothing new is created; labelled
    /// floating IPs that already exist stay until their machine is deleted
    /// or opts out. The floating IP is created in the machine's project, named
    /// after the machine (`{name}-fip` when that name is taken), and counts
    /// against the project's `floating_ips` quota like any other. A refusal
    /// leaves the machine as it is and says why in its [`FIP_CONDITION`]
    /// condition. A failed associate deletes the floating IP just created
    /// so the pool does not leak; the next pass retries. Unlabeled floating
    /// IPs are left alone.
    pub fn sync_auto_floating_ips(&self) -> ApiResult<()> {
        let machines = list_machines(self.store())?;
        let live: HashSet<Id> = machines.iter().map(|m| m.meta.id.clone()).collect();
        let network = self.auto_fip_network()?;
        let fips = self.floating_ips().list(None)?;
        let ports = self.ports().list(None)?;
        let mut project_choice: HashMap<Id, Option<bool>> = HashMap::new();
        let mut opted_out: HashSet<Id> = HashSet::new();

        for machine in &machines {
            let wanted = label_choice(&machine.meta).or_else(|| {
                let project = machine.meta.project.as_ref()?;
                *project_choice
                    .entry(project.clone())
                    .or_insert_with(|| self.project_choice(project))
            });
            if wanted == Some(false) {
                opted_out.insert(machine.meta.id.clone());
                self.note(machine, None);
                continue;
            }
            let Some(addr) = machine.ipv4 else {
                self.note(machine, None);
                continue;
            };
            if fips
                .iter()
                .any(|f| port_covers(f, &ports, &machine.meta.id, addr))
            {
                self.note(machine, None);
                continue;
            }
            if !self.address_has_a_home(&machine.meta, addr, &ports) {
                // Associate would fail with "no subnet contains" every pass,
                // and a new floating IP would be created and rolled back each
                // time. Wait until a subnet covers the address.
                tracing::debug!(
                    machine = %machine.meta.id,
                    address = %addr,
                    "fabric auto-fip: no subnet contains the guest address; skipped"
                );
                continue;
            }
            if let Some(existing) = fips
                .iter()
                .find(|f| is_auto(f) && labeled_machine(f).as_ref() == Some(&machine.meta.id))
            {
                // Created earlier, not pointing at the guest yet. Retry
                // rather than allocating a second address.
                match self.associate_to_machine(&existing.meta.id, &machine.meta.id) {
                    Ok(_) => {
                        self.note(machine, None);
                    }
                    Err(e) => self.refused(machine, "associate", &e),
                }
                continue;
            }
            let Some(net) = &network else {
                self.note(machine, None);
                continue;
            };
            let Some(name) = self.auto_fip_name(&machine.meta) else {
                continue;
            };
            let mut meta = Meta::new(kind::FLOATING_IP, name, machine.meta.project.clone());
            meta.labels
                .insert(AUTO_FIP_LABEL.to_owned(), "true".to_owned());
            meta.labels
                .insert(MACHINE_LABEL.to_owned(), machine.meta.id.to_string());
            let created = match self.create_floating_ip(
                meta,
                FloatingIpSpec {
                    network: net.meta.id.clone(),
                    address: None,
                    port: None,
                },
            ) {
                Ok(fip) => fip,
                Err(e) => {
                    self.refused(machine, "create", &e);
                    continue;
                }
            };
            match self.associate_to_machine(&created.meta.id, &machine.meta.id) {
                Ok(_) => {
                    self.note(machine, None);
                }
                Err(e) => {
                    self.refused(machine, "associate", &e);
                    if let Err(e) = self.delete_floating_ip(&created.meta.id) {
                        tracing::warn!(
                            floating_ip = %created.meta.id,
                            error = %e,
                            "fabric auto-fip: rollback failed"
                        );
                    }
                }
            }
        }
        self.reap_auto_floating_ips(&live, &opted_out)
    }

    /// How many labelled floating IPs exist while no network is marked.
    /// Before the mark existed a network named `fleet`, or the only
    /// external one, handed them out; after an upgrade they are still
    /// there and nothing says so unless this is logged.
    pub fn auto_fips_without_a_network(&self) -> ApiResult<usize> {
        if self.auto_fip_network()?.is_some() {
            return Ok(0);
        }
        Ok(self
            .floating_ips()
            .list(None)?
            .iter()
            .filter(|f| is_auto(f))
            .count())
    }

    /// The project's own [`AUTO_FIP_LABEL`]. A project fabric cannot read
    /// has no say.
    pub(crate) fn project_choice(&self, project: &Id) -> Option<bool> {
        let row = self
            .store()
            .collection::<gate::Project>()
            .try_get(project)
            .ok()??;
        label_choice(&row.meta)
    }

    /// Record a refusal on the machine. Logged loudly the first time and
    /// when the reason changes, quietly on the passes in between.
    pub(crate) fn refused(&self, machine: &ListedMachine, step: &str, e: &ApiError) {
        let changed = self.note(machine, Some((reason_of(e), &e.message)));
        if changed {
            tracing::warn!(
                machine = %machine.meta.id,
                error = %e,
                "fabric auto-fip: {step} failed"
            );
        } else {
            tracing::debug!(
                machine = %machine.meta.id,
                error = %e,
                "fabric auto-fip: {step} still failing"
            );
        }
    }

    /// Set the machine's [`FIP_CONDITION`] to `problem`, or drop it for
    /// `None`. Writes only when it differs from what the machine carries.
    /// True when it wrote.
    pub(crate) fn note(&self, machine: &ListedMachine, problem: Option<(&str, &str)>) -> bool {
        let have = machine
            .noted
            .as_ref()
            .map(|(r, m)| (r.as_str(), m.as_str()));
        if have == problem {
            return false;
        }
        let wrote = self
            .store()
            .collection::<MachineView>()
            .modify(&machine.meta.id, |row| {
                row.status.conditions.retain(|c| c.kind != FIP_CONDITION);
                if let Some((reason, message)) = problem {
                    row.status.conditions.push(Condition {
                        kind: FIP_CONDITION.to_owned(),
                        status: false,
                        reason: reason.to_owned(),
                        message: message.to_owned(),
                        last_transition: chrono::Utc::now(),
                    });
                }
                Ok(())
            });
        match wrote {
            Ok(_) => true,
            Err(StoreError::NotFound(_)) => false,
            Err(e) => {
                tracing::warn!(
                    machine = %machine.meta.id,
                    error = %e,
                    "fabric auto-fip: condition was not recorded"
                );
                false
            }
        }
    }

    /// True when associate can find or make a port for `addr`: a port in the
    /// machine's project already holds it, or a subnet the machine may use
    /// contains it. A store error counts as true so associate reports it.
    pub(crate) fn address_has_a_home(
        &self,
        machine: &Meta,
        addr: IpAddr,
        ports: &[crate::types::Port],
    ) -> bool {
        if ports
            .iter()
            .any(|p| p.meta.project == machine.project && p.spec.fixed_ips.contains(&addr))
        {
            return true;
        }
        self.subnet_containing(machine, addr)
            .map(|s| s.is_some())
            .unwrap_or(true)
    }

    /// The external network marked `auto_floating_ip`. The API refuses a
    /// second one; should two exist anyway, the lower id is used.
    pub(crate) fn auto_fip_network(&self) -> ApiResult<Option<Network>> {
        let mut marked: Vec<Network> = self
            .networks()
            .list(None)?
            .into_iter()
            .filter(|n| n.spec.external && n.spec.auto_floating_ip)
            .collect();
        marked.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
        Ok(marked.into_iter().next())
    }

    pub(crate) fn auto_fip_name(&self, machine: &Meta) -> Option<String> {
        let project = machine.project.as_ref();
        if self.fip_name_free(project, &machine.name) {
            return Some(machine.name.clone());
        }
        let alt = format!("{}-fip", machine.name);
        if self.fip_name_free(project, &alt) {
            return Some(alt);
        }
        tracing::warn!(
            machine = %machine.id,
            name = %machine.name,
            "fabric auto-fip: name taken"
        );
        None
    }

    pub(crate) fn fip_name_free(&self, project: Option<&Id>, name: &str) -> bool {
        matches!(
            self.floating_ips().get_by_name(project, name),
            Err(StoreError::NotFound(_))
        )
    }

    pub(crate) fn machine_of_auto_fip(&self, fip: &FloatingIp) -> Option<Id> {
        if let Some(id) = labeled_machine(fip) {
            return Some(id);
        }
        let pid = fip.spec.port.as_ref()?;
        let port = self.ports().get(pid).ok()?;
        port.spec.device.filter(|d| d.kind == kind::MACHINE)
    }

    /// Drop labelled floating IPs whose machine is gone or has opted out,
    /// and the port they pointed at. A port whose device is a machine that
    /// still exists stays, as does a port some other floating IP still uses.
    pub(crate) fn reap_auto_floating_ips(
        &self,
        live: &HashSet<Id>,
        opted_out: &HashSet<Id>,
    ) -> ApiResult<()> {
        let fips = self.floating_ips().list(None)?;
        for fip in &fips {
            if !is_auto(fip) {
                continue;
            }
            let Some(machine) = self.machine_of_auto_fip(fip) else {
                continue;
            };
            if live.contains(&machine) && !opted_out.contains(&machine) {
                continue;
            }
            let port_id = fip.spec.port.clone();
            if let Err(e) = self.delete_floating_ip(&fip.meta.id) {
                tracing::warn!(
                    floating_ip = %fip.meta.id,
                    error = %e,
                    "fabric auto-fip: delete failed"
                );
                continue;
            }
            let Some(pid) = port_id else {
                continue;
            };
            let delete_port = match self.ports().get(&pid) {
                Ok(port) => !port.spec.device.as_ref().is_some_and(|d| live.contains(d)),
                Err(StoreError::NotFound(_)) => false,
                Err(e) => {
                    tracing::warn!(port = %pid, error = %e, "fabric auto-fip: port lookup failed");
                    continue;
                }
            };
            if !delete_port {
                continue;
            }
            let still_used = self
                .floating_ips()
                .list(None)?
                .iter()
                .any(|other| other.spec.port.as_ref() == Some(&pid));
            if still_used {
                continue;
            }
            if let Err(e) = self.delete_port(&pid) {
                tracing::warn!(port = %pid, error = %e, "fabric auto-fip: port delete failed");
            }
        }
        Ok(())
    }
}
