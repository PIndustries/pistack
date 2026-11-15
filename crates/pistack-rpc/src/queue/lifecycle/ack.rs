// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::error::RpcError;
use crate::proto::CommandResult;
use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Deliver a result from `node`. The command must be pending and must
    /// have been issued to that node.
    pub fn ack(&self, node: &str, result: CommandResult) -> Result<(), RpcError> {
        let id = result.id;
        if self.ack_all(node, vec![result]).accepted == 1 {
            Ok(())
        } else {
            Err(RpcError::UnknownCommand(id))
        }
    }
}
