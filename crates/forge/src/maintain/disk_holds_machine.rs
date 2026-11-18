// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{machine_disks_are_shared, Host, Machine};
use pistack_store::Store;

use super::shared_backends::SHARED_BACKENDS;
use super::volume::Volume;
use super::volume_names::volume_names;
use super::volume_type::VolumeType;

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
