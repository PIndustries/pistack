// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::router_namespace::RouterNamespace;

pub(in crate::overlay) struct PendingRouter {
    pub(in crate::overlay) at: usize,
    pub(in crate::overlay) ns: RouterNamespace,
}
