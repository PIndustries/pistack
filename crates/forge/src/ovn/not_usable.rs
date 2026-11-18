// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{usable_underlay, Host};

use super::chassis_ready::chassis_ready;
use super::refusal::Refusal;

/// Why `host` cannot take a machine on an OVN network. `None` when it can.
pub(crate) fn not_usable(host: &Host) -> Option<Refusal> {
    if usable_underlay(host.spec.underlay_ip.as_deref()).is_none() {
        return Some(Refusal::NoUnderlay);
    }
    (!chassis_ready(host)).then_some(Refusal::NotReady)
}
