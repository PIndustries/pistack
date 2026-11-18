// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{
    delete_requested, Host, Machine, MachineKind, PowerState, EVACUATED_FROM_ANNOTATION,
    NODE_PIN_ANNOTATION,
};
use pistack_store::Store;
use pistack_types::{ApiResult, Phase, Resource};

use super::evacuate_options::EvacuateOptions;
use super::evacuated::Evacuated;
use super::evacuation::Evacuation;
use super::fencer::Fencer;
use super::left::Left;
use super::migrating_to::migrating_to;
use super::no_status::NoStatus;
use super::prove_off::prove_off;
use super::record_proof::record_proof;
use super::shared_backends::SHARED_BACKENDS;
use super::volume_spec_view::VolumeSpecView;
use super::volume_status_view::VolumeStatusView;
use super::volume_type_spec_view::VolumeTypeSpecView;

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
