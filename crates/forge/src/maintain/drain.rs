// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::agent_driver::begin_migration;
use crate::scheduler::{self, Allocations, Strategy};
use crate::types::{
    delete_requested, usable_underlay, Host, Machine, MachineKind, PowerState, NODE_PIN_ANNOTATION,
};
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{kind, ApiResult, Id, Phase, Resources};
use std::collections::BTreeMap;

use super::can_live_migrate::can_live_migrate;
use super::disk_holds_machine::disk_holds_machine;
use super::kind_word::kind_word;
use super::left::Left;
use super::live_migration_driver::LIVE_MIGRATION_DRIVER;
use super::migrating_to::migrating_to;
use super::moving::Moving;
use super::no_destination::no_destination;
use super::phase_word::phase_word;
use super::size_lookup::size_lookup;

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
    // rest: ready, enabled, selector, zone, room. `others` is every other
    // host, so a report can say when none of them can take a migration at
    // all rather than that placement found nothing.
    let others: Vec<&Host> = hosts.iter().filter(|h| h.meta.id != host.meta.id).collect();
    let dests: Vec<Host> = others
        .iter()
        .filter(|h| can_live_migrate(h))
        .filter(|h| usable_underlay(h.spec.underlay_ip.as_deref()).is_some())
        .map(|h| (*h).clone())
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

    let can_migrate = can_live_migrate(host);
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
        if let Some(reason) = disk_holds_machine(store, m, host) {
            leave(reason);
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
            Err(_) => {
                leave(no_destination(
                    host,
                    &others,
                    &dests.iter().collect::<Vec<_>>(),
                    size,
                    &alloc,
                ));
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
