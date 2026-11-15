// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::rpc_driver::RpcDriver;
use super::test_support::*;
use crate::driver::HostDriver as _;
use crate::types::PowerState;
use pistack_rpc::proto::{CommandKind, CommandResult, MachineRef, MachineRequest, RunningMachine};
use pistack_rpc::CommandQueue;
use pistack_types::ApiError;
use std::time::Duration;

#[tokio::test]
async fn rpc_driver_round_trips_through_queue() {
    let queue = CommandQueue::default();
    let driver = RpcDriver::for_node(queue.clone(), "h1");
    let m = machine("web");

    // Simulate the agent: pull the command, answer it.
    let q = queue.clone();
    let mid = m.meta.id.clone();
    let agent = tokio::spawn(async move {
        let cmds = q.next("h1", Duration::from_secs(5), 16).await;
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].kind, CommandKind::StartMachine);
        let req: MachineRequest = serde_json::from_value(cmds[0].payload.clone()).unwrap();
        assert_eq!(req.id, mid);
        assert_eq!(req.image, "ubuntu");
        let rt = RunningMachine {
            id: req.id,
            addresses: vec!["10.1.0.9".into()],
            power: pistack_rpc::proto::PowerState::On,
            message: String::new(),
        };
        q.ack(
            "h1",
            CommandResult::ok(cmds[0].id).with_payload(serde_json::to_value(rt).unwrap()),
        )
        .unwrap();

        let cmds = q.next("h1", Duration::from_secs(5), 16).await;
        assert_eq!(cmds[0].kind, CommandKind::StopMachine);
        let r: MachineRef = serde_json::from_value(cmds[0].payload.clone()).unwrap();
        q.ack(
            "h1",
            CommandResult::from_api_error(
                cmds[0].id,
                &ApiError::not_found(format!("{} not here", r.id)),
            ),
        )
        .unwrap();
    });

    let rt = driver.start(&m).await.unwrap();
    assert_eq!(rt.addresses, vec!["10.1.0.9"]);
    assert_eq!(rt.power, PowerState::On);
    let e = driver.stop(&m.meta.id).await.unwrap_err();
    assert_eq!(e.code, pistack_types::error::ErrorCode::NotFound);
    assert!(driver.list().await.unwrap().is_empty());
    agent.await.unwrap();
}
