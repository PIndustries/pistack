// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! With the OVN backend selected, a machine that names a network is sent to
//! its host with the port's OVN binding, and is not sent at all while the
//! port has none. One test in its own binary, because the backend is chosen
//! by a process-wide environment variable.

use std::time::Duration;

use fabric::{MockBackend, NetworkSpec, PortBinding, SubnetSpec};
use forge::{new_machine, HostDriver, MachineSpec, RpcDriver};
use pistack_rpc::{CommandKind, CommandQueue, CommandResult, MachineRequest};
use pistack_store::Store;
use pistack_types::{error::ErrorCode, kind, Id, Meta};

const NODE: &str = "node-a";

#[tokio::test]
async fn a_machine_on_an_ovn_network_starts_with_its_port_binding() {
    std::env::set_var("PISTACK_FABRIC_BACKEND", "ovn");

    let store = Store::in_memory().unwrap();
    // Records are written through the mock backend; this test is about
    // what forge reads back and sends, not about OVN's northbound rows.
    let fabric = fabric::Client::new(store.clone(), std::sync::Arc::new(MockBackend::new()));
    let project = Id::new(kind::PROJECT);
    let net = fabric
        .create_network(
            Meta::new(kind::NETWORK, "app", Some(project.clone())),
            NetworkSpec::default(),
        )
        .unwrap();
    fabric
        .create_subnet(
            Meta::new(kind::SUBNET, "app-net", Some(project.clone())),
            SubnetSpec {
                network: net.meta.id.clone(),
                cidr: "10.90.0.0/24".parse().unwrap(),
                gateway: Some("10.90.0.1".parse().unwrap()),
                allocation_pools: vec![],
                dns_nameservers: vec![],
                enable_dhcp: true,
                host_routes: vec![],
            },
        )
        .unwrap();
    let machine = new_machine(
        "box",
        Some(project.clone()),
        MachineSpec {
            size: "s.1x1".into(),
            image: "ubuntu-24.04".into(),
            networks: vec!["app".into()],
            ..MachineSpec::default()
        },
    );
    let port = fabric
        .create_port_for_device_with(&project, "app", machine.meta.id.clone(), &[])
        .await
        .unwrap();
    let bind = |binding: Option<PortBinding>| {
        fabric
            .ports()
            .modify(&port.meta.id, |p| {
                p.status.binding = binding.clone();
                Ok(())
            })
            .unwrap();
    };

    let queue = CommandQueue::default();
    let driver = RpcDriver::for_node(queue.clone(), NODE).with_store(store.clone());

    // Fabric has not written the port to OVN yet: nothing is sent.
    bind(None);
    let err = driver.start(&machine).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(err.message.contains("not bound in OVN"), "{err}");
    assert!(queue
        .next(NODE, Duration::from_millis(50), 4)
        .await
        .is_empty());

    let iface = port.meta.id.to_string();
    bind(Some(PortBinding {
        vif_type: "ovs".into(),
        vif_details: [
            ("bridge".to_string(), "br-int".to_string()),
            ("iface_id".to_string(), iface.clone()),
        ]
        .into_iter()
        .collect(),
    }));
    let agent = {
        let queue = queue.clone();
        tokio::spawn(async move {
            let cmds = queue.next(NODE, Duration::from_secs(5), 4).await;
            assert_eq!(cmds.len(), 1);
            assert_eq!(cmds[0].kind, CommandKind::StartMachine);
            let req: MachineRequest = serde_json::from_value(cmds[0].payload.clone()).unwrap();
            let running =
                serde_json::json!({ "id": req.id.to_string(), "addresses": ["10.90.0.2"] });
            queue
                .ack(NODE, CommandResult::ok(cmds[0].id).with_payload(running))
                .unwrap();
            req
        })
    };
    driver.start(&machine).await.unwrap();
    let req = agent.await.unwrap();
    assert_eq!(req.nics.len(), 1);
    assert_eq!(req.nics[0].address, "10.90.0.2");
    assert_eq!(req.nics[0].segment_id, None);
    assert!(req.overlay.is_none());
    let binding = req.nics[0].binding.as_ref().unwrap();
    assert_eq!(binding.vif_type, "ovs");
    assert_eq!(binding.vif_details["bridge"], "br-int");
    assert_eq!(binding.vif_details["iface_id"], iface);
}
