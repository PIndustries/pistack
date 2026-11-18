// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;

/// Out-of-band power control is set and cleared by a cloud admin, and read
/// back without the password. All three return the control (or null) under
/// `power_control`, never a password.
pub(crate) fn power_view(host: &Host) -> serde_json::Value {
    let power = host.spec.power_control.as_ref().map(|pc| {
        serde_json::json!({
            "kind": pc.kind,
            "address": pc.address,
            "user": pc.user,
            "secret": pc.secret,
        })
    });
    serde_json::json!({ "power_control": power })
}
