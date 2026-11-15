// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::rpc_driver::RpcDriver;
impl RpcDriver {
    pub fn node(&self) -> &str {
        &self.node
    }
}
