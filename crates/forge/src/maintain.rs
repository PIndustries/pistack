// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Host maintenance: getting machines off a host.
//!
//! [`drain`] is for a host that is up. It live-migrates what can be
//! live-migrated and reports the rest. It never stops a machine.

use crate::agent_driver::begin_migration;
use crate::scheduler::{self, Allocations, Strategy};
use crate::types::{
    delete_requested, usable_underlay, Host, Machine, MachineKind, PowerState, Size,
    NODE_PIN_ANNOTATION,
};
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{kind, ApiResult, Id, Phase, Resources};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The host capability that can live-migrate a guest. The agent's migrate
/// command is `virsh migrate`, whatever else the host runs.
const LIVE_MIGRATION_DRIVER: &str = "libvirt";

/// A machine that is on its way to another host.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Moving {
    pub id: Id,
    pub name: String,
    /// Destination host id.
    pub to: Id,
    /// Destination host name. Empty when the host record is gone.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub to_name: String,
}

/// A machine that was left where it is, and why.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Left {
    pub id: Id,
    pub name: String,
    pub reason: String,
}

/// What `POST /hosts/{id}/drain` answers.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrainReport {
    pub host: Host,
    /// Live migrations this call started, and ones still running from an
    /// earlier call. A machine leaves this list when its new host reports it.
    pub migrating: Vec<Moving>,
    /// Machines still on the host that this call did not move.
    pub not_moved: Vec<Left>,
}

/// Sizes by name and by id, as `spec.size` may hold either.
pub(crate) fn size_lookup(store: &Store) -> ApiResult<impl Fn(&Machine) -> Option<Resources>> {
    let sizes: Vec<Size> = store.collection::<Size>().list(None)?;
    Ok(move |m: &Machine| {
        sizes
            .iter()
            .find(|s| s.meta.name == m.spec.size || s.meta.id.to_string() == m.spec.size)
            .map(|s| s.spec.resources)
    })
}

/// The host a machine is being live-migrated to, read off the status
/// `begin_migration` wrote.
fn migrating_to(m: &Machine) -> Option<&str> {
    if m.status.phase != Phase::Updating {
        return None;
    }
    m.status.message.strip_prefix("migrating ")
}

fn kind_word(kind: MachineKind) -> &'static str {
    match kind {
        MachineKind::Vm => "virtual machine",
        MachineKind::Container => "container",
        MachineKind::Metal => "bare metal machine",
    }
}

/// Start a live migration for every machine on `host` that can have one.
///
/// Safe to call again: a machine already migrating is reported and not
/// touched, and the room it will take on its destination is counted before
/// anything else is placed.
pub fn drain(
    store: &Store,
    queue: Option<&CommandQueue>,
    strategy: Strategy,
    host: &Host,
) -> ApiResult<(Vec<Moving>, Vec<Left>)> {
    let machines = store.collection::<Machine>().list(None)?;
    let hosts = store.collection::<Host>().list(None)?;
    let size_of = size_lookup(store)?;
    let names: BTreeMap<String, String> = hosts
        .iter()
        .map(|h| (h.meta.id.to_string(), h.meta.name.clone()))
        .collect();

    // Other hosts a guest can be migrated to. The scheduler applies the
    // rest: ready, enabled, selector, zone, room.
    let dests: Vec<Host> = hosts
        .iter()
        .filter(|h| h.meta.id != host.meta.id)
        .filter(|h| h.spec.drivers.iter().any(|d| d == LIVE_MIGRATION_DRIVER))
        .filter(|h| usable_underlay(h.spec.underlay_ip.as_deref()).is_some())
        .cloned()
        .collect();

    // Names on each host for the affinity hints, and the room in-flight
    // migrations will take. `status.free` does not count those yet, because
    // the machine stays on its source until the destination reports it.
    let mut alloc = Allocations::new();
    for m in &machines {
        if let Some(h) = &m.status.host {
            alloc.add(h, &m.meta.name, Resources::default());
        }
        if let Some(dest) = migrating_to(m) {
            if let Some(h) = hosts.iter().find(|h| h.meta.id.to_string() == dest) {
                alloc.add(&h.meta.id, &m.meta.name, size_of(m).unwrap_or_default());
            }
        }
    }

    let can_migrate = host.spec.drivers.iter().any(|d| d == LIVE_MIGRATION_DRIVER);
    let mut moving = Vec::new();
    let mut left = Vec::new();
    let mut here: Vec<&Machine> = machines
        .iter()
        .filter(|m| m.status.host.as_ref() == Some(&host.meta.id))
        .collect();
    here.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));

    for m in here {
        let mut leave = |reason: String| {
            left.push(Left {
                id: m.meta.id.clone(),
                name: m.meta.name.clone(),
                reason,
            })
        };
        if let Some(dest) = migrating_to(m) {
            match Id::parse(kind::HOST, dest) {
                Some(to) => moving.push(Moving {
                    id: m.meta.id.clone(),
                    name: m.meta.name.clone(),
                    to_name: names.get(dest).cloned().unwrap_or_default(),
                    to,
                }),
                None => leave(format!("is {}", m.status.message)),
            }
            continue;
        }
        if delete_requested(m) {
            leave("is being deleted".into());
            continue;
        }
        if m.meta
            .annotations
            .get(NODE_PIN_ANNOTATION)
            .is_some_and(|v| !v.is_empty())
        {
            leave("is a pod pinned to this host by its volumes".into());
            continue;
        }
        if m.spec.kind != MachineKind::Vm {
            leave(format!(
                "is a {}; only a virtual machine can be live-migrated",
                kind_word(m.spec.kind)
            ));
            continue;
        }
        if !can_migrate {
            leave(format!(
                "the host has no {LIVE_MIGRATION_DRIVER} runtime, which live migration needs"
            ));
            continue;
        }
        if m.status.phase != Phase::Ready {
            let mut reason = format!(
                "is {}; only a ready machine can be live-migrated",
                phase_word(m.status.phase)
            );
            if !m.status.message.is_empty() {
                reason.push_str(&format!(" ({})", m.status.message));
            }
            leave(reason);
            continue;
        }
        if m.status.power != PowerState::On {
            leave("is powered off; only a running machine can be live-migrated".into());
            continue;
        }
        let Some(queue) = queue else {
            leave("live migration needs the agent transport".into());
            continue;
        };
        let Some(size) = size_of(m) else {
            leave(format!("size '{}' not found", m.spec.size));
            continue;
        };
        let dest = match scheduler::place_networked(&m.spec, &size, &dests, &alloc, strategy) {
            Ok(id) => id,
            Err(e) => {
                leave(e.message);
                continue;
            }
        };
        let Some(dest) = dests.iter().find(|h| h.meta.id == dest) else {
            continue;
        };
        match begin_migration(store, queue, m, dest) {
            Ok(_) => {
                alloc.add(&dest.meta.id, &m.meta.name, size);
                moving.push(Moving {
                    id: m.meta.id.clone(),
                    name: m.meta.name.clone(),
                    to: dest.meta.id.clone(),
                    to_name: dest.meta.name.clone(),
                });
            }
            Err(e) => leave(e.message),
        }
    }
    Ok((moving, left))
}

fn phase_word(phase: Phase) -> String {
    format!("{phase:?}").to_lowercase()
}
