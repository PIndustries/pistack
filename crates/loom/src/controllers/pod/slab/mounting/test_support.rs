// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::probes::test_support::*;
use crate::types::{Pod, PodSpec, PodVolume, VolumeSource};
use pistack_store::Store;
use pistack_types::{kind, Id, Phase};

pub(in crate::controllers::pod) fn put_volume(
    store: &Store,
    name: &str,
    phase: Phase,
    attached: bool,
    device: Option<&str>,
    backend_ref: Option<&str>,
    host: Option<&Id>,
) -> Id {
    let vol = store
        .collection::<crate::controllers::pod::slab::SlabVolume>()
        .create(pistack_types::Resource::new(
            name,
            None,
            crate::controllers::pod::slab::SlabVolSpec { size_gib: 1 },
        ))
        .unwrap();
    store
        .collection::<crate::controllers::pod::slab::SlabVolume>()
        .modify(&vol.meta.id, |v| {
            v.status.phase = phase;
            if attached {
                v.status.attached_to = Some(Id::new(kind::MACHINE));
                v.status
                    .conditions
                    .push(crate::controllers::pod::slab::SlabCondition {
                        kind: "Attached".into(),
                        status: true,
                    });
            }
            v.status.device = device.map(str::to_string);
            v.status.backend_ref = backend_ref.map(str::to_string);
            v.status.host = host.map(|id| id.to_string());
            Ok(())
        })
        .unwrap();
    vol.meta.id
}
pub(in crate::controllers::pod) fn pod_with_volume(store: &Store, vol: Id) -> Id {
    let pod = Pod::new(
        "web",
        None,
        PodSpec {
            containers: vec![ctr("web", None, None)],
            volumes: vec![PodVolume {
                name: "data".into(),
                source: VolumeSource::Volume(vol),
            }],
            networks: vec!["default".into()],
            ..PodSpec::default()
        },
    );
    store.collection::<Pod>().create(pod).unwrap().meta.id
}
