// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;

use super::named::named;

/// True when the machine named at least one network.
pub fn wants_port(m: &Machine) -> bool {
    !named(&m.spec.networks).is_empty()
}
