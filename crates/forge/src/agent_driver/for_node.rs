// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::rpc_driver::RpcDriver;
use pistack_rpc::CommandQueue;
impl RpcDriver {
    pub fn for_node(queue: CommandQueue, node: impl Into<String>) -> Self {
        Self {
            queue,
            node: node.into(),
            store: None,
        }
    }
}
