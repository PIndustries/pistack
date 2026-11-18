// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{IntOrName, ServicePort};

pub(crate) fn port_matches(port: &ServicePort, want: &IntOrName) -> bool {
    match want {
        IntOrName::Int(n) => port.port == *n,
        IntOrName::Name(name) => !name.is_empty() && port.name == *name,
    }
}
