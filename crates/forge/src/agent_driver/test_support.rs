// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{new_host, new_machine, Host, Machine, MachineSpec};
use chrono::Utc;
use pistack_rpc::proto::{AgentHello, Heartbeat, MachineReport};
use pistack_store::Store;
use pistack_types::{Id, Kind, Meta, Phase, Resources};

pub(super) fn machine(name: &str) -> Machine {
    new_machine(
        name,
        None,
        MachineSpec {
            size: "s.1x1".into(),
            image: "ubuntu".into(),
            ..Default::default()
        },
    )
}

pub(super) fn hello(node: &str) -> AgentHello {
    AgentHello {
        node_name: node.into(),
        capabilities: vec!["kvm".into(), "mock".into()],
        resources: Resources {
            vcpus: 8,
            memory_mib: 16384,
            disk_gib: 200,
            gpus: 0,
        },
        version: "t".into(),
        labels: [("rack".to_string(), "r1".to_string())]
            .into_iter()
            .collect(),
        underlay_ip: None,
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct ImgRow {
    #[serde(flatten)]
    pub(super) meta: Meta,
    #[serde(default)]
    pub(super) spec: serde_json::Value,
    #[serde(default)]
    pub(super) status: serde_json::Value,
}

impl Kind for ImgRow {
    const KIND: &'static str = "img";
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }
}

pub(super) fn put_image(store: &Store, name: &str, phase: &str, backing: Option<&str>) {
    put_image_at(store, name, phase, backing, None);
}

pub(super) fn put_image_at(
    store: &Store,
    name: &str,
    phase: &str,
    backing: Option<&str>,
    file_path: Option<&str>,
) {
    let meta = Meta::new(pistack_types::kind::IMAGE, name, None);
    let mut status = serde_json::Map::new();
    status.insert("phase".into(), serde_json::Value::String(phase.into()));
    if let Some(b) = backing {
        status.insert("backing_ref".into(), serde_json::Value::String(b.into()));
    }
    if let Some(p) = file_path {
        status.insert("file_path".into(), serde_json::Value::String(p.into()));
    }
    store
        .collection::<ImgRow>()
        .create(ImgRow {
            meta,
            spec: serde_json::json!({}),
            status: serde_json::Value::Object(status),
        })
        .unwrap();
}

/// A ready image with a file on this server, owned by `project`.
pub(super) fn put_image_for(
    store: &Store,
    name: &str,
    project: Option<&Id>,
    visibility: &str,
    file: &str,
) -> Id {
    let meta = Meta::new(pistack_types::kind::IMAGE, name, project.cloned());
    let id = meta.id.clone();
    store
        .collection::<ImgRow>()
        .create(ImgRow {
            meta,
            spec: serde_json::json!({ "format": "qcow2", "visibility": visibility }),
            status: serde_json::json!({ "phase": "ready", "file_path": file }),
        })
        .unwrap();
    id
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct MemberRow {
    #[serde(flatten)]
    pub(super) meta: Meta,
    pub(super) spec: serde_json::Value,
}

impl Kind for MemberRow {
    const KIND: &'static str = "imgmem";
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }
}

pub(super) fn image_file(tag: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "pistack-boot-{tag}-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    std::fs::write(&path, b"disk").unwrap();
    path.to_string_lossy().into_owned()
}

pub(super) fn machine_in(project: Option<&Id>, image: &str) -> Machine {
    let mut m = machine("web");
    m.meta.project = project.cloned();
    m.spec.image = image.into();
    m
}

pub(super) fn placed(name: &str, host: &Id, networks: &[&str]) -> Machine {
    let mut m = machine(name);
    m.status.host = Some(host.clone());
    m.status.phase = Phase::Ready;
    m.spec.networks = networks.iter().map(|s| (*s).to_string()).collect();
    m
}

/// A host whose machine disks are on storage every host can see.
pub(super) fn host_with(name: &str, node: &str, underlay: Option<&str>, phase: Phase) -> Host {
    let mut h = new_host(
        name,
        node,
        Resources {
            vcpus: 4,
            memory_mib: 8192,
            disk_gib: 100,
            gpus: 0,
        },
        vec!["kvm".into()],
        [(
            crate::types::DISK_LABEL.to_owned(),
            crate::types::DISK_SHARED.to_owned(),
        )]
        .into_iter()
        .collect(),
    );
    h.spec.underlay_ip = underlay.map(str::to_owned);
    h.status.phase = phase;
    h
}

pub(super) fn reports(node: &str, ids: &[&Id]) -> Heartbeat {
    Heartbeat {
        node_name: node.into(),
        resources_free: Resources::default(),
        machines: ids
            .iter()
            .map(|id| MachineReport {
                id: (*id).clone(),
                phase: Phase::Ready,
                message: String::new(),
                resources: None,
                addresses: vec![],
                power: Some(pistack_rpc::proto::PowerState::On),
                probes: vec![],
            })
            .collect(),
        at: Utc::now(),
        ovn: None,
        usage: None,
    }
}
