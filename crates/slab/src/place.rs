// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Read just enough of a forge machine and host to know which agent should
//! attach a disk. slab does not depend on forge; these views ignore every
//! field they do not need.

use pistack_store::Store;
use pistack_types::{kind, ApiError, ApiResult, Id, Phase, Resource, Spec};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct MachineSpecView {}

impl Spec for MachineSpecView {
    const KIND: &'static str = kind::MACHINE;
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct MachineStatusView {
    #[serde(default)]
    host: Option<Id>,
    #[serde(default)]
    phase: Phase,
}

/// Annotation forge puts on a machine it evacuated off a dead host. The
/// value is that host's id. Same key as `forge::EVACUATED_FROM_ANNOTATION`.
const EVACUATED_FROM: &str = "pistack.io/evacuated-from";

/// Where an evacuated machine was and where it runs now, as agent node
/// names.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Moved {
    pub from: String,
    pub to: String,
}

/// The move of a machine that was evacuated and is running again on
/// another host. `None` for any other machine, and while an evacuated one
/// is still waiting for a host or starting.
pub(crate) fn evacuated_move(store: &Store, machine: &Id) -> ApiResult<Option<Moved>> {
    let Some(placed) = store
        .collection::<Resource<MachineSpecView, MachineStatusView>>()
        .try_get(machine)?
    else {
        return Ok(None);
    };
    let Some(from) = placed
        .meta
        .annotations
        .get(EVACUATED_FROM)
        .and_then(|id| Id::parse(kind::HOST, id))
    else {
        return Ok(None);
    };
    let Some(now) = placed.status.host else {
        return Ok(None);
    };
    if placed.status.phase != Phase::Ready || now == from {
        return Ok(None);
    }
    let hosts = store.collection::<Resource<HostSpecView, HostStatusView>>();
    let (Some(from), Some(to)) = (hosts.try_get(&from)?, hosts.try_get(&now)?) else {
        return Ok(None);
    };
    if from.spec.node_name.is_empty() || to.spec.node_name.is_empty() {
        return Ok(None);
    }
    Ok(Some(Moved {
        from: from.spec.node_name,
        to: to.spec.node_name,
    }))
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct HostSpecView {
    #[serde(default)]
    node_name: String,
}

impl Spec for HostSpecView {
    const KIND: &'static str = kind::HOST;
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct HostStatusView {}

/// Agent node name for the host that runs `machine`.
pub fn node_for_machine(store: &Store, machine: &Id) -> ApiResult<String> {
    let placed = store
        .collection::<Resource<MachineSpecView, MachineStatusView>>()
        .get(machine)?;
    let host = placed.status.host.ok_or_else(|| {
        ApiError::unprocessable(format!("machine {machine} is not placed on a host"))
    })?;
    let host = store
        .collection::<Resource<HostSpecView, HostStatusView>>()
        .get(&host)?;
    if host.spec.node_name.is_empty() {
        return Err(ApiError::unprocessable(format!(
            "host {} has no node name",
            host.meta.id
        )));
    }
    Ok(host.spec.node_name)
}

/// Whether `machine` is still in the store. A deleted machine leaves no row.
pub(crate) fn machine_exists(store: &Store, machine: &Id) -> ApiResult<bool> {
    Ok(store
        .collection::<Resource<MachineSpecView, MachineStatusView>>()
        .try_get(machine)?
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_name_comes_from_the_placed_host() {
        let store = Store::in_memory().unwrap();
        let host = store
            .collection::<Resource<HostSpecView, HostStatusView>>()
            .create(Resource::new(
                "fleet",
                None,
                HostSpecView {
                    node_name: "fleet".into(),
                },
            ))
            .unwrap();
        let mut machine = Resource::<MachineSpecView, MachineStatusView>::new(
            "demo",
            Some(Id::new(kind::PROJECT)),
            MachineSpecView {},
        );
        machine.status.host = Some(host.meta.id.clone());
        let machine = store
            .collection::<Resource<MachineSpecView, MachineStatusView>>()
            .create(machine)
            .unwrap();
        assert_eq!(node_for_machine(&store, &machine.meta.id).unwrap(), "fleet");

        let mut loose = Resource::<MachineSpecView, MachineStatusView>::new(
            "loose",
            Some(Id::new(kind::PROJECT)),
            MachineSpecView {},
        );
        loose.status.host = None;
        let loose = store
            .collection::<Resource<MachineSpecView, MachineStatusView>>()
            .create(loose)
            .unwrap();
        assert!(node_for_machine(&store, &loose.meta.id).is_err());
    }
}
