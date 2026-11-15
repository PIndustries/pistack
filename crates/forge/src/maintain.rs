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
    delete_requested, machine_disks_are_shared, usable_underlay, Host, Machine, MachineKind,
    PowerState, Size, EVACUATED_FROM_ANNOTATION, NODE_PIN_ANNOTATION,
};
use async_trait::async_trait;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{kind, ApiError, ApiResult, Id, Phase, Resource, Resources, Spec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// The host capability that can live-migrate a guest. The agent's migrate
/// command is `virsh migrate`, whatever else the host runs, and it is
/// reached only when the agent reports this capability.
pub const LIVE_MIGRATION_DRIVER: &str = "libvirt";

/// Volume backends whose data is not on the host that attaches it.
const SHARED_BACKENDS: &[&str] = &["rbd"];

/// Whether `host`'s agent can take part in a live migration, as source or
/// as destination.
pub fn can_live_migrate(host: &Host) -> bool {
    host.spec
        .drivers
        .iter()
        .any(|d| d.trim().eq_ignore_ascii_case(LIVE_MIGRATION_DRIVER))
}

/// Why `machine` is not asked off `host`: the disk that stays behind, said
/// in the words `drain` reports and a migrate call returns. `None` when
/// nothing on this host holds it.
///
/// A root disk is a file on its host, so a host that keeps its disks to
/// itself cannot hand a machine to another. A volume that is not on shared
/// storage holds its machine the same way; a volume whose type forge cannot
/// see is counted as local, because nothing says its data is anywhere else.
/// [`evacuate`] reasons about the same rows.
pub fn disk_holds_machine(store: &Store, machine: &Machine, host: &Host) -> Option<String> {
    if !machine_disks_are_shared(host) {
        return Some(
            "its root disk is a file on this host, and live migration needs storage both hosts \
             share"
                .to_owned(),
        );
    }
    let Ok(volumes) = store.collection::<Volume>().list(None) else {
        return None;
    };
    let attached: Vec<Volume> = volumes
        .into_iter()
        .filter(|v| v.status.attached_to.as_ref() == Some(&machine.meta.id))
        .collect();
    if attached.is_empty() {
        return None;
    }
    let Ok(types) = store.collection::<VolumeType>().list(None) else {
        return Some(format!(
            "its volumes ({}) are on storage this host has not shared, which live migration \
             cannot move",
            volume_names(&attached)
        ));
    };
    attached.iter().find_map(|v| {
        let shared = types.iter().any(|t| {
            (t.meta.name == v.spec.volume_type || t.meta.id.to_string() == v.spec.volume_type)
                && SHARED_BACKENDS.contains(&t.spec.backend.trim())
        });
        (!shared).then(|| {
            format!(
                "volume {} is on storage this host has not shared, which live migration cannot \
                 move",
                v.meta.name
            )
        })
    })
}

fn volume_names(volumes: &[Volume]) -> String {
    let mut names: Vec<&str> = volumes.iter().map(|v| v.meta.name.as_str()).collect();
    names.sort_unstable();
    names.join(", ")
}

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

pub(crate) fn phase_word(phase: Phase) -> String {
    format!("{phase:?}").to_lowercase()
}

/// Why the drain found no destination. The scheduler saw only the hosts
/// that can take a live migration and answers in a tally of the whole fleet;
/// an operator needs host names, so this names one host per reason, in the
/// order the placement checks them.
fn no_destination(
    host: &Host,
    others: &[&Host],
    tried: &[&Host],
    size: Resources,
    alloc: &Allocations,
) -> String {
    if others.is_empty() {
        return format!(
            "host {} is the only host here, so there is nowhere to migrate to",
            host.meta.name
        );
    }
    if tried.is_empty() {
        return format!(
            "none of the {} other {} here can take a live migration",
            others.len(),
            if others.len() == 1 { "host" } else { "hosts" }
        );
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(h) = tried.iter().find(|h| h.status.phase != Phase::Ready) {
        parts.push(format!("{} is not ready", h.meta.name));
    }
    if let Some(h) = tried.iter().find(|h| h.spec.disabled) {
        parts.push(format!("{} is disabled", h.meta.name));
    }
    if let Some(h) = tried
        .iter()
        .find(|h| !size.fits_in(&h.status.free.saturating_sub(&alloc.used_on(&h.meta.id))))
    {
        parts.push(format!(
            "{} has no room for {} vcpus and {} MiB",
            h.meta.name, size.vcpus, size.memory_mib
        ));
    }
    if parts.is_empty() {
        return format!(
            "none of the {} hosts that could take it fits its networks, affinity or \
             host_selector",
            tried.len()
        );
    }
    parts.join(", ")
}

// ---- evacuate ----------------------------------------------------------------

/// How long to wait for a power-off to be confirmed before the evacuation
/// is refused. A host that will not answer its BMC within this is treated
/// as unfenceable, and nothing moves.
pub const FENCE_TIMEOUT: Duration = Duration::from_secs(60);
/// How often a pending power-off is re-read.
const FENCE_POLL: Duration = Duration::from_millis(500);

/// Body of `POST /hosts/{id}/evacuate`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct EvacuateOptions {
    /// The operator says the host is dead. Required every time a host has
    /// no power control, whether or not it is already marked down. Not
    /// needed for a host the server can power off and confirm.
    #[serde(default)]
    pub confirm_dead: bool,
    /// Accept that each machine starts again from its image without the
    /// root disk it had. No machine is evacuated without it.
    #[serde(default)]
    pub rebuild_from_image: bool,
}

/// How the evacuation proved the host is off, so its machines can be
/// started elsewhere without leaving a second copy running.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Proof {
    /// The server cut the host's power through its BMC and read the off
    /// state back.
    PowerOff,
    /// An operator said so (`confirm_dead`), for a host with no power
    /// control the server can drive.
    Operator,
}

impl Proof {
    pub fn as_str(self) -> &'static str {
        match self {
            Proof::PowerOff => "power-off",
            Proof::Operator => "operator",
        }
    }

    /// The proof in the shape an audit `action` takes: lowercase, words
    /// joined with underscores.
    fn audit_tag(self) -> &'static str {
        match self {
            Proof::PowerOff => "power_off",
            Proof::Operator => "operator",
        }
    }
}

/// Cuts a host's power from outside and proves it is off. Reached through
/// a trait so an evacuation can be tested against a fake; the real one
/// drives the host's BMC over IPMI or Redfish.
#[async_trait]
pub trait Fencer: Send + Sync {
    /// Power `host` off and confirm the off state. Errors if the host has
    /// no power control, or is not confirmed off within [`FENCE_TIMEOUT`].
    async fn fence(&self, host: &Host) -> ApiResult<()>;
}

/// The real fencer: drives a host's BMC over IPMI or Redfish with the
/// shared `power` crate, and reads the BMC password from the secret named
/// on the host. The password is read only here, only for one power-off,
/// and is never returned or logged.
pub struct BmcFencer {
    store: Store,
    timeout: Duration,
    poll: Duration,
}

impl BmcFencer {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            timeout: FENCE_TIMEOUT,
            poll: FENCE_POLL,
        }
    }

    /// The BMC password for `host`, read from the secret its power control
    /// names. A host with no power control, or naming a secret that is not
    /// there, cannot be fenced.
    fn password(&self, pc: &crate::types::PowerControl) -> ApiResult<Vec<u8>> {
        let client = keep::Client::open(&self.store)
            .map_err(|_| ApiError::unavailable("the secrets service is unavailable"))?;
        let listed = client
            .list_secrets(None)
            .map_err(|_| ApiError::unavailable("the secrets service is unavailable"))?;
        let Some(secret) = listed
            .into_iter()
            .find(|s| s.meta.name == pc.secret || s.meta.id.to_string() == pc.secret)
        else {
            return Err(ApiError::unavailable(format!(
                "host power control names a secret, {:?}, that is not there",
                pc.secret
            )));
        };
        client
            .get_secret_payload(&secret.meta.id)
            .map_err(|_| ApiError::unavailable("the host's BMC password could not be read"))
    }
}

#[async_trait]
impl Fencer for BmcFencer {
    async fn fence(&self, host: &Host) -> ApiResult<()> {
        let pc = host
            .spec
            .power_control
            .as_ref()
            .ok_or_else(|| ApiError::conflict("host has no power control"))?;
        let kind = power::Kind::parse(&pc.kind)?;
        let secret = power::Secret::Bytes(self.password(pc)?);
        let control = kind.control(pc.address.clone(), pc.user.clone(), secret);
        control.power_off().await?;
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            match control.power_status().await {
                Ok(power::ChassisPower::Off) => return Ok(()),
                Ok(_) | Err(_) if tokio::time::Instant::now() >= deadline => {
                    return Err(ApiError::unavailable(format!(
                        "host {} was told to power off but is not confirmed off within {}s",
                        host.meta.name,
                        self.timeout.as_secs()
                    )));
                }
                Ok(_) | Err(_) => tokio::time::sleep(self.poll).await,
            }
        }
    }
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
    /// How this call proved the host is off: a power-off the server
    /// confirmed, or the operator's word.
    pub proof: Proof,
    /// Set when a power-off was tried and did not happen, saying why. The
    /// proof is then `operator` and the machines moved on that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_off_failed: Option<String>,
    pub evacuated: Vec<Evacuated>,
    /// Machines left on the host's record, and why.
    pub skipped: Vec<Left>,
}

/// What an evacuation settled on: the proof it used, what it moved, what it
/// left, and a note when a power-off was tried and failed.
pub struct Evacuation {
    pub proof: Proof,
    pub power_off_failed: Option<String>,
    pub evacuated: Vec<Evacuated>,
    pub skipped: Vec<Left>,
}

// slab's rows, read as views so forge does not depend on slab. They ignore
// every field they do not need and are never written back. `evacuate` and
// [`has_local_volume`] read both, so the kind is named once here.

type Volume = Resource<VolumeSpecView, VolumeStatusView>;
type VolumeType = Resource<VolumeTypeSpecView, NoStatus>;

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
    /// slab names its volume types `voltype`; there is no `kind` entry for it.
    const KIND: &'static str = "voltype";
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct NoStatus {}

/// Why a host without power control cannot be evacuated without a
/// confirmation, naming both ways out.
fn needs_proof(host: &Host) -> ApiError {
    let name = &host.meta.name;
    ApiError::conflict(format!(
        "host {name} has no power control the server can drive, so it cannot prove the host is \
         off. Its machines may be running, and evacuating them would start second copies. Either \
         confirm the host is dead (confirm_dead, `pi host evacuate --confirm-dead`), or give the \
         host power control so the server can power it off and check (`pi host power set`)"
    ))
}

/// Why a host that is disabled but still reporting cannot be evacuated on the
/// strength of a disabled flag alone. The host answers its control plane, so
/// its machines are almost certainly running.
fn still_reporting(host: &Host) -> ApiError {
    let name = &host.meta.name;
    let consequence = if host.spec.power_control.is_some() {
        "evacuating it cuts the host's power and hard-stops every machine on it, since their root \
         disks are here"
    } else {
        "the server cannot cut its power, so evacuating its machines risks starting second copies \
         while these keep running"
    };
    ApiError::conflict(format!(
        "host {name} is disabled but still reporting. {consequence}. Confirm the host is dead \
         (confirm_dead, `pi host evacuate --confirm-dead`) to go ahead, or use drain to empty a \
         working host",
    ))
}

/// What a fence attempt that did not end in a confirmed power-off has to say,
/// with the reason the host could not be proved off in it.
fn fence_failed(host: &Host, transport: &str, reason: &str) -> String {
    format!(
        "a power-off over {transport} was tried for host {} and failed ({reason})",
        host.meta.name
    )
}

/// Prove `host` is off, so its machines may start elsewhere without a
/// second copy.
///
/// A host that stopped reporting may be evacuated: it is gone, which is what
/// evacuate is for. A host that still reports may not, even once disabled —
/// that is a host that works, and a working host is emptied with drain.
///
/// A host that is down and has power control is fenced: the server powers it
/// off and reads the off state back, and no confirmation is needed. A host
/// that still reports needs `confirm_dead` before the server will touch its
/// power at all, because disabling a host is not a statement that it is dead.
///
/// A power-off that fails or cannot be confirmed within [`FENCE_TIMEOUT`]
/// refuses the evacuation and moves nothing, unless `confirm_dead` was given:
/// a BMC that died with its host is a dead host, and an operator who has
/// looked at it is allowed to say so. The report then says the power-off was
/// tried and failed, and on what grounds. Without the confirmation nothing
/// moves, and the refusal says why. Returns the proof used and, when a
/// power-off was attempted and did not happen, the note describing it.
async fn prove_off(
    host: &Host,
    opts: EvacuateOptions,
    fencer: Option<&dyn Fencer>,
) -> ApiResult<(Proof, Option<String>)> {
    // A host still doing its work is emptied with drain, which copies
    // machines off, not with an evacuation that abandons them here.
    let down = host.status.phase == Phase::Error;
    if !down && !host.spec.disabled {
        return Err(ApiError::conflict(format!(
            "host {} is up. Evacuate only acts on a host that is down, or one you have disabled. \
             Use drain to empty a working host",
            host.meta.name
        )));
    }
    // Disabled says no new work lands here; it does not say the host is off.
    // Only a host that stopped reporting may be powered off unasked.
    if !down && !opts.confirm_dead {
        return Err(still_reporting(host));
    }
    if let (Some(pc), Some(fencer)) = (host.spec.power_control.as_ref(), fencer) {
        // Power control is configured but the fencer cannot reach it (no
        // secret, no tool): that is a fault, and the operator's word only
        // stands in for it when they have explicitly said the host is dead.
        let transport = pc.kind.clone();
        if let Err(e) = fencer.fence(host).await {
            let why = fence_failed(host, &transport, &e.message);
            if !opts.confirm_dead {
                return Err(ApiError::conflict(format!(
                    "{why}, so nothing was evacuated and its machines were not started a second \
                     time. Confirm the host is dead (confirm_dead, \
                     `pi host evacuate --confirm-dead`) to evacuate anyway"
                )));
            }
            return Ok((Proof::Operator, Some(why)));
        }
        return Ok((Proof::PowerOff, None));
    }
    if opts.confirm_dead {
        return Ok((Proof::Operator, None));
    }
    Err(needs_proof(host))
}

/// Hand the machines of a dead host back to the scheduler.
///
/// Nothing moves until the host is proved off (see [`prove_off`]). Safe to
/// call again: a machine that was evacuated is no longer on the host, so a
/// second call only sees what the first one skipped.
pub async fn evacuate(
    store: &Store,
    host: &Host,
    opts: EvacuateOptions,
    fencer: Option<&dyn Fencer>,
) -> ApiResult<Evacuation> {
    let (proof, power_off_failed) = prove_off(host, opts, fencer).await?;
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
            proof = proof.as_str(),
            "forge: machine evacuated; it starts from its image without its old root disk"
        );
        evacuated.push(Evacuated {
            id: m.meta.id.clone(),
            name: m.meta.name.clone(),
            rebuilt_from: m.spec.image.clone(),
            volumes: attached.iter().map(|v| v.meta.name.clone()).collect(),
        });
    }
    record_proof(
        store,
        host,
        proof,
        power_off_failed.as_deref(),
        evacuated.len(),
    );
    Ok(Evacuation {
        proof,
        power_off_failed,
        evacuated,
        skipped,
    })
}

/// Write an audit record naming the proof an evacuation used, alongside the
/// `evacuate` action the gateway already records. When a power-off was tried
/// and failed the reason rides along as an audit note, so the record says that
/// the fence was attempted and did not happen, not only whose word was taken
/// instead. A store error is logged, not returned: on a follower the
/// write is refused, and the evacuation itself has already happened.
fn record_proof(
    store: &Store,
    host: &Host,
    proof: Proof,
    power_off_failed: Option<&str>,
    moved: usize,
) {
    let mut meta = pistack_types::Meta::new("audit", "audit", None);
    meta.name = meta.id.to_string();
    let mut notes = std::collections::BTreeMap::new();
    if let Some(why) = power_off_failed {
        notes.insert("power_off_failed".to_owned(), why.to_owned());
    }
    let event = gate::audit::AuditEvent {
        meta,
        spec: gate::audit::AuditSpec {
            method: "POST".into(),
            path: format!("/compute/hosts/{}/evacuate", host.meta.id),
            action: format!("evacuate:proof_{}", proof.audit_tag()),
            kind: kind::HOST.into(),
            resource: host.meta.id.to_string(),
            notes,
            ..Default::default()
        },
        status: gate::audit::AuditStatus {},
    };
    if let Err(e) = store.collection::<gate::audit::AuditEvent>().create(event) {
        tracing::debug!(host = %host.meta.id, error = %e, "forge: evacuation proof was not audited");
        return;
    }
    tracing::info!(
        host = %host.meta.id,
        proof = proof.as_str(),
        moved,
        power_off_failed = power_off_failed.unwrap_or("none"),
        "forge: host evacuated"
    );
}
