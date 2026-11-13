// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Host maintenance: getting machines off a host.
//!
//! [`drain`] is for a host that is up. It live-migrates what can be
//! live-migrated and reports the rest. It never stops a machine.
//!
//! [`evacuate`] is for a host that is gone. Nothing can be copied off it,
//! so a machine is handed back to the scheduler and starts somewhere else
//! from its image. Its root disk, which was a file on the dead host, is
//! not there. Volumes on shared storage are plugged back in by slab;
//! a machine with a volume that lived on the dead host is left alone.
//! The agent of a host that returns is told to delete the copies it still
//! holds; see `Registrar::delete_stale_copy`.

use crate::agent_driver::begin_migration;
use crate::scheduler::{self, Allocations, Strategy};
use crate::types::{
    delete_requested, usable_underlay, Host, Machine, MachineKind, PowerState, Size,
    EVACUATED_FROM_ANNOTATION, NODE_PIN_ANNOTATION,
};
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{kind, ApiError, ApiResult, Id, Phase, Resource, Resources, Spec};
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

// ---- evacuate ----------------------------------------------------------------

/// Volume backends whose data is not on the host that attaches it.
const SHARED_BACKENDS: &[&str] = &["rbd"];

/// Body of `POST /hosts/{id}/evacuate`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct EvacuateOptions {
    /// The operator says a disabled host that may still be reporting is
    /// dead. Not needed for a host that is already down.
    #[serde(default)]
    pub confirm_dead: bool,
    /// Accept that each machine starts again from its image without the
    /// root disk it had. No machine is evacuated without it.
    #[serde(default)]
    pub rebuild_from_image: bool,
}

/// A machine that was handed back to the scheduler.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Evacuated {
    pub id: Id,
    pub name: String,
    /// The image it starts from again. Its root disk stayed on the host.
    pub rebuilt_from: String,
    /// Shared volumes that are plugged back in once it runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,
}

/// What `POST /hosts/{id}/evacuate` answers.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EvacuateReport {
    pub host: Host,
    pub evacuated: Vec<Evacuated>,
    /// Machines left on the host's record, and why.
    pub skipped: Vec<Left>,
}

// slab's rows, read as views so forge does not depend on slab. They ignore
// every field they do not need and are never written back.

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct VolumeSpecView {
    #[serde(default)]
    volume_type: String,
}

impl Spec for VolumeSpecView {
    const KIND: &'static str = kind::VOLUME;
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct VolumeStatusView {
    #[serde(default)]
    attached_to: Option<Id>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct VolumeTypeSpecView {
    #[serde(default)]
    backend: String,
}

impl Spec for VolumeTypeSpecView {
    const KIND: &'static str = "voltype";
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct NoStatus {}

/// A host may be evacuated when it has stopped reporting, or when an
/// operator disabled it and says it is dead. Anything else risks starting
/// a second copy of a machine that is still running.
fn may_evacuate(host: &Host, confirm_dead: bool) -> ApiResult<()> {
    if host.status.phase == Phase::Error {
        return Ok(());
    }
    if host.spec.disabled && confirm_dead {
        return Ok(());
    }
    let name = &host.meta.name;
    Err(ApiError::conflict(if host.spec.disabled {
        format!(
            "host {name} is disabled but still reporting. Its machines may be running, and \
             evacuating them would start second copies. If the host is dead, confirm it \
             (confirm_dead, `pi host evacuate --confirm-dead`)"
        )
    } else {
        format!(
            "host {name} is up. Evacuate only acts on a host that is down, or on a disabled \
             host you confirm is dead. Use drain to empty a working host"
        )
    }))
}

/// Hand the machines of a dead host back to the scheduler.
///
/// Safe to call again: a machine that was evacuated is no longer on the
/// host, so a second call only sees what the first one skipped.
pub fn evacuate(
    store: &Store,
    host: &Host,
    opts: EvacuateOptions,
) -> ApiResult<(Vec<Evacuated>, Vec<Left>)> {
    may_evacuate(host, opts.confirm_dead)?;
    let col = store.collection::<Machine>();
    let mut here: Vec<Machine> = col
        .list(None)?
        .into_iter()
        .filter(|m| m.status.host.as_ref() == Some(&host.meta.id))
        .collect();
    here.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
    let volumes = store
        .collection::<Resource<VolumeSpecView, VolumeStatusView>>()
        .list(None)?;
    let types = store
        .collection::<Resource<VolumeTypeSpecView, NoStatus>>()
        .list(None)?;
    let backend_of = |volume_type: &str| {
        types
            .iter()
            .find(|t| t.meta.name == volume_type || t.meta.id.to_string() == volume_type)
            .map(|t| t.spec.backend.trim().to_owned())
    };

    let from = host.meta.id.to_string();
    let mut evacuated = Vec::new();
    let mut skipped = Vec::new();
    for m in here {
        let mut skip = |reason: String| {
            skipped.push(Left {
                id: m.meta.id.clone(),
                name: m.meta.name.clone(),
                reason,
            })
        };
        if delete_requested(&m) {
            skip("is being deleted".into());
            continue;
        }
        if migrating_to(&m).is_some() {
            skip("is being live-migrated; wait for that to end".into());
            continue;
        }
        if m.spec.kind == MachineKind::Metal {
            skip("is a bare metal machine; it is the hardware and cannot start elsewhere".into());
            continue;
        }
        if m.meta
            .annotations
            .get(NODE_PIN_ANNOTATION)
            .is_some_and(|v| !v.is_empty())
        {
            skip("is a pod pinned to this host by its volumes".into());
            continue;
        }
        let attached: Vec<_> = volumes
            .iter()
            .filter(|v| v.status.attached_to.as_ref() == Some(&m.meta.id))
            .collect();
        // A volume whose type is unknown is treated as local: nothing says
        // its data is anywhere but on the dead host.
        let stuck = attached.iter().find_map(|v| {
            let backend = backend_of(&v.spec.volume_type);
            match backend.as_deref() {
                Some(b) if SHARED_BACKENDS.contains(&b) => None,
                Some("") | None => Some((v, "local".to_owned())),
                Some(b) => Some((v, b.to_owned())),
            }
        });
        if let Some((v, backend)) = stuck {
            skip(format!(
                "volume {} is on {backend} storage on this host and cannot follow the machine",
                v.meta.name
            ));
            continue;
        }
        if !opts.rebuild_from_image {
            skip(format!(
                "its root disk is on this host. To start it elsewhere from image {} without \
                 that disk, ask for it (rebuild_from_image, `pi host evacuate --rebuild-from-image`)",
                m.spec.image
            ));
            continue;
        }
        let note = format!(
            "moved off host {} ({}); the root disk stayed there and the machine starts again from image {}",
            host.meta.name, host.meta.id, m.spec.image
        );
        let mut done = false;
        col.modify(&m.meta.id, |x| {
            // Placed elsewhere or deleted since the list was read.
            if x.status.host.as_ref() != Some(&host.meta.id) || delete_requested(x) {
                return Ok(());
            }
            x.meta
                .annotations
                .insert(EVACUATED_FROM_ANNOTATION.into(), from.clone());
            let st = &mut x.status;
            st.host = None;
            st.phase = Phase::Pending;
            st.addresses.clear();
            st.power = PowerState::Unknown;
            st.probes.clear();
            st.message = format!("evacuated from host {}", host.meta.name);
            st.conditions.retain(|c| c.kind != "HostReachable");
            st.set_condition("Evacuated", true, "RebuiltFromImage", &note);
            done = true;
            Ok(())
        })?;
        if !done {
            skip("changed while the host was being evacuated; run it again".into());
            continue;
        }
        tracing::warn!(
            machine = %m.meta.id,
            name = %m.meta.name,
            host = %host.meta.id,
            image = %m.spec.image,
            "forge: machine evacuated; it starts from its image without its old root disk"
        );
        evacuated.push(Evacuated {
            id: m.meta.id.clone(),
            name: m.meta.name.clone(),
            rebuilt_from: m.spec.image.clone(),
            volumes: attached.iter().map(|v| v.meta.name.clone()).collect(),
        });
    }
    Ok((evacuated, skipped))
}
