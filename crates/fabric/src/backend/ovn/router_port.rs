// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Port;

/// A router port as the router wants it.
pub(in crate::backend::ovn) struct RouterPort {
    pub(in crate::backend::ovn) port: Port,
    /// `<address>/<prefix>` on the port's subnet.
    pub(in crate::backend::ovn) network: String,
    pub(in crate::backend::ovn) gateway: bool,
}
