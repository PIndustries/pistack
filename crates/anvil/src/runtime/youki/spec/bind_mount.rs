// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::OciBind;

pub(in crate::runtime::youki) fn bind_mount(bind: &OciBind) -> serde_json::Value {
    let kind = if bind.directory { "rbind" } else { "bind" };
    let flag = if bind.read_only { "ro" } else { "rw" };
    serde_json::json!({
        "destination": bind.destination,
        "type": "bind",
        "source": bind.source,
        "options": [kind, "rprivate", flag],
    })
}
