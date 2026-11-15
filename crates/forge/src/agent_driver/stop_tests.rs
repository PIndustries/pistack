// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::rpc_driver::RpcDriver;
use crate::driver::HostDriver as _;
use pistack_rpc::{CommandQueue, QueueConfig};
use pistack_types::Id;
use std::time::Duration;

#[tokio::test]
async fn rpc_driver_timeout_is_unavailable() {
    let queue = CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_millis(50),
        capacity: 4,
        heartbeat_interval: Duration::from_secs(5),
        ovn: None,
    });
    let driver = RpcDriver::for_node(queue, "offline");
    let e = driver
        .delete(&Id::new(pistack_types::kind::MACHINE))
        .await
        .unwrap_err();
    assert_eq!(e.code, pistack_types::error::ErrorCode::Unavailable);
}
